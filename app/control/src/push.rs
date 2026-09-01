//! Web Push（RFC 8030／8291／8292）—— 驗證碼一到就推到家人手機上。
//!
//! 面板自己當推送伺服器：酬載加密後 POST 給 FCM／Apple，用 VAPID 表明身分。
//! 自己寫而不用 `web-push` crate，是為了不帶進第二套 HTTP 客戶端。
//!
//! ⚠️ 驗證碼放在通知內文，所以加密不是可選的 —— 金鑰材料只有那台裝置有。
//! ⚠️ iOS 只有「加到主畫面」之後才有 `PushManager`，且忽略 `icon` 與
//!    `actions`。所以內文只放碼，不寫「點一下複製」（iPhone 上做不到）。

use anyhow::{bail, Context, Result};
use base64::Engine;
use hmac::{Hmac, Mac};
// 走 p256 轉出來的 rand_core：版本天生跟它對得上，不必自己再列一個頂層相依
use p256::elliptic_curve::rand_core::{OsRng, RngCore};
use p256::elliptic_curve::sec1::ToEncodedPoint;
use serde::Serialize;
use sha2::Sha256;

use crate::db;

/// 推送服務保留這則通知多久。比驗證碼本身的壽命長就沒有意義了。
const TTL_SECS: u32 = 900;

/// VAPID JWT 有效期（RFC 8292 建議不超過 24 小時）。
const JWT_LIFETIME_SECS: i64 = 12 * 3600;

/// RFC 8188 的記錄大小。酬載遠小於這個值，只會有一筆記錄。
const RECORD_SIZE: u32 = 4096;

const B64: base64::engine::general_purpose::GeneralPurpose =
    base64::engine::general_purpose::URL_SAFE_NO_PAD;

/// 要送到裝置上的通知。欄位名對應 service worker 讀的那幾個。
#[derive(Debug, Clone, Serialize)]
pub struct Notification {
    pub title: String,
    pub body: String,
    /// 同 tag 的新通知蓋掉舊的 —— 兩組碼並排是最容易抄錯的情境。
    pub tag: String,
    /// 點擊後要開啟的路徑。
    pub url: String,
    /// 給 Android 的「複製」按鈕用。沒抽到碼時為 None。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

pub struct Push {
    client: reqwest::Client,
    /// VAPID 的 `sub`：推送服務出問題時聯絡得到營運者。
    contact: String,
}

impl Push {
    pub fn new(mail_from: &str) -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .unwrap_or_default(),
            contact: format!("mailto:{}", mail_from.trim()),
        }
    }

    /// 推一則通知給一台裝置。
    ///
    /// `Ok(false)` = 這個訂閱已經死了（404／410），呼叫端該刪掉它。
    pub async fn send(&self, db: &db::Db, sub: &db::PushSub, n: &Notification) -> Result<bool> {
        let (secret, public) = vapid_keys(db)?;
        let payload = serde_json::to_vec(n)?;

        let ua_public = B64.decode(&sub.p256dh).context("p256dh 不是合法的 base64url")?;
        let auth = B64.decode(&sub.auth).context("auth 不是合法的 base64url")?;
        let body = encrypt(&ua_public, &auth, &payload, None, None)?;

        let res = self
            .client
            .post(&sub.endpoint)
            .header("Authorization", vapid_header(&sub.endpoint, &secret, &public, &self.contact)?)
            .header("Content-Encoding", "aes128gcm")
            .header("Content-Type", "application/octet-stream")
            .header("TTL", TTL_SECS.to_string())
            // 驗證碼是使用者正在等的東西，值得叫醒裝置
            .header("Urgency", "high")
            .body(body)
            .send()
            .await
            .context("連不上推送服務")?;

        let status = res.status();
        // 推送服務在說「別再推了」。留著只會每次都失敗且無法自癒。
        if status.as_u16() == 404 || status.as_u16() == 410 {
            return Ok(false);
        }
        if !status.is_success() {
            let detail = res.text().await.unwrap_or_default();
            bail!("推送服務回 {status}：{}", detail.chars().take(200).collect::<String>());
        }
        Ok(true)
    }
}

// ── VAPID（RFC 8292）────────────────────────────────

/// 取出金鑰對，第一次呼叫時產生。回 (私鑰, 公鑰的 base64url)。
///
/// ⚠️ 換掉這把等於所有既有訂閱一次作廢（推送服務會全部回 403）。
///    只在缺的時候產生，絕不順手輪替。
pub fn vapid_keys(db: &db::Db) -> Result<(p256::SecretKey, String)> {
    if let (Some(sk), Some(pk)) = (
        db::get_setting(db, db::keys::VAPID_PRIVATE)?,
        db::get_setting(db, db::keys::VAPID_PUBLIC)?,
    ) {
        let bytes = B64.decode(&sk).context("VAPID 私鑰不是合法的 base64url")?;
        let secret = p256::SecretKey::from_slice(&bytes).context("VAPID 私鑰無效")?;
        return Ok((secret, pk));
    }

    let secret = p256::SecretKey::random(&mut OsRng);
    let public = B64.encode(
        secret
            .public_key()
            .to_encoded_point(false) // 未壓縮的 65 bytes，瀏覽器要的就是這個形式
            .as_bytes(),
    );
    db::seed_setting(db, db::keys::VAPID_PRIVATE, &B64.encode(secret.to_bytes()))?;
    db::seed_setting(db, db::keys::VAPID_PUBLIC, &public)?;

    // 重讀：併發時 seed 可能沒寫進去，要拿實際生效的那把（同 db::hmac_key）。
    let sk = db::get_setting(db, db::keys::VAPID_PRIVATE)?.context("無法建立 VAPID 金鑰")?;
    let pk = db::get_setting(db, db::keys::VAPID_PUBLIC)?.context("無法建立 VAPID 金鑰")?;
    let bytes = B64.decode(&sk)?;
    Ok((p256::SecretKey::from_slice(&bytes)?, pk))
}

/// `Authorization: vapid t=<jwt>,k=<公鑰>`。
fn vapid_header(
    endpoint: &str,
    secret: &p256::SecretKey,
    public_b64: &str,
    contact: &str,
) -> Result<String> {
    let jwt = vapid_jwt(&audience(endpoint)?, secret, contact, db::now())?;
    Ok(format!("vapid t={jwt},k={public_b64}"))
}

/// `aud` 是推送服務的 origin，不是完整 endpoint —— 帶路徑會被判成受眾不符。
fn audience(endpoint: &str) -> Result<String> {
    let rest = endpoint
        .strip_prefix("https://")
        .context("推送 endpoint 必須是 https")?;
    let host = rest.split('/').next().filter(|h| !h.is_empty()).context("推送 endpoint 沒有主機名")?;
    Ok(format!("https://{host}"))
}

fn vapid_jwt(aud: &str, secret: &p256::SecretKey, sub: &str, now: i64) -> Result<String> {
    use p256::ecdsa::{signature::Signer, Signature, SigningKey};

    let header = B64.encode(br#"{"typ":"JWT","alg":"ES256"}"#);
    let claims = B64.encode(serde_json::to_vec(&serde_json::json!({
        "aud": aud,
        "exp": now + JWT_LIFETIME_SECS,
        "sub": sub,
    }))?);
    let signing_input = format!("{header}.{claims}");

    let key = SigningKey::from(secret);
    // JWS 要的是 r‖s 的定長形式，不是 DER
    let sig: Signature = key.sign(signing_input.as_bytes());
    Ok(format!("{signing_input}.{}", B64.encode(sig.to_bytes())))
}

// ── 酬載加密（RFC 8291 + RFC 8188）──────────────────

/// HKDF-Extract：就是一次 HMAC。
fn extract(salt: &[u8], ikm: &[u8]) -> [u8; 32] {
    let mut mac = <Hmac<Sha256>>::new_from_slice(salt).expect("HMAC 接受任意長度金鑰");
    mac.update(ikm);
    mac.finalize().into_bytes().into()
}

/// HKDF-Expand，只要一個區塊（L ≤ 32）—— 這裡最長只取 32。
fn expand(prk: &[u8], info: &[u8], len: usize) -> Vec<u8> {
    debug_assert!(len <= 32, "只實作單一區塊的 Expand");
    let mut mac = <Hmac<Sha256>>::new_from_slice(prk).expect("HMAC 接受任意長度金鑰");
    mac.update(info);
    mac.update(&[1u8]);
    mac.finalize().into_bytes()[..len].to_vec()
}

/// 把酬載加密成 `aes128gcm` 的內容編碼形式。
///
/// `salt` 與 `ephemeral` 只有測試指定 —— RFC 8291 §5 的測試向量要固定值。
fn encrypt(
    ua_public: &[u8],
    auth: &[u8],
    plaintext: &[u8],
    salt: Option<[u8; 16]>,
    ephemeral: Option<p256::SecretKey>,
) -> Result<Vec<u8>> {
    use aes_gcm::aead::{Aead, KeyInit, Payload};
    use aes_gcm::{Aes128Gcm, Nonce};

    let salt = match salt {
        Some(s) => s,
        None => {
            let mut s = [0u8; 16];
            OsRng.fill_bytes(&mut s);
            s
        }
    };
    let as_secret = ephemeral.unwrap_or_else(|| p256::SecretKey::random(&mut OsRng));
    let as_public = as_secret.public_key().to_encoded_point(false);
    let as_public = as_public.as_bytes();

    let ua_key = p256::PublicKey::from_sec1_bytes(ua_public).context("裝置公鑰無效")?;
    let shared = p256::ecdh::diffie_hellman(as_secret.to_nonzero_scalar(), ua_key.as_affine());

    // RFC 8291 §3.4：key_info 綁進雙方公鑰，換了任一邊金鑰就完全不同。
    let mut key_info = Vec::with_capacity(14 + 65 + 65);
    key_info.extend_from_slice(b"WebPush: info\0");
    key_info.extend_from_slice(ua_public);
    key_info.extend_from_slice(as_public);
    let ikm = expand(&extract(auth, shared.raw_secret_bytes()), &key_info, 32);

    // 之後就是 RFC 8188 的一般流程
    let prk = extract(&salt, &ikm);
    let cek = expand(&prk, b"Content-Encoding: aes128gcm\0", 16);
    let nonce = expand(&prk, b"Content-Encoding: nonce\0", 12);

    // 0x02 是「最後一筆記錄」的分隔位元組（RFC 8188 §2）
    let mut padded = plaintext.to_vec();
    padded.push(0x02);

    let cipher = Aes128Gcm::new_from_slice(&cek).context("CEK 長度不對")?;
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce), Payload { msg: &padded, aad: &[] })
        .map_err(|_| anyhow::anyhow!("酬載加密失敗"))?;

    // 標頭：salt(16) ‖ rs(4) ‖ idlen(1) ‖ keyid(公鑰 65 bytes)
    let mut out = Vec::with_capacity(21 + as_public.len() + ciphertext.len());
    out.extend_from_slice(&salt);
    out.extend_from_slice(&RECORD_SIZE.to_be_bytes());
    out.push(as_public.len() as u8);
    out.extend_from_slice(as_public);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 8291 §5 的測試向量，釘死整條金鑰推導鏈。
    /// ⚠️ 寫錯不會有錯誤訊息，推送服務照收、手機靜靜地沒反應。
    #[test]
    fn matches_the_rfc8291_test_vector() {
        let plaintext = b"When I grow up, I want to be a watermelon";
        let ua_public = B64
            .decode("BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4")
            .unwrap();
        let auth = B64.decode("BTBZMqHH6r4Tts7J_aSIgg").unwrap();
        let salt: [u8; 16] = B64.decode("DGv6ra1nlYgDCS1FRnbzlw").unwrap().try_into().unwrap();
        let as_secret = p256::SecretKey::from_slice(
            &B64.decode("yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw").unwrap(),
        )
        .unwrap();

        let got = encrypt(&ua_public, &auth, plaintext, Some(salt), Some(as_secret)).unwrap();

        let want = B64
            .decode("DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A_yl95bQpu6cVPTpK4Mqgkf1CXztLVBSt2Ks3oZwbuwXPXLWyouBWLVWGNWQexSgSxsj_Qulcy4a-fN")
            .unwrap();
        assert_eq!(got, want, "RFC 8291 測試向量對不上，金鑰推導有問題");
    }

    /// 每次都要換 salt 與臨時金鑰 —— 同 CEK 配同 nonce 是 AES-GCM 最致命的誤用。
    #[test]
    fn each_encryption_is_unique() {
        let ua_public = B64
            .decode("BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4")
            .unwrap();
        let auth = B64.decode("BTBZMqHH6r4Tts7J_aSIgg").unwrap();
        let a = encrypt(&ua_public, &auth, b"hi", None, None).unwrap();
        let b = encrypt(&ua_public, &auth, b"hi", None, None).unwrap();
        assert_ne!(a, b, "同樣的明文不該產生同樣的密文");
        assert_ne!(a[..16], b[..16], "salt 必須每次都換");
    }

    /// `aud` 只能是 origin。帶了路徑推送服務會判成受眾不符而回 401。
    #[test]
    fn audience_drops_the_path() {
        assert_eq!(
            audience("https://fcm.googleapis.com/fcm/send/abc123").unwrap(),
            "https://fcm.googleapis.com"
        );
        assert_eq!(
            audience("https://web.push.apple.com/QRSTUV").unwrap(),
            "https://web.push.apple.com"
        );
    }

    #[test]
    fn audience_rejects_non_https() {
        assert!(audience("http://insecure.example/x").is_err());
        assert!(audience("https:///nohost").is_err());
    }

    /// JWT 要是三段、且能被公鑰驗回來。
    #[test]
    fn jwt_is_verifiable_with_the_public_key() {
        use p256::ecdsa::{signature::Verifier, Signature, VerifyingKey};

        let secret = p256::SecretKey::from_slice(
            &B64.decode("yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw").unwrap(),
        )
        .unwrap();
        let jwt = vapid_jwt("https://fcm.googleapis.com", &secret, "mailto:a@b.c", 1_700_000_000)
            .unwrap();

        let parts: Vec<&str> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3, "JWT 必須是三段");

        let claims: serde_json::Value =
            serde_json::from_slice(&B64.decode(parts[1]).unwrap()).unwrap();
        assert_eq!(claims["aud"], "https://fcm.googleapis.com");
        assert_eq!(claims["sub"], "mailto:a@b.c");
        assert_eq!(claims["exp"], 1_700_000_000 + JWT_LIFETIME_SECS);

        let vk = VerifyingKey::from(secret.public_key());
        let sig = Signature::from_slice(&B64.decode(parts[2]).unwrap()).unwrap();
        vk.verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &sig)
            .expect("簽章驗不過");
    }

    /// 金鑰對存下去之後必須每次都拿到同一把 ——
    /// 換掉等於所有既有訂閱作廢。
    #[test]
    fn vapid_keys_are_stable_once_created() {
        let db = db::test_db();
        let (s1, p1) = vapid_keys(&db).unwrap();
        let (s2, p2) = vapid_keys(&db).unwrap();
        assert_eq!(s1.to_bytes(), s2.to_bytes());
        assert_eq!(p1, p2);
    }

    /// 公鑰要是未壓縮的 65 bytes —— 瀏覽器的 applicationServerKey
    /// 只吃這個形式。
    #[test]
    fn public_key_is_uncompressed() {
        let db = db::test_db();
        let (_, public) = vapid_keys(&db).unwrap();
        let bytes = B64.decode(&public).unwrap();
        assert_eq!(bytes.len(), 65);
        assert_eq!(bytes[0], 0x04, "未壓縮點的前綴");
    }
}
