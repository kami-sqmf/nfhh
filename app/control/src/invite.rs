//! 邀請函連結。
//!
//! admin 登記一個 Email，面板順帶寄一封邀請函過去，信裡的按鈕直接落在
//! 「建立 Passkey」那一步 —— 不必再回頭輸入一次自己的位址、再等一組驗證碼。
//!
//! 這條路徑證明的事情跟驗證碼**完全一樣**：這個信箱是你的。連結只寄到那個
//! 位址，收得到它就等於收得到寄去那裡的碼。差別只在少了一次手動抄寫。
//!
//! v6 把註冊入口從「邀請連結」改成「登記 Email」，理由是連結可以被轉傳。
//! 這裡把連結加回來但不退回原點：
//!
//!   - 連結**綁定單一位址**，用它建出來的帳號永遠是那個信箱的。轉傳給別人
//!     等於把自己的位址送人，而不是憑空多一個名額。
//!   - 一樣是單次使用（註冊完成時 `used_at` 一落，連結就死了）。
//!   - admin 撤銷或重新登記時當場失效。
//!   - 存的是 HMAC 而非權杖本身，`invited_emails` 洩漏不等於可以換帳號。
//!
//! 收不到信、或連結被信箱客戶端吃掉時，原本那條「用 Email 加入 + 驗證碼」
//! 完全沒動，隨時可以退回去走。

use anyhow::Result;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use uuid::Uuid;

use crate::db;

/// 跟驗證碼分開的一把金鑰。同一把會讓兩種憑據的雜湊可以互相冒充。
const KEY_SETTING: &str = "invite_hmac_key";

/// 產生一支權杖。兩個 v4 UUID = 256 bit 的 CSPRNG 輸出 ——
/// 這是個沒有次數限制、也不過期的入口，長度必須讓猜測完全不成立。
pub fn generate() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

/// 存進 DB 的形式。查詢時把收到的權杖照樣算一次再比對。
pub fn hash(db: &db::Db, token: &str) -> Result<String> {
    let key = db::hmac_key(db, KEY_SETTING)?;
    let mut mac = <Hmac<Sha256>>::new_from_slice(key.as_bytes())
        .map_err(|e| anyhow::anyhow!("HMAC 金鑰無效: {e}"))?;
    mac.update(b"invite:");
    mac.update(token.trim().as_bytes());
    Ok(mac.finalize().into_bytes().iter().map(|b| format!("{b:02x}")).collect())
}

/// 信裡那個按鈕要指向的位址。
///
/// 路徑而不是查詢字串：轉貼進聊天室時整條都在，不會被只認 path 的
/// 連結預覽或安全掃描切掉後半段。SPA 的 fallback 本來就會把未知路徑
/// 回成 index.html，前端在啟動時認出這個路徑（見 main.js）。
pub fn link(origin: &str, token: &str) -> String {
    format!("{}/join/{token}", origin.trim_end_matches('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_256_bits_of_hex() {
        let t = generate();
        assert_eq!(t.len(), 64);
        assert!(t.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn tokens_do_not_repeat() {
        let set: std::collections::HashSet<_> = (0..50).map(|_| generate()).collect();
        assert_eq!(set.len(), 50, "隨機性壞掉了");
    }

    /// 存的必須是雜湊，不是權杖本身 —— 這張表洩漏不該等於可以換帳號。
    #[test]
    fn stored_value_does_not_contain_the_token() {
        let db = db::test_db();
        let t = generate();
        let h = hash(&db, &t).unwrap();
        assert!(!h.contains(&t));
        assert_eq!(h.len(), 64, "HMAC-SHA256 的十六進位是 64 字元");
    }

    #[test]
    fn hash_is_stable_and_distinguishes_tokens() {
        let db = db::test_db();
        assert_eq!(hash(&db, "abc").unwrap(), hash(&db, " abc ").unwrap());
        assert_ne!(hash(&db, "abc").unwrap(), hash(&db, "abd").unwrap());
    }

    /// 驗證碼跟邀請連結各用一把金鑰：同一個字串在兩邊算出來的不能一樣，
    /// 否則其中一張表洩漏就能拿去偽造另一種憑據。
    #[test]
    fn invite_and_otp_hashes_are_not_interchangeable() {
        let db = db::test_db();
        assert_ne!(hash(&db, "123456").unwrap(), crate::otp::hash(&db, "", "123456").unwrap());
    }

    #[test]
    fn link_does_not_double_the_slash() {
        assert_eq!(link("https://dnf.example.com", "abc"), "https://dnf.example.com/join/abc");
        assert_eq!(link("https://dnf.example.com/", "abc"), "https://dnf.example.com/join/abc");
    }

}
