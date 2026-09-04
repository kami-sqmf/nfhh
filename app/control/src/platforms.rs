//! 平台清單。
//!
//! 唯一來源是 `config/smartdns/domain-set/*.list` —— 跟 smartdns 讀的是同一批
//! 檔案，所以面板列出的平台不可能跟實際被代理的網域不一致。新增平台 =
//! 丟一個 `.list` 進去；停用 = 改名成 `.disabled`（掃描時自然跳過）。
//!
//! 使用者可見的平台必須在檔案開頭宣告顯示名，並可選擇宣告品牌色：
//!
//! ```text
//! # platform-name: Disney+
//! # platform-color: #0063E5
//! ```
//!
//! 顏色是選填的。沒宣告時面板會從代號推導一個穩定的色相 —— 那保證每個
//! 平台都有可分辨的標記，但不會剛好是品牌色。
//!
//! 沒有這行的集合視為基礎設施 —— 例如 `test.list` 的診斷網域，以及
//! `*-cdn.list` 這種「同一平台的額外網域」。後者尤其不能加標頭，
//! 否則啟用時授權矩陣會冒出第二個同名平台。基礎設施集合照樣被 smartdns
//! 解析，只是不會出現在授權矩陣或驗證碼分頁裡。

use serde::Serialize;
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
pub struct Platform {
    /// 檔名去掉副檔名，例如 `netflix`。也是 smartdns domain-set 的名字。
    pub code: String,
    /// 給人看的名字，例如 `Disney+`。
    pub name: String,
    /// 品牌色（`#RRGGBB`）。None = 由前端從代號推導。
    pub color: Option<String>,
}

/// 只掃前幾行找標頭 —— 網域清單可能有數百行，沒必要整份讀進來。
const HEADER_SCAN_BYTES: usize = 512;
const HEADER: &str = "# platform-name:";
const HEADER_COLOR: &str = "# platform-color:";

/// 列出目前啟用中、且對使用者可見的平台，依 code 排序讓輸出穩定。
///
/// 讀不到目錄時回傳空清單而不是錯誤：平台清單拿不到只該讓授權矩陣空著，
/// 不該讓整個面板起不來。
pub fn list(dir: &str) -> Vec<Platform> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        tracing::warn!("讀不到平台目錄 {dir}，平台清單將是空的");
        return Vec::new();
    };

    let mut out: Vec<Platform> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let path = e.path();
            // `.list.disabled` 的副檔名是 disabled，不會通過這關
            if path.extension()? != "list" {
                return None;
            }
            let code = path.file_stem()?.to_str()?.to_string();
            let head = read_head(&path)?;
            let name = header(&head, HEADER)?;
            Some(Platform { code, color: header(&head, HEADER_COLOR).filter(is_hex), name })
        })
        .collect();

    out.sort_by(|a, b| a.code.cmp(&b.code));
    out
}

fn header(head: &str, key: &str) -> Option<String> {
    let v = head
        .lines()
        .find_map(|l| l.trim().strip_prefix(key))?
        .trim()
        .to_string();
    (!v.is_empty()).then_some(v)
}

/// 只收 `#RRGGBB`。壞掉的值退回推導色，而不是讓前端拿到一個
/// 會讓 CSS 整條宣告失效的字串。
fn is_hex(v: &String) -> bool {
    v.len() == 7 && v.starts_with('#') && v[1..].chars().all(|c| c.is_ascii_hexdigit())
}

/// 讀開頭若干位元組。從中間截斷可能切在多位元組字元上，
/// 所以用 lossy 轉換而不是 from_utf8 —— 標頭是 ASCII 前綴，不受影響。
fn read_head(path: &Path) -> Option<String> {
    use std::io::Read;
    let mut buf = vec![0u8; HEADER_SCAN_BYTES];
    let mut f = std::fs::File::open(path).ok()?;
    let n = f.read(&mut buf).ok()?;
    buf.truncate(n);
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// 該平台清單檔裡的網域（跳過註解與空行、一律小寫）。給連結背書用：
/// 卡片只替落在這些網域下的連結畫品牌按鈕。
///
/// ⚠️ 因此清單裡只能放**平台自己持有**的網域（見 DECISIONS.md）。
pub fn domains(dir: &str, code: &str) -> Vec<String> {
    let path = format!("{dir}/{code}.list");
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            // 檔案不在（平台為空、清單被 disabled）是正常情況；權限錯之類的要留下線索，
            // 否則按鈕集體消失沒人知道為什麼。
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!("讀不到平台清單 {path}（{e}），該平台的連結不會有品牌按鈕");
            }
            return Vec::new();
        }
    };
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| l.to_lowercase())
        .collect()
}

/// 從收件信箱推平台：`netflix@share.example.com` → `netflix`。
///
/// 用信箱而不是 DKIM 簽章網域：信箱是你自己指派的，意圖明確；
/// DKIM 網域經常是 `amazonses.com` 這類代寄商，對不上任何平台。
/// 認不出來就回 None —— 那封信只會出現在管理收件匣，不會外流到
/// 任何人的驗證碼分頁。
pub fn of_mailbox(mailbox: &str, known: &[Platform]) -> Option<String> {
    let local = mailbox.split('@').next()?.trim().to_lowercase();
    known
        .iter()
        .find(|p| p.code.eq_ignore_ascii_case(&local))
        .map(|p| p.code.clone())
}

/// 從寄件者位址推平台。
///
/// `map` 是「平台代號 → 樣式清單」。每個樣式兩種形態，由有沒有 `@` 決定：
///
///   - `info@members.netflix.com` → 比對**完整位址**
///   - `netflix.com`              → 比對**網域**，含子網域
///
/// 會做網域比對是因為實際資料就是這樣：同一個平台用了
/// `info@members.netflix.com` 與 `info@account.netflix.com`，
/// 只認完整位址的話，對方換一個子網域設定就失效了。
///
/// 網域比對用 `.` 邊界，`evil-netflix.com` 不會命中 `netflix.com`。
pub fn of_sender(
    sender: &str,
    map: &std::collections::BTreeMap<String, Vec<String>>,
) -> Option<String> {
    let addr = sender.trim().to_lowercase();
    let domain = addr.split('@').nth(1)?;

    map.iter()
        .find(|(_, pats)| {
            pats.iter().any(|pat| {
                let pat = pat.trim().to_lowercase();
                if pat.is_empty() {
                    false
                } else if pat.contains('@') {
                    addr == pat
                } else {
                    domain == pat || domain.ends_with(&format!(".{pat}"))
                }
            })
        })
        .map(|(code, _)| code.clone())
}

/// 判定一封信屬於哪個平台。
///
/// 寄件者優先於收件信箱：位址對應是管理員明確設定的，而信箱只是路由意圖。
/// 用 catch-all 收全部時信箱根本推不出東西 —— 那正是這個順序存在的理由。
pub fn classify(
    sender: Option<&str>,
    mailbox: &str,
    map: &std::collections::BTreeMap<String, Vec<String>>,
    mailboxes: &std::collections::BTreeMap<String, String>,
    known: &[Platform],
) -> Option<String> {
    sender
        .and_then(|s| of_sender(s, map))
        // 設定裡的代號可能指向一個已經被停用的平台，要擋掉
        .filter(|code| known.iter().any(|p| &p.code == code))
        // admin 明說的信箱對應。排在 local part 推導之前 ——
        // 推導只在「信箱名剛好等於代號」時才對，`disney@` 對 `disneyplus`
        // 就推不出來，那封信會變成誰都看不到。
        .or_else(|| of_mailbox_map(mailbox, mailboxes, known))
        .or_else(|| of_mailbox(mailbox, known))
}

/// 查 admin 設定的「平台 → 收件信箱」對應，反向找出平台。
fn of_mailbox_map(
    mailbox: &str,
    mailboxes: &std::collections::BTreeMap<String, String>,
    known: &[Platform],
) -> Option<String> {
    let mailbox = mailbox.trim().to_lowercase();
    mailboxes
        .iter()
        .find(|(_, m)| m.trim().to_lowercase() == mailbox)
        .map(|(code, _)| code.clone())
        .filter(|code| known.iter().any(|p| &p.code == code))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, body: &str) {
        std::fs::write(dir.join(name), body).unwrap();
    }

    fn fixture() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let p = d.path();
        write(p, "netflix.list", "# platform-name: Netflix\n# platform-color: #E50914\nnetflix.com\n");
        write(p, "disneyplus.list", "# Disney+ 控制平面網域\n# platform-name: Disney+\ndisneyplus.com\n");
        // 診斷用集合：沒有 platform-name，不該被使用者看到
        write(p, "test.list", "# 診斷用網域 —— 常設，請勿移除。\nifconfig.me\n");
        // 停用的集合：副檔名不是 .list
        write(p, "netflix-cdn.list.disabled", "# platform-name: Netflix CDN\nnflxvideo.net\n");
        d
    }

    #[test]
    fn lists_only_enabled_and_named_sets() {
        let d = fixture();
        let got = list(d.path().to_str().unwrap());
        let codes: Vec<_> = got.iter().map(|p| p.code.as_str()).collect();
        assert_eq!(codes, vec!["disneyplus", "netflix"], "test 與 .disabled 都不該出現");
        assert_eq!(got[0].name, "Disney+", "顯示名要從標頭讀，不是把檔名首字大寫");
        assert_eq!(got[1].color.as_deref(), Some("#E50914"), "品牌色要讀進來");
        assert_eq!(got[0].color, None, "沒宣告顏色就是 None，由前端推導");
    }

    /// 標頭不必在第一行 —— 檔案開頭通常有一段說明註解。
    #[test]
    fn header_may_follow_other_comments() {
        let d = fixture();
        let got = list(d.path().to_str().unwrap());
        assert!(got.iter().any(|p| p.code == "disneyplus"));
    }

    #[test]
    fn maps_mailbox_to_platform() {
        let d = fixture();
        let known = list(d.path().to_str().unwrap());
        assert_eq!(of_mailbox("netflix@share.example.com", &known).as_deref(), Some("netflix"));
        assert_eq!(of_mailbox("Disneyplus@Share.Example.com", &known).as_deref(), Some("disneyplus"));
        // 認不出來寧可回 None，也不要猜一個平台把驗證碼發給錯的人
        assert_eq!(of_mailbox("random@share.example.com", &known), None);
        assert_eq!(of_mailbox("test@share.example.com", &known), None, "診斷集合不是平台");
    }

    /// ⚠️ 這是 `disney@` 那個真實案例。平台代號來自檔名（`disneyplus.list`），
    /// 但實際收件信箱是 `disney@` —— local part 推導在這裡就是推不出來，
    /// 而那封信會變成誰都看不到。admin 明說的對應要救得回來。
    #[test]
    fn an_explicit_mapping_covers_what_the_local_part_cannot() {
        let d = fixture();
        let known = list(d.path().to_str().unwrap());
        let boxes = std::collections::BTreeMap::from([
            ("disneyplus".to_string(), "disney@share.example.com".to_string()),
        ]);

        // 沒有對應時推不出來
        assert_eq!(
            classify(None, "disney@share.example.com", &senders(), &no_mailboxes(), &known),
            None,
            "local part 推導對 disney@ / disneyplus 本來就不成立"
        );

        assert_eq!(
            classify(None, "disney@share.example.com", &senders(), &boxes, &known).as_deref(),
            Some("disneyplus")
        );
        // 大小寫與空白不該影響比對
        assert_eq!(
            classify(None, "  Disney@Share.Example.com ", &senders(), &boxes, &known).as_deref(),
            Some("disneyplus")
        );
    }

    /// 寄件者對應仍然優先 —— 信箱只是路由意圖，位址是明確設定的。
    #[test]
    fn the_sender_mapping_still_wins_over_the_mailbox_mapping() {
        let d = fixture();
        let known = list(d.path().to_str().unwrap());
        // 故意把信箱對應指到另一個平台，看誰贏
        let boxes = std::collections::BTreeMap::from([
            ("disneyplus".to_string(), "share@example.com".to_string()),
        ]);
        assert_eq!(
            classify(Some("info@members.netflix.com"), "share@example.com", &senders(), &boxes, &known)
                .as_deref(),
            Some("netflix")
        );
    }

    /// 對應指向已停用的平台時不能拿它當答案。
    #[test]
    fn a_mapping_to_a_disabled_platform_is_ignored() {
        let d = fixture();
        let known = list(d.path().to_str().unwrap());
        let boxes = std::collections::BTreeMap::from([
            ("gone".to_string(), "x@share.example.com".to_string()),
        ]);
        assert_eq!(classify(None, "x@share.example.com", &senders(), &boxes, &known), None);
    }

    /// 大多數既有測試不涉及信箱對應，用空的。
    fn no_mailboxes() -> std::collections::BTreeMap<String, String> {
        std::collections::BTreeMap::new()
    }

    fn senders() -> std::collections::BTreeMap<String, Vec<String>> {
        std::collections::BTreeMap::from([
            ("netflix".into(), vec!["netflix.com".into()]),
            ("disneyplus".into(), vec!["disneyplus@trx.mail2.disneyplus.com".into()]),
        ])
    }

    /// 網域樣式要涵蓋子網域 —— 線上實際就有 members. 與 account. 兩個。
    #[test]
    fn domain_pattern_covers_subdomains() {
        let m = senders();
        assert_eq!(of_sender("info@members.netflix.com", &m).as_deref(), Some("netflix"));
        assert_eq!(of_sender("info@account.netflix.com", &m).as_deref(), Some("netflix"));
        assert_eq!(of_sender("INFO@Netflix.COM", &m).as_deref(), Some("netflix"), "大小寫要正規化");
    }

    /// 但不能被前綴騙 —— evil-netflix.com 不是 netflix.com 的子網域。
    #[test]
    fn domain_pattern_respects_the_dot_boundary() {
        let m = senders();
        assert_eq!(of_sender("info@evil-netflix.com", &m), None);
        assert_eq!(of_sender("info@netflix.com.evil.tw", &m), None);
    }

    /// 含 @ 的樣式只認完整位址，不會外溢到同網域的其他信箱。
    #[test]
    fn address_pattern_is_exact() {
        let m = senders();
        assert_eq!(
            of_sender("disneyplus@trx.mail2.disneyplus.com", &m).as_deref(),
            Some("disneyplus")
        );
        assert_eq!(of_sender("other@trx.mail2.disneyplus.com", &m), None);
    }

    /// 寄件者優先於收件信箱 —— 位址對應是明確設定，信箱只是路由意圖。
    #[test]
    fn sender_wins_over_mailbox() {
        let d = fixture();
        let known = list(d.path().to_str().unwrap());
        let m = senders();
        assert_eq!(
            classify(Some("info@members.netflix.com"), "share@example.com", &m, &no_mailboxes(), &known).as_deref(),
            Some("netflix"),
            "catch-all 信箱推不出平台，要靠寄件者"
        );
        // 認不出寄件者時退回信箱
        assert_eq!(
            classify(Some("random@example.com"), "netflix@share.example.com", &m, &no_mailboxes(), &known).as_deref(),
            Some("netflix")
        );
        // 兩者都認不出就是 None，不猜
        assert_eq!(classify(Some("a@b.c"), "share@example.com", &m, &no_mailboxes(), &known), None);
    }

    /// 設定裡的代號可能指向已經停用的平台，那筆對應要視為無效。
    #[test]
    fn ignores_mappings_to_disabled_platforms() {
        let d = fixture();
        let known = list(d.path().to_str().unwrap());
        let m = std::collections::BTreeMap::from([("hbo".into(), vec!["hbo.com".into()])]);
        assert_eq!(classify(Some("x@hbo.com"), "share@example.com", &m, &no_mailboxes(), &known), None);
    }

    /// 目錄不存在只該讓清單空著，不該讓面板起不來。
    #[test]
    fn missing_directory_yields_empty_list() {
        assert!(list("/nonexistent/domain-set").is_empty());
    }

    #[test]
    fn domains_reads_the_list_skipping_comments() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("netflix.list"), "# platform-name: Netflix\n\nnetflix.com\nNFLXEXT.com\n").unwrap();
        assert_eq!(domains(dir.path().to_str().unwrap(), "netflix"), vec!["netflix.com", "nflxext.com"]);
        assert!(domains(dir.path().to_str().unwrap(), "nope").is_empty());
    }
}
