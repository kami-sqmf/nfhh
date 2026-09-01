//! smartdns 查詢稽核。
//!
//! 面板用它回答兩個問題：
//!   1. 「這個網路還在用嗎」—— 決定白名單條目要不要自動續期
//!   2. 「它最近查了什麼」—— 白名單頁展開後的逐筆內容
//!
//! 刻意做成「持續 tail ＋ 記憶體滾動視窗」，而不是每次請求去讀整個檔：
//! 查詢量大時 `audit.log` 會長到數 MB，每次重讀太貴；而視窗只需要幾十分鐘，
//! 重啟後歸零是可以接受的 —— 最差的後果是續期判斷晚一輪生效。
//!
//! ⚠️ 這裡的資料是家人查過的網域。誰看得到什麼由呼叫端分級：
//!    一般成員只拿得到自己那個 IP 的 [`Window::recent`]，
//!    admin 只拿得到 [`Window::stats`] 的彙總數字。

use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::os::unix::fs::MetadataExt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncSeekExt, BufReader};

/// 單一 IP 最多保留幾筆。防的是有裝置狂打 DNS 把記憶體吃光 ——
/// 視窗的用途是「看最近在做什麼」，不是完整紀錄。
const MAX_PER_IP: usize = 200;

#[derive(Debug, Clone, Serialize)]
pub struct Query {
    /// 流水號。同一秒、同一個網域會出現一模一樣的兩筆（A 與 AAAA 各記一次），
    /// 所以 `(at, domain)` 不是身分 —— 前端要拿它當清單的 key。
    pub seq: u64,
    pub at: i64,
    pub domain: String,
}

#[derive(Debug, Clone, Copy, Serialize, Default)]
pub struct Stats {
    pub count: usize,
    /// 最後一次查詢的時間。None = 視窗內完全沒有活動。
    pub last_at: Option<i64>,
}

pub struct Window {
    inner: Mutex<HashMap<String, VecDeque<Query>>>,
    keep_secs: i64,
    /// 全域遞增，跨 IP 也不重複。重啟歸零沒關係 —— 視窗本來就跟著歸零。
    seq: AtomicU64,
}

impl Window {
    pub fn new(keep_secs: i64) -> Self {
        Self { inner: Mutex::new(HashMap::new()), keep_secs, seq: AtomicU64::new(0) }
    }

    pub fn record(&self, ip: &str, domain: &str, at: i64) {
        let mut m = self.inner.lock().unwrap();
        let q = m.entry(ip.to_string()).or_default();
        let seq = self.seq.fetch_add(1, Ordering::Relaxed);
        q.push_back(Query { seq, at, domain: domain.to_string() });
        while q.len() > MAX_PER_IP {
            q.pop_front();
        }
    }

    /// 丟掉超出保留期的紀錄，並清掉空掉的 IP ——
    /// 否則已移除的白名單條目會在 map 裡永遠佔一個鍵。
    pub fn prune(&self, now: i64) {
        let cutoff = now - self.keep_secs;
        let mut m = self.inner.lock().unwrap();
        m.retain(|_, q| {
            while q.front().is_some_and(|x| x.at < cutoff) {
                q.pop_front();
            }
            !q.is_empty()
        });
    }

    /// 彙總數字。`within_secs` 讓呼叫端自己決定要看多近的區間
    /// （畫面上是 5 分鐘，續期判斷用比較寬的區間）。
    pub fn stats(&self, ip: &str, within_secs: i64, now: i64) -> Stats {
        let m = self.inner.lock().unwrap();
        let Some(q) = m.get(ip) else { return Stats::default() };
        let cutoff = now - within_secs;
        let recent = q.iter().filter(|x| x.at >= cutoff);
        Stats {
            count: recent.clone().count(),
            last_at: recent.map(|x| x.at).max(),
        }
    }

    /// 逐筆內容，新的在前。只給該 IP 的擁有者看。
    pub fn recent(&self, ip: &str, limit: usize, now: i64, within_secs: i64) -> Vec<Query> {
        let m = self.inner.lock().unwrap();
        let Some(q) = m.get(ip) else { return Vec::new() };
        let cutoff = now - within_secs;
        q.iter()
            .rev()
            .filter(|x| x.at >= cutoff)
            .take(limit)
            .cloned()
            .collect()
    }

}

/// 輪詢間隔。稽核檔是本機檔案，兩秒對「近五分鐘」的精度綽綽有餘。
const POLL: Duration = Duration::from_secs(2);

/// 持續追蹤稽核檔，把新的查詢餵進視窗。
///
/// 處理三種檔案狀態：
///   - **還沒出現**：smartdns 尚未開 audit 或還沒重啟。只警告一次然後繼續等。
///   - **第一次看到**：跳到檔尾。既有內容是舊的，而我們用「讀到的當下」
///     當時間戳，補讀歷史只會憑空生出假的活躍度。
///   - **輪替或被截斷**：`audit-num 2` 會讓 smartdns 換檔，inode 因此改變。
///     新檔案的內容是新的，從頭讀。
pub async fn tail(window: Arc<Window>, path: String) {
    let mut ino: Option<u64> = None;
    let mut pos: u64 = 0;
    let mut warned_missing = false;

    loop {
        match tokio::fs::metadata(&path).await {
            Err(_) => {
                if !warned_missing {
                    tracing::warn!(
                        "讀不到 smartdns 稽核檔 {path}，查詢統計會是空的。\
                         確認 smartdns.conf 已開 audit-enable 並重啟 smartdns。"
                    );
                    warned_missing = true;
                }
            }
            Ok(meta) => {
                if warned_missing {
                    tracing::info!("稽核檔 {path} 出現了，開始追蹤");
                    warned_missing = false;
                }
                let cur = meta.ino();
                match ino {
                    None => {
                        pos = meta.len();
                        ino = Some(cur);
                    }
                    Some(prev) if prev != cur || meta.len() < pos => {
                        tracing::debug!("稽核檔已輪替，從新檔案開頭繼續");
                        pos = 0;
                        ino = Some(cur);
                    }
                    _ => {}
                }
                if meta.len() > pos {
                    match read_new(&window, &path, pos).await {
                        Ok(next) => pos = next,
                        Err(e) => tracing::warn!("讀取稽核檔失敗: {e}"),
                    }
                }
                window.prune(crate::db::now());
            }
        }
        tokio::time::sleep(POLL).await;
    }
}

/// 從 `from` 讀到目前的檔尾，回傳新的位移。
///
/// 最後一行可能還沒寫完（smartdns 正寫到一半），所以只在讀到換行時才
/// 推進位移 —— 半行留到下一輪重讀，避免把它當成一筆壞掉的紀錄丟掉。
async fn read_new(window: &Window, path: &str, from: u64) -> std::io::Result<u64> {
    let mut f = tokio::fs::File::open(path).await?;
    f.seek(std::io::SeekFrom::Start(from)).await?;

    let mut reader = BufReader::new(f);
    let mut pos = from;
    let mut line = String::new();
    let now = crate::db::now();

    loop {
        line.clear();
        let n = reader.read_line(&mut line).await?;
        if n == 0 || !line.ends_with('\n') {
            break;
        }
        pos += n as u64;
        if let Some((ip, domain)) = parse_line(line.trim_end()) {
            window.record(ip, domain, now);
        }
    }
    Ok(pos)
}

/// 從一行稽核紀錄取出「誰查了什麼」。
///
/// smartdns 的格式（對著 1.2026.08.05 的二進位確認過）是
/// `[時間] <client-ip> query <domain>, type N, time Nms, ...`。
///
/// **時間刻意不從行內解析。** 我們是持續 tail，讀到的當下就是寫入的當下；
/// 而行內是當地時間字串，自己轉 epoch 得再拖一個時區函式庫進來，
/// 換來的精度對「最近五分鐘」這種問題毫無意義。
pub fn parse_line(line: &str) -> Option<(&str, &str)> {
    // 用 rsplit 而非 split：有些建置會輸出 `[時間][pid][等級] `，
    // 從右邊找最後一個 "] " 兩種格式都對。
    let rest = line.rsplit_once("] ")?.1;
    let (ip, rest) = rest.split_once(" query ")?;

    // 認不出是 IP 就丟掉。稽核檔裡混著別的訊息，不是每行都是查詢紀錄。
    let ip = ip.trim();
    if ip.parse::<std::net::IpAddr>().is_err() {
        return None;
    }

    let domain = rest.split(',').next()?.trim();
    if domain.is_empty() {
        return None;
    }
    Some((ip, domain))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINE: &str = "[2026-08-30 14:41:07,123] 198.51.100.7 query ipv4-c001.nflxvideo.net, \
                        type 1, time 5ms, speed: 0.0ms, group default, result 203.0.113.10";

    #[test]
    fn parses_a_real_audit_line() {
        let (ip, domain) = parse_line(LINE).unwrap();
        assert_eq!(ip, "198.51.100.7");
        assert_eq!(domain, "ipv4-c001.nflxvideo.net");
    }

    /// 有些建置會多印 pid 與等級，從右邊找 "] " 兩種都對。
    #[test]
    fn tolerates_the_verbose_prefix() {
        let l = "[2026-08-30 14:41:07,123][ 1234][INFO] 2001:db8::1 query www.netflix.com, type 28, time 3ms";
        let (ip, domain) = parse_line(l).unwrap();
        assert_eq!(ip, "2001:db8::1");
        assert_eq!(domain, "www.netflix.com");
    }

    /// 稽核檔裡混著別的訊息，不是每行都是查詢紀錄。
    #[test]
    fn ignores_lines_that_are_not_queries() {
        assert!(parse_line("").is_none());
        assert!(parse_line("[2026-08-30 14:41:07,123] cache reload complete").is_none());
        assert!(parse_line("[2026-08-30 14:41:07,123] not-an-ip query x.com, type 1").is_none());
        assert!(parse_line("198.51.100.7 query x.com, type 1").is_none(), "沒有時間戳前綴");
    }

    /// A 與 AAAA 會讓同一個網域在同一秒各記一筆 —— 兩筆從 at/domain 完全
    /// 分不出來，所以 seq 必須不同，前端才有東西可以當清單的 key。
    #[test]
    fn identical_queries_still_get_distinct_ids() {
        let w = Window::new(1800);
        w.record("1.1.1.1", "www.netflix.com", 1000); // A
        w.record("1.1.1.1", "www.netflix.com", 1000); // AAAA

        let q = w.recent("1.1.1.1", 20, 1000, 300);
        assert_eq!(q.len(), 2);
        assert_ne!(q[0].seq, q[1].seq, "一模一樣的兩筆也要分得出來");
    }

    /// 流水號是全域的：不同 IP 的紀錄放進同一個清單也不會撞。
    #[test]
    fn ids_do_not_repeat_across_ips() {
        let w = Window::new(1800);
        w.record("1.1.1.1", "a.com", 1000);
        w.record("2.2.2.2", "a.com", 1000);
        assert_ne!(w.recent("1.1.1.1", 20, 1000, 300)[0].seq, w.recent("2.2.2.2", 20, 1000, 300)[0].seq);
    }

    #[test]
    fn counts_only_within_the_asked_window() {
        let w = Window::new(1800);
        w.record("1.1.1.1", "a.com", 1000);
        w.record("1.1.1.1", "b.com", 1500);
        w.record("1.1.1.1", "c.com", 1900);

        let s = w.stats("1.1.1.1", 300, 2000); // 只看最近 300 秒
        assert_eq!(s.count, 1, "只有 c.com 落在區間內");
        assert_eq!(s.last_at, Some(1900));

        assert_eq!(w.stats("1.1.1.1", 2000, 2000).count, 3);
    }

    #[test]
    fn unknown_ip_reports_no_activity() {
        let w = Window::new(1800);
        let s = w.stats("9.9.9.9", 300, 2000);
        assert_eq!(s.count, 0);
        assert_eq!(s.last_at, None, "沒有活動要能跟『剛剛有活動』分得開");
    }

    #[test]
    fn recent_returns_newest_first_and_respects_limit() {
        let w = Window::new(1800);
        for (i, d) in ["a.com", "b.com", "c.com"].iter().enumerate() {
            w.record("1.1.1.1", d, 1000 + i as i64);
        }
        let got = w.recent("1.1.1.1", 2, 2000, 1800);
        let domains: Vec<_> = got.iter().map(|q| q.domain.as_str()).collect();
        assert_eq!(domains, vec!["c.com", "b.com"]);
    }

    /// 已移除的白名單條目不該在 map 裡永遠佔一個鍵。
    #[test]
    fn pruning_drops_stale_ips_entirely() {
        let w = Window::new(300);
        w.record("1.1.1.1", "a.com", 1000);
        w.record("2.2.2.2", "b.com", 1900);

        w.prune(2000);
        assert_eq!(w.stats("1.1.1.1", 10_000, 2000).count, 0, "過期的要被清掉");
        assert_eq!(w.stats("2.2.2.2", 10_000, 2000).count, 1);
    }

    /// 狂打 DNS 的裝置不能把記憶體吃光。
    #[test]
    fn per_ip_history_is_bounded() {
        let w = Window::new(1800);
        for i in 0..(MAX_PER_IP as i64 + 50) {
            w.record("1.1.1.1", "flood.com", 1000 + i);
        }
        assert_eq!(w.stats("1.1.1.1", 10_000, 2000).count, MAX_PER_IP);
    }
}
