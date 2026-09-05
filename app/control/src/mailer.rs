//! 出站寄信 —— 註冊流程的 Email 驗證碼與邀請函。
//!
//! 走 Resend 的 REST API。Cloudflare Email Routing **只能收不能寄**，
//! 所以面板得自己有一條出站管道，不能沿用收信那套。
//!
//! 沒設金鑰時 [`Mailer::enabled`] 為 false，「用 Email 加入」整條路在 UI 上
//! 就是停用狀態 —— 而不是讓人填完信箱、按下去才發現寄不出去。
//!
//! 兩封信的排版方式刻意不同。驗證碼是純文字（見 [`text_body`]）；邀請函則交給
//! Resend 的樣板（dashboard 上編輯、`{{{VAR}}}` 代入變數），因為那是一封有版面
//! 的信，把它的 HTML 埋進 Rust 字串等於每次改文案都要重新編譯、重新部署。

use anyhow::{bail, Context, Result};
use std::time::Duration;

/// Resend 偶爾會慢，但註冊流程是使用者站在畫面前等，
/// 寧可早點失敗給明確訊息，也不要讓他盯著轉圈。
const TIMEOUT: Duration = Duration::from_secs(10);
const ENDPOINT: &str = "https://api.resend.com/emails";

pub struct Mailer {
    key: Option<String>,
    from: String,
    /// 邀請函樣板的 id 或別名（Resend dashboard 上那個 alias）。
    invite_template: String,
    client: reqwest::Client,
}

impl Mailer {
    pub fn new(key: String, from: String, invite_template: String) -> Self {
        Self {
            key: (!key.trim().is_empty()).then_some(key),
            from,
            invite_template,
            client: reqwest::Client::builder()
                .timeout(TIMEOUT)
                .build()
                .unwrap_or_default(),
        }
    }

    pub fn enabled(&self) -> bool {
        self.key.is_some()
    }

    /// 寄出一組驗證碼。
    ///
    /// 錯誤訊息刻意不帶回收件位址與碼本身 —— 這個字串會走到前端，
    /// 而註冊頁在「這個位址沒被邀請」之外不該再洩漏任何東西。
    pub async fn send_code(&self, to: &str, code: &str, ttl_minutes: i64) -> Result<()> {
        self.send(
            serde_json::json!({
                "from": self.from,
                "to": [to],
                "subject": format!("{code} — OTT 共享控制台驗證碼"),
                "text": text_body(code, ttl_minutes),
            }),
            "驗證碼寄送失敗，請稍後再試",
        )
        .await
    }

    /// 寄出邀請函。
    ///
    /// 走 Resend 的樣板：只送 `id` 與 `variables`，HTML 由 Resend 那邊套。
    /// **樣板要先發布**，草稿狀態送過去會被回 4xx。
    ///
    /// `platforms` 是給人看的平台名（例如「Netflix、Disney+」）。登記時沒指定
    /// 平台是合法的，那時信上要講明白之後才會開通，而不是印一個空白欄位
    /// 讓收信的人以為自己什麼都有。
    pub async fn send_invite(&self, to: &str, link: &str, platforms: &str) -> Result<()> {
        self.send(self.invite_body(to, link, platforms), "邀請函寄送失敗").await
    }

    /// 邀請函的請求內容。抽出來是為了讓測試看得到實際送出去的欄位 ——
    /// 變數名一個字打錯，樣板上就是一塊空白，而那只有真的寄一封才會發現。
    fn invite_body(&self, to: &str, link: &str, platforms: &str) -> serde_json::Value {
        serde_json::json!({
            "from": self.from,
            "to": [to],
            // 主旨由樣板決定，這裡給了反而會被 API 擋下來
            "template": {
                "id": self.invite_template,
                "variables": {
                    "INVITE_LINK": link,
                    "Platform": platforms,
                    "TARGET_EMAIL": to,
                },
            },
        })
    }

    /// 兩封信共用的送出與錯誤處理。
    ///
    /// `failure` 是要給使用者看的訊息 —— 刻意不帶回收件位址與憑據本身，
    /// 因為這個字串會走到前端。
    async fn send(&self, body: serde_json::Value, failure: &str) -> Result<()> {
        let key = self.key.as_deref().context("尚未設定寄信金鑰")?;

        let res = self
            .client
            .post(ENDPOINT)
            .bearer_auth(key)
            .json(&body)
            .send()
            .await
            .context("連不上寄信服務")?;

        if !res.status().is_success() {
            let status = res.status();
            // 內容進日誌不進回應：Resend 的錯誤訊息可能含收件位址
            let detail = res.text().await.unwrap_or_default();
            tracing::error!("寄信失敗 status={status} body={detail}");
            bail!("{failure}");
        }
        Ok(())
    }
}

/// 純文字內文。刻意不做 HTML：驗證碼信只有一個任務，
/// 而純文字在所有信箱裡都不會被排版搞砸，也不會被擋圖。
///
/// 主旨已經帶了碼，多數手機在通知列就能看到，不必開信。
fn text_body(code: &str, ttl_minutes: i64) -> String {
    format!(
        "你的驗證碼是 {code}\n\n\
         請在 {ttl_minutes} 分鐘內回到控制台輸入。\n\n\
         這組碼用來確認這個信箱是你的：第一次加入時，通過後會請你在手機上\n\
         建立 Passkey，之後登入都不再需要信箱或密碼；已經有帳號的人可以用它\n\
         暫時登入一次。\n\n\
         如果你沒有要求這組碼，忽略這封信即可，但不要把它交給任何人 ——\n\
         拿到它的人就能以你的身分進到控制台。\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mailer(key: &str) -> Mailer {
        Mailer::new(key.into(), "a@b.c".into(), "ott-share-invitation".into())
    }

    #[test]
    fn missing_key_disables_the_whole_path() {
        assert!(!mailer("").enabled());
        assert!(!mailer("   ").enabled(), "空白不算設定");
        assert!(mailer("re_x").enabled());
    }

    #[tokio::test]
    async fn sending_without_a_key_fails_before_any_request() {
        let m = mailer("");
        let err = m.send_code("x@y.z", "123456", 10).await.unwrap_err();
        assert!(err.to_string().contains("尚未設定"));
        let err = m.send_invite("x@y.z", "https://x/join/t", "Netflix").await.unwrap_err();
        assert!(err.to_string().contains("尚未設定"));
    }

    /// 樣板信的欄位名就是契約：Resend 那邊對不上不會報錯，只會寄出一封
    /// 有空格的信。這個測試把三個變數名釘死在這裡。
    #[test]
    fn invite_payload_matches_the_template_contract() {
        let b = mailer("re_x").invite_body("mei@example.com", "https://dnf.example.com/join/abc", "Netflix、Disney+");
        assert_eq!(b["template"]["id"], "ott-share-invitation");
        assert_eq!(b["template"]["variables"]["INVITE_LINK"], "https://dnf.example.com/join/abc");
        assert_eq!(b["template"]["variables"]["TARGET_EMAIL"], "mei@example.com");
        assert_eq!(b["template"]["variables"]["Platform"], "Netflix、Disney+");
        assert_eq!(b["to"][0], "mei@example.com");
        // 帶了 subject／html／text 之中任何一個，API 會回 validation error
        assert!(b.get("subject").is_none());
        assert!(b.get("html").is_none());
        assert!(b.get("text").is_none());
    }

    /// 端到端煙霧測試：真的寄一封信出去。
    ///
    /// 這是唯一能驗證「寄件網域已在 Resend 完成驗證」的方法 ——
    /// 唯寄送權限的金鑰讀不到 /domains，設定對不對只有寄了才知道。
    ///
    /// 預設不跑。要跑：
    /// ```sh
    /// set -a; . ./.env; set +a
    /// NFHH_SMOKE_TO=you@example.com cargo test -- --ignored smoke
    /// ```
    #[tokio::test]
    #[ignore = "會真的寄出一封信，需要有效的 RESEND_API_KEY"]
    async fn smoke_send_real_email() {
        let key = std::env::var("RESEND_API_KEY").expect("需要 RESEND_API_KEY");
        let from = std::env::var("NFHH_MAIL_FROM").unwrap_or_else(|_| "share@example.com".into());
        let to = std::env::var("NFHH_SMOKE_TO").expect("需要 NFHH_SMOKE_TO");

        let code = crate::otp::generate();
        // 寫到檔案而不是只印出來，讓核對不依賴終端機捲軸還在
        let _ = std::fs::write("/tmp/nfhh-smoke-code.txt", &code);
        println!("寄出的驗證碼：{code}（已存到 /tmp/nfhh-smoke-code.txt）");

        Mailer::new(key, from, "ott-share-invitation".into())
            .send_code(&to, &code, 10)
            .await
            .expect("寄送失敗 —— 多半是寄件網域還沒在 Resend 驗證");
        println!("已寄至 {to}");
    }

    /// 邀請函的端到端煙霧測試。樣板存不存在、發布了沒、變數名對不對，
    /// 這些 Resend 都只在真的送一次時才會告訴你。
    ///
    /// ```sh
    /// set -a; . ./.env; set +a
    /// NFHH_SMOKE_TO=you@example.com cargo test -- --ignored smoke_send_invite
    /// ```
    #[tokio::test]
    #[ignore = "會真的寄出一封信，需要有效的 RESEND_API_KEY"]
    async fn smoke_send_invite() {
        let key = std::env::var("RESEND_API_KEY").expect("需要 RESEND_API_KEY");
        let from = std::env::var("NFHH_MAIL_FROM").unwrap_or_else(|_| "share@example.com".into());
        let tpl = std::env::var("NFHH_INVITE_TEMPLATE")
            .unwrap_or_else(|_| "ott-share-invitation".into());
        let to = std::env::var("NFHH_SMOKE_TO").expect("需要 NFHH_SMOKE_TO");

        Mailer::new(key, from, tpl)
            .send_invite(&to, "https://dnf.example.com/join/smoketest", "Netflix、Disney+")
            .await
            .expect("寄送失敗 —— 多半是樣板還沒發布，或別名打錯");
        println!("已寄至 {to}");
    }

    /// 驗證碼現在也能登入（見 otp.rs），內文得講清楚「別把它給任何人」——
    /// 而且不能再寫「光有碼不能登入」，那句已經不是真的。
    #[test]
    fn body_tells_the_reader_not_to_hand_the_code_to_anyone() {
        let b = text_body("482913", 10);
        assert!(b.contains("482913"));
        assert!(b.contains("10 分鐘"));
        assert!(b.contains("不要把它交給任何人"));
        assert!(!b.contains("不能登入"));
    }
}
