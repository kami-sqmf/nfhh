//! Cloudflare Email Routing —— 查轉發收件人的驗證狀態，並建立目的地位址。
//!
//! ## 建立位址是冪等的（2026-09 實測）
//!
//! | 目標位址 | 回應 | 寄信 |
//! |---|---|---|
//! | 不存在 | 200 `unverified` | ✅ |
//! | 已存在且已驗證 | 200 回原紀錄（不重置驗證） | ❌ |
//! | 已存在且未驗證 | 200 或 429 `code 2025` | ✅ 重寄 |
//!
//! 所以「已存在就忽略」與「重發驗證信」都是同一支 POST，重打就好。
//!
//! ⚠️ 不要用 DELETE + POST 重發：DELETE 有冷卻（`code 2032`，實測約 11 分鐘），
//!    剛建的位址刪不掉，而且會誤刪已驗證位址的路由規則。
//!
//! 沒設帳戶或 token 時 [`Cloudflare::enabled`] 為 false，UI 顯示「未查詢」
//! 而不是假裝「尚未驗證」—— `cf_checked_at` 為 NULL 跟 `cf_verified_at`
//! 為 NULL 是兩件不同的事，schema 把它們分開就是為了這個。
//!
//! ⚠️ 需要的權限是**帳戶層級**的 `Email Routing Addresses`（讀 + 寫）。
//!    順帶一提：拿 `/user/tokens/verify` 檢查這種窄權限 token 會得到
//!    「Invalid API Token」，因為那支端點自己是 user 層級的。要驗證
//!    token 能不能用，直接打下面這支。

use anyhow::{bail, Context, Result};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(10);

/// 一筆轉發收件人在 Cloudflare 的狀態。
pub struct Destination {
    pub email: String,
    /// 通過驗證的時間（epoch 秒）。None = 尚未驗證。
    pub verified_at: Option<i64>,
}

pub struct Cloudflare {
    account: String,
    token: Option<String>,
    client: reqwest::Client,
}

impl Cloudflare {
    pub fn new(account: String, token: String) -> Self {
        let usable = !account.trim().is_empty() && !token.trim().is_empty();
        Self {
            account,
            token: usable.then_some(token),
            client: reqwest::Client::builder()
                .timeout(TIMEOUT)
                .build()
                .unwrap_or_default(),
        }
    }

    pub fn enabled(&self) -> bool {
        self.token.is_some()
    }

    /// 列出帳戶底下所有的轉發收件人位址。
    ///
    /// 這是**帳戶層級**的資源（`/accounts/{id}/...`），不是 zone 層級 ——
    /// 所以一定要有帳戶 ID，光有 token 組不出路徑。
    pub async fn destinations(&self) -> Result<Vec<Destination>> {
        let token = self.token.as_deref().context("尚未設定 Cloudflare token")?;
        let url = format!(
            "https://api.cloudflare.com/client/v4/accounts/{}/email/routing/addresses?per_page=100",
            self.account
        );

        let res = self
            .client
            .get(&url)
            .bearer_auth(token)
            .send()
            .await
            .context("連不上 Cloudflare")?;

        let status = res.status();
        let body: serde_json::Value = res.json().await.context("Cloudflare 回應不是 JSON")?;

        if !status.is_success() || body["success"] != serde_json::Value::Bool(true) {
            let msg = body["errors"][0]["message"]
                .as_str()
                .unwrap_or("未知錯誤")
                .to_string();
            bail!("Cloudflare 查詢失敗（{status}）：{msg}");
        }

        Ok(body["result"]
            .as_array()
            .map(|xs| {
                xs.iter()
                    .filter_map(|x| {
                        Some(Destination {
                            email: x["email"].as_str()?.trim().to_lowercase(),
                            // 缺欄位與 null 都代表尚未驗證
                            verified_at: x["verified"].as_str().and_then(parse_rfc3339_utc),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    /// 建立一個目的地位址；已存在就當作成功。
    ///
    /// `Ok(true)` = 驗證信寄出去了；`Ok(false)` = 沒寄（已驗證或冷卻中）。
    /// 兩者都不是錯誤，呼叫端只是要決定怎麼跟使用者說。
    pub async fn create_destination(&self, email: &str) -> Result<bool> {
        let token = self.token.as_deref().context("尚未設定 Cloudflare token")?;
        let url = format!(
            "https://api.cloudflare.com/client/v4/accounts/{}/email/routing/addresses",
            self.account
        );

        let res = self
            .client
            .post(&url)
            .bearer_auth(token)
            .json(&serde_json::json!({ "email": email.trim().to_lowercase() }))
            .send()
            .await
            .context("連不上 Cloudflare")?;

        let status = res.status();
        let body: serde_json::Value = res.json().await.context("Cloudflare 回應不是 JSON")?;

        if body["success"] == serde_json::Value::Bool(true) {
            // 已驗證的位址重打會回原紀錄，那代表這次沒有寄信
            return Ok(body["result"]["verified"].is_null());
        }

        let code = body["errors"][0]["code"].as_i64().unwrap_or(0);
        // 冷卻中。位址與狀態都對，當成錯誤會讓使用者以為要重新登記。
        if code == RATE_LIMITED {
            return Ok(false);
        }

        let msg = body["errors"][0]["message"].as_str().unwrap_or("未知錯誤");
        bail!("Cloudflare 建立位址失敗（{status}）：{msg}");
    }
}

/// 「驗證信剛寄過」。Cloudflare 自己就有節流，面板不必再疊一層。
const RATE_LIMITED: i64 = 2025;

/// 解析 Cloudflare 回的 `2024-07-28T14:42:55.951615Z` 成 epoch 秒。
///
/// 自己寫而不是拉一個日期函式庫進來：這是全專案唯一需要解析時間字串的
/// 地方，格式固定、永遠是 UTC（`Z` 結尾），沒有時區規則要處理。
/// 小數秒直接丟掉 —— 顯示的是「2 年前」，微秒毫無意義。
fn parse_rfc3339_utc(s: &str) -> Option<i64> {
    let s = s.trim();
    let (date, rest) = s.split_once('T')?;
    let time = rest.split(['.', 'Z', '+']).next()?;

    let mut d = date.split('-');
    let y: i64 = d.next()?.parse().ok()?;
    let m: i64 = d.next()?.parse().ok()?;
    let day: i64 = d.next()?.parse().ok()?;

    let mut t = time.split(':');
    let hh: i64 = t.next()?.parse().ok()?;
    let mm: i64 = t.next()?.parse().ok()?;
    let ss: i64 = t.next().unwrap_or("0").parse().ok()?;

    if !(1..=12).contains(&m) || !(1..=31).contains(&day) || hh > 23 || mm > 59 || ss > 60 {
        return None;
    }
    Some(days_from_civil(y, m, day) * 86400 + hh * 3600 + mm * 60 + ss)
}

/// 民用日期 → 自 1970-01-01 起的天數。
///
/// Howard Hinnant 的 `days_from_civil`：把年份的起點移到三月，
/// 二月與閏日就落在年尾，閏年規則因此不必特別處理。
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_credentials_disable_the_lookup() {
        assert!(!Cloudflare::new(String::new(), "t".into()).enabled());
        assert!(!Cloudflare::new("a".into(), String::new()).enabled());
        assert!(!Cloudflare::new("a".into(), "  ".into()).enabled());
        assert!(Cloudflare::new("a".into(), "t".into()).enabled());
    }

    #[test]
    fn epoch_zero_round_trips() {
        assert_eq!(parse_rfc3339_utc("1970-01-01T00:00:00Z"), Some(0));
    }

    /// 對照真實資料：這幾筆是帳戶裡實際存在的驗證時間。
    #[test]
    fn parses_real_cloudflare_timestamps() {
        // 2023-07-20T12:40:24Z
        assert_eq!(parse_rfc3339_utc("2023-07-20T12:40:24.693675Z"), Some(1_689_856_824));
        // 2024-07-28T14:42:55Z —— 閏年，且在 2 月之後
        assert_eq!(parse_rfc3339_utc("2024-07-28T14:42:55.951615Z"), Some(1_722_177_775));
        // 2025-03-12T10:14:17Z
        assert_eq!(parse_rfc3339_utc("2025-03-12T10:14:17.808689Z"), Some(1_741_774_457));
    }

    /// 閏日本身最容易錯 —— 三月起點的算法就是為了這個。
    #[test]
    fn handles_leap_day() {
        assert_eq!(parse_rfc3339_utc("2024-02-29T00:00:00Z"), Some(1_709_164_800));
        // 2100 不是閏年（能被 100 整除但不能被 400 整除）
        assert_eq!(
            parse_rfc3339_utc("2100-03-01T00:00:00Z").unwrap()
                - parse_rfc3339_utc("2100-02-28T00:00:00Z").unwrap(),
            86_400,
            "2100 年 2 月只有 28 天"
        );
    }

    #[test]
    fn rejects_garbage_instead_of_guessing() {
        assert_eq!(parse_rfc3339_utc(""), None);
        assert_eq!(parse_rfc3339_utc("not a date"), None);
        assert_eq!(parse_rfc3339_utc("2024-13-01T00:00:00Z"), None, "沒有 13 月");
        assert_eq!(parse_rfc3339_utc("2024-07-28T99:00:00Z"), None);
    }
}
