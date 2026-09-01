//! 註冊流程的 Email 一次性驗證碼。
//!
//! 這組碼只證明「這個信箱是你的」。通過之後才會請對方在裝置上建立 Passkey ——
//! **光有碼不能登入任何東西**，登入永遠需要 Passkey。所以這裡的威脅模型是
//! 「別讓人用不屬於自己的信箱去換一個帳號」，不是「保護登入憑證」。
//!
//! 碼存 HMAC 而非明碼：`email_otp` 表洩漏不該等於可以冒用任何人的信箱。
//! 純 SHA-256 在這裡沒有意義 —— 六位數只有一百萬種，離線暴力破解是微秒級的事。
//! 金鑰在首次使用時產生後存進 `settings`，不需要多一個環境變數讓人忘記設。

use anyhow::Result;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use uuid::Uuid;

use crate::db;

/// 有效期。設計稿的 1o 寫「10 分鐘內有效」。
pub const TTL_SECS: i64 = 600;
/// 重寄冷卻。1o 的畫面上有「重新寄送（0:42）」倒數。
pub const RESEND_COOLDOWN_SECS: i64 = 60;
/// 錯幾次就鎖掉這組碼。鎖掉之後要重寄，不是永久封鎖信箱。
pub const MAX_ATTEMPTS: i64 = 5;
/// 通過驗證後多久內必須完成 Passkey 註冊。
/// 給得短，避免「幾天前驗過」還能拿來建帳號。
pub const VERIFIED_WINDOW_SECS: i64 = 900;

/// 金鑰存在 `settings`，第一次用到時才產生（見 [`db::hmac_key`]）。
/// 邀請連結用的是**另一把**，兩者的雜湊不能互相冒充。
const KEY_SETTING: &str = "otp_hmac_key";

/// 把碼綁著信箱一起簽。否則同一組碼可以從 A 信箱的紀錄搬去 B 信箱用。
pub fn hash(db: &db::Db, email: &str, code: &str) -> Result<String> {
    let key = db::hmac_key(db, KEY_SETTING)?;
    let mut mac = <Hmac<Sha256>>::new_from_slice(key.as_bytes())
        .map_err(|e| anyhow::anyhow!("HMAC 金鑰無效: {e}"))?;
    mac.update(email.trim().to_lowercase().as_bytes());
    mac.update(b":");
    mac.update(code.as_bytes());
    Ok(hex(&mac.finalize().into_bytes()))
}

/// 六位數字。前導零要保留 —— `042913` 跟 `42913` 是不同的碼，
/// 而使用者看到的是六格輸入框。
pub fn generate() -> String {
    let b = Uuid::new_v4().into_bytes();
    let n = u32::from_be_bytes([b[0], b[1], b[2], b[3]]) % 1_000_000;
    format!("{n:06}")
}

/// 使用者可能從信件複製時帶到空白或連字號。
pub fn normalize(input: &str) -> String {
    input.chars().filter(|c| c.is_ascii_digit()).collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem() -> db::Db {
        db::test_db()
    }

    #[test]
    fn codes_are_six_digits_with_leading_zeros_kept() {
        for _ in 0..200 {
            let c = generate();
            assert_eq!(c.len(), 6, "六格輸入框，長度必須固定");
            assert!(c.chars().all(|c| c.is_ascii_digit()));
        }
    }

    #[test]
    fn codes_are_not_all_the_same() {
        let a: std::collections::HashSet<_> = (0..50).map(|_| generate()).collect();
        assert!(a.len() > 40, "隨機性壞掉了");
    }

    /// 同一組碼不能從別的信箱搬過來用。
    #[test]
    fn hash_binds_the_code_to_the_email() {
        let db = mem();
        let a = hash(&db, "mei@example.com", "123456").unwrap();
        let b = hash(&db, "other@example.com", "123456").unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn hash_is_stable_and_case_insensitive() {
        let db = mem();
        let a = hash(&db, "Mei@Example.com", "123456").unwrap();
        let b = hash(&db, "  mei@example.com ", "123456").unwrap();
        assert_eq!(a, b, "信箱正規化要跟 db 層一致");
        assert_ne!(a, hash(&db, "mei@example.com", "123457").unwrap());
    }

    /// 金鑰只產生一次；重讀不該換一把，否則所有在途的碼會突然失效。
    #[test]
    fn key_is_generated_once_and_reused() {
        let db = mem();
        let a = hash(&db, "mei@example.com", "123456").unwrap();
        let b = hash(&db, "mei@example.com", "123456").unwrap();
        assert_eq!(a, b);
    }

    /// 存的必須是雜湊，不是碼本身。
    #[test]
    fn stored_value_does_not_contain_the_code() {
        let db = mem();
        let h = hash(&db, "mei@example.com", "482913").unwrap();
        assert!(!h.contains("482913"));
        assert_eq!(h.len(), 64, "HMAC-SHA256 的十六進位是 64 字元");
    }

    #[test]
    fn normalize_strips_what_people_paste() {
        assert_eq!(normalize(" 482 913 "), "482913");
        assert_eq!(normalize("482-913"), "482913");
        assert_eq!(normalize("abc"), "");
    }
}
