//! nftables 白名單同步：把 DB 的白名單投影到 nft set。
//!
//! 每次變更都是整個 set 重建，不是增量增刪（理由見 DECISIONS.md）。
//! 同時維護 nft/clients.nft，供 nfhh-firewall.service 開機載入。

use crate::db::{self, Db};
use anyhow::{bail, Context, Result};
use std::io::Write;
use std::process::{Command, Stdio};

const TABLE: &str = "inet nfhh";

/// 把 DB 的白名單完整同步到執行中的 nft set 與持久化檔案。
pub fn sync(db: &Db, clients_nft_path: &str) -> Result<usize> {
    let purged = db::purge_expired(db)?;
    if purged > 0 {
        tracing::info!("清除 {purged} 筆過期白名單");
    }

    let entries = db::list_allow(db)?;
    let now = db::now();

    let mut v4 = Vec::new();
    let mut v6 = Vec::new();
    for e in &entries {
        let ttl = e.expires_at - now;
        if ttl <= 0 {
            continue;
        }
        let elem = format!("{} timeout {}s", e.ip, ttl);
        if e.ip.contains(':') {
            v6.push(elem);
        } else {
            v4.push(elem);
        }
    }

    apply_live(&v4, &v6)?;
    write_persist(clients_nft_path, &v4, &v6)?;
    Ok(v4.len() + v6.len())
}

/// 用單一 `nft -f -` 交易套用，避免中途失敗留下半套狀態。
fn apply_live(v4: &[String], v6: &[String]) -> Result<()> {
    let mut script = String::new();
    script.push_str(&format!("flush set {TABLE} clients_v4\n"));
    script.push_str(&format!("flush set {TABLE} clients_v6\n"));
    if !v4.is_empty() {
        script.push_str(&format!(
            "add element {TABLE} clients_v4 {{ {} }}\n",
            v4.join(", ")
        ));
    }
    if !v6.is_empty() {
        script.push_str(&format!(
            "add element {TABLE} clients_v6 {{ {} }}\n",
            v6.join(", ")
        ));
    }

    let mut child = Command::new("nft")
        .arg("-f")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("執行 nft 失敗 —— 容器內是否有 nftables？是否給了 NET_ADMIN？")?;

    child
        .stdin
        .as_mut()
        .context("取不到 nft 的 stdin")?
        .write_all(script.as_bytes())?;

    let out = child.wait_with_output()?;
    if !out.status.success() {
        bail!(
            "nft 套用失敗 ({}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// 寫入開機用的持久化檔案。
/// 先寫暫存檔再 rename，避免開機時剛好讀到寫到一半的內容。
fn write_persist(path: &str, v4: &[String], v6: &[String]) -> Result<()> {
    let mut s = String::from(
        "# ⚠️ 自動產生 —— 由控制平面依 SQLite 內容重寫，手動修改會被覆蓋。\n\
         # 開機時由 nfhh-firewall.service 載入，用途是補上「重開機後、控制平面啟動前」\n\
         # 這段空窗期的白名單，否則各樓層會被擋在外面。\n\
         # 真實來源是控制平面的 SQLite；這裡的 timeout 以寫檔當下重算。\n\n",
    );
    for e in v4 {
        s.push_str(&format!("add element {TABLE} clients_v4 {{ {e} }}\n"));
    }
    for e in v6 {
        s.push_str(&format!("add element {TABLE} clients_v6 {{ {e} }}\n"));
    }

    let tmp = format!("{path}.tmp");
    std::fs::write(&tmp, s).with_context(|| format!("寫入 {tmp} 失敗"))?;
    std::fs::rename(&tmp, path).with_context(|| format!("更名為 {path} 失敗"))?;
    Ok(())
}

/// 一次性遷移：把 clients.nft 既有的條目收進 DB。
/// 只在 DB 完全沒有白名單資料時執行，避免首次上線時 sync() 清掉手動加的條目。
pub fn import_legacy(db: &Db, path: &str, ttl_days: i64) -> Result<usize> {
    if !db::list_allow(db)?.is_empty() {
        return Ok(0);
    }
    let Ok(content) = std::fs::read_to_string(path) else {
        return Ok(0);
    };

    let mut n = 0;
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with('#') || !line.contains("add element") {
            continue;
        }
        // add element inet nfhh clients_v4 { 1.2.3.4 timeout 7d }
        let Some(inner) = line.split_once('{').and_then(|(_, r)| r.split_once('}')) else {
            continue;
        };
        let mut parts = inner.0.split_whitespace();
        let Some(ip) = parts.next() else { continue };
        if ip.parse::<std::net::IpAddr>().is_err() {
            continue;
        }
        // 沿用原本的剩餘時間；解析不出來就給預設 TTL
        let ttl = parts
            .nth(1) // 跳過 "timeout"
            .and_then(parse_duration)
            .unwrap_or(ttl_days * 86400);

        db::upsert_allow(db, ip, Some("由 clients.nft 匯入"), None, db::now() + ttl, ttl_days)?;
        n += 1;
    }
    if n > 0 {
        tracing::warn!("首次啟動：自 {path} 匯入 {n} 筆既有白名單，避免同步時被清空");
    }
    Ok(n)
}

fn parse_duration(s: &str) -> Option<i64> {
    let (num, unit) = s.split_at(s.find(|c: char| !c.is_ascii_digit())?);
    let n: i64 = num.parse().ok()?;
    Some(match unit {
        "s" => n,
        "m" => n * 60,
        "h" => n * 3600,
        "d" => n * 86400,
        _ => return None,
    })
}

/// 啟動時檢查 nft 表是否存在。
pub fn preflight() -> Result<()> {
    let out = Command::new("nft")
        .args(["list", "table", "inet", "nfhh"])
        .output()
        .context("找不到 nft 執行檔")?;
    if !out.status.success() {
        // 用 restart 不是 start：這個 unit 是 RemainAfterExit=yes 的 oneshot，
        // 表被手動刪掉後它仍是 active，start 對已 active 的 unit 什麼都不會做。
        bail!(
            "nft 表 inet nfhh 不存在。請先啟動防火牆服務：\n  \
             sudo systemctl restart nfhh-firewall.service\n\
             （docker.service 依賴這個 unit，restart 會連帶重啟 Docker）"
        );
    }
    Ok(())
}
