//! nfhh-control — OTT Household 管理面板
//!
//! 用 Passkey (WebAuthn) 認證，手機可一鍵把「目前所在網路的公網 IP」
//! 加進 DNS/proxy 白名單。
//!
//! 只綁 127.0.0.1，入口是 Cloudflare Tunnel；`NFHH_BIND` 設成非 loopback
//! 會拒絕啟動（理由見 DECISIONS.md）。

mod cloudflare;
mod db;
mod dnslog;
mod invite;
mod mail;
mod mailer;
mod nft;
mod otp;
mod platforms;
mod push;
mod ratelimit;

use anyhow::{Context, Result};
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::sync::Arc;
use tower_sessions::{MemoryStore, Session, SessionManagerLayer};
use webauthn_rs::prelude::*;

// ── 設定 ──────────────────────────────────────────────

struct Config {
    rp_id: String,
    origin: String,
    bind: String,
    db_path: String,
    clients_nft: String,
    dynamic_conf: String,
    dot_conf: String,
    dot_host: String,
    /// 平台清單的來源目錄（config/smartdns/domain-set），唯讀掛進容器。
    domain_set_dir: String,
    /// 每人可佔用的白名單條目數。
    ///
    /// v6 之前這是**全域**上限（`NFHH_MAX_ENTRIES=12`）。改成 per-user 之後
    /// 濫用防護的來源變成「每人 4 條 × 成員數」，而成員數由 admin 的
    /// Email 登記把關 —— 比一個全域數字更貼近真正想擋的事。
    max_per_user: i64,
    default_ttl_days: i64,
    /// Cloudflare Email Worker 推送信件用的共用密鑰。留空 = 停用該端點。
    mail_secret: String,
    mail_keep_days: i64,
    /// 可信的寄件品牌網域（比對 DKIM 簽章網域，不是信封寄件者）。
    /// 只是**種子值**，實際以 settings.sender_domains 為準（見 seed_settings／mail_ingest）。
    mail_allowed_senders: Vec<String>,
    /// 只是 `sender_verify_mode` 的**種子**（面板怎麼顯示未通過驗證的信：
    /// `0` = observe、`1` = enforce）。轉發閘門 `forward_enforce_sender` 固定種子
    /// 為 `1`、不看這個變數 —— 未通過驗證的信預設就不轉發，要放寬只能在面板改。
    mail_enforce_sender: bool,
    /// 收信端在 `Authentication-Results` 裡署名的 authserv-id。只有它寫的
    /// 驗證結果算數；Cloudflare Email Routing 是 `mx.cloudflare.net`。
    mail_authserv_id: String,
    /// smartdns 的查詢稽核檔。讀不到時查詢統計為空，其餘功能不受影響。
    dns_audit: String,
    /// Cloudflare 帳戶與 token，用來查轉發收件人的驗證狀態。
    /// 任一為空 = 停用該查詢，UI 顯示「未查詢」而不是假裝「尚未驗證」。
    cf_account: String,
    cf_token: String,
    /// Resend 金鑰與寄件位址。金鑰為空 = 「用 Email 加入」停用。
    resend_key: String,
    mail_from: String,
    /// 轉發信箱的網域，如 `share.example.com` —— 面板要自己組出
    /// `netflix@share.example.com` 這種 mailbox（登記邀請時順帶新增轉發）。
    /// 只是**種子值**，實際以 settings 表為準。
    mail_domain: String,
    /// 邀請函樣板在 Resend 上的 id 或別名。樣板要先發布，草稿寄不出去。
    invite_template: String,
    /// 稽核的保留期與列數上限。公開端點的每次失敗都寫一列，沒有上限就是免費的磁碟。
    audit_keep_days: i64,
    audit_max_rows: i64,
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

impl Config {
    fn from_env() -> Result<Self> {
        Ok(Self {
            rp_id: env_or("NFHH_RP_ID", "dnf.example.com"),
            origin: env_or("NFHH_ORIGIN", "https://dnf.example.com"),
            bind: env_or("NFHH_BIND", "127.0.0.1:8081"),
            db_path: env_or("NFHH_DB", "/data/control.db"),
            clients_nft: env_or("NFHH_CLIENTS_NFT", "/nft/clients.nft"),
            dynamic_conf: env_or("NFHH_DYNAMIC_CONF", "/smartdns/dynamic-ip.conf"),
            dot_conf: env_or("NFHH_DOT_CONF", "/smartdns/dot.conf"),
            dot_host: env_or("NFHH_DOT_HOST", "dns.example.com"),
            domain_set_dir: env_or("NFHH_DOMAIN_SET_DIR", "/domain-set"),
            max_per_user: env_or("NFHH_MAX_PER_USER", "4").parse().unwrap_or(4),
            default_ttl_days: env_or("NFHH_TTL_DAYS", "7").parse().unwrap_or(7),
            mail_secret: env_or("NFHH_MAIL_SECRET", ""),
            mail_keep_days: env_or("NFHH_MAIL_KEEP_DAYS", "14").parse().unwrap_or(14),
            mail_allowed_senders: env_or("NFHH_MAIL_ALLOWED_SENDERS", "netflix.com,disneyplus.com")
                .split(',')
                .map(|s| s.trim().to_lowercase())
                .filter(|s| !s.is_empty())
                .collect(),
            mail_enforce_sender: env_or("NFHH_MAIL_ENFORCE_SENDER", "0") == "1",
            // 前後空白會讓比對永遠不成立 —— 那等於默默扣住所有信件
            mail_authserv_id: env_or("NFHH_MAIL_AUTHSERV_ID", "mx.cloudflare.net")
                .trim()
                .to_string(),
            dns_audit: env_or("NFHH_DNS_AUDIT", "/smartdns-data/audit.log"),
            cf_account: env_or("NFHH_CF_ACCOUNT", ""),
            cf_token: env_or("NFHH_CF_TOKEN", ""),
            resend_key: env_or("NFHH_RESEND_KEY", ""),
            mail_from: env_or("NFHH_MAIL_FROM", "share@example.com"),
            mail_domain: env_or("NFHH_MAIL_DOMAIN", "share.example.com"),
            invite_template: env_or("NFHH_INVITE_TEMPLATE", "ott-share-invitation"),
            // 夾範圍：0 天等於每次清光，0 列的 LIMIT 也一樣，而 -1 在 SQLite
            // 的 LIMIT 是「不限」—— 打錯一個字不該讓稽核整張消失或永不清理
            audit_keep_days: env_or("NFHH_AUDIT_KEEP_DAYS", "90")
                .parse()
                .unwrap_or(90)
                .clamp(1, 3650),
            audit_max_rows: env_or("NFHH_AUDIT_MAX_ROWS", "20000")
                .parse()
                .unwrap_or(20000)
                .clamp(100, 1_000_000),
        })
    }
}

/// 查詢視窗保留多久。比續期檢查的間隔長，否則檢查跑到時視窗已經空了，
/// 還在用的網路會被誤判成閒置。
const DNS_WINDOW_SECS: i64 = 1800;
/// 畫面上「近 5 分鐘 N 筆查詢」的區間。
const DNS_RECENT_SECS: i64 = 300;

struct AppState {
    db: db::Db,
    webauthn: Webauthn,
    cfg: Config,
    mailer: mailer::Mailer,
    cf: cloudflare::Cloudflare,
    push: push::Push,
    /// smartdns 查詢的記憶體滾動視窗，由背景 tail 任務餵。
    dns: Arc<dnslog::Window>,
    /// 不需要登入的那幾支端點共用的限流器。沒有帳號可擋，只剩來源 IP 可數。
    join_limiter: ratelimit::Limiter,
}

type Shared = Arc<AppState>;

// ── 錯誤處理 ──────────────────────────────────────────

struct AppError(anyhow::Error);

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        tracing::warn!("請求失敗: {:#}", self.0);
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": self.0.to_string() })),
        )
            .into_response()
    }
}

impl<E: Into<anyhow::Error>> From<E> for AppError {
    fn from(e: E) -> Self {
        Self(e.into())
    }
}

type ApiResult<T> = std::result::Result<T, AppError>;

// ── 取得真實用戶端 IP ─────────────────────────────────

/// 只信任 `CF-Connecting-IP`，刻意不讀 `X-Forwarded-For`（見 DECISIONS.md）。
fn client_ip(headers: &HeaderMap) -> Option<String> {
    headers
        .get("cf-connecting-ip")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<IpAddr>().ok())
        .map(|ip| ip.to_string())
}

// ── 工作階段輔助 ──────────────────────────────────────

const S_USER: &str = "uid";
const S_NAME: &str = "uname";
const S_REG: &str = "reg_state";
const S_REG_USER: &str = "reg_user";
const S_AUTH: &str = "auth_state";

/// 信箱＋passkey 登入的目標。跟註冊的 `S_REG_USER` 分開 ——
/// 共用同一把鍵曾讓「啟動登入」覆寫「註冊目標」，member 的新 Passkey
/// 就被寫進 admin 的資料列。
const S_LOGIN_USER: &str = "login_user";

/// 這個 session 剛證明過擁有哪個信箱。`register_start` 除了查全域的
/// `email_otp.verified_at`，還要求證明是**同一個瀏覽器**做的。
const S_EMAIL_PROOF: &str = "email_proof";

/// 任一認證流程開始時，先把其他流程留下的狀態全部清掉。
/// 登入與註冊各自是獨立的狀態機，殘留的鍵會讓 finish 讀到另一條流程的目標。
/// 清不掉就整個請求失敗 —— 帶著殘留狀態繼續，正是這個弱點的成因。
/// 刻意不清的是 `S_USER`／`S_NAME`（登入身分，加備援金鑰本來就要登入著）
/// 與 `S_EMAIL_PROOF`（信箱證明，`register_start` 在清除之後才讀它）——
/// 別把它們加進這個迴圈。
async fn clear_auth_flows(session: &Session) -> Result<()> {
    for key in [S_REG, S_REG_USER, S_AUTH, S_LOGIN_USER, S_DISC] {
        session.remove::<serde_json::Value>(key).await?;
    }
    Ok(())
}

/// 註冊完成前的最後一道不變量：已登入的人只能替**自己**加備援金鑰；
/// 建立新帳號的流程則不該有人登入著。
fn check_registration_owner(logged_in: Option<&str>, p: &PendingReg) -> Result<()> {
    match (logged_in, p.is_new) {
        (Some(uid), false) if uid == p.user_id => Ok(()),
        (Some(_), false) => anyhow::bail!("註冊目標與目前登入的帳號不符，請重新開始"),
        (Some(_), true) => anyhow::bail!("已登入的帳號不能建立新帳號，請先登出"),
        (None, false) => anyhow::bail!("加註備援 Passkey 需要先登入"),
        (None, true) => Ok(()),
    }
}

async fn current_user(session: &Session) -> Option<(String, String)> {
    let id: Option<String> = session.get(S_USER).await.ok().flatten();
    let name: Option<String> = session.get(S_NAME).await.ok().flatten();
    id.zip(name)
}

/// 每個需要登入的動作都走這裡，**每次都回 DB 確認帳號還在**。
///
/// ⚠️ 光看 session 不夠：它存在記憶體，帳號刪掉之後那份 session 還活著，
/// 被移除的成員仍能授權 IP、看驗證碼，直到容器重啟。
/// 釘在 `a_deleted_member_loses_access_immediately`。
async fn require_user(st: &Shared, session: &Session) -> ApiResult<(String, String)> {
    let (id, name) = current_user(session)
        .await
        .context("尚未登入")
        .map_err(AppError::from)?;
    if db::get_user(&st.db, &id)?.is_none() {
        return Err(AppError(anyhow::anyhow!("帳號已不存在")));
    }
    Ok((id, name))
}

/// 每次從 DB 重讀角色，讓降權即時生效。
async fn require_admin(st: &Shared, session: &Session) -> ApiResult<db::User> {
    let (uid, _) = require_user(st, session).await?;
    let user = db::get_user(&st.db, &uid)?.context("帳號已不存在")?;
    if !user.is_admin() {
        return Err(AppError(anyhow::anyhow!("需要管理員權限")));
    }
    Ok(user)
}

// ── 加入（Email 驗證碼）────────────────────────────────
//
// 註冊入口從「邀請連結」改成「admin 先登記 Email」。差別不在便利性，
// 而在憑據綁的是**信箱**還是一段可轉傳的字串：連結被轉發出去等於任何人
// 都能建帳號；登記的位址只有本人收得到碼。
//
// 這組碼只證明信箱是本人的，通過後仍要建 Passkey 才算有帳號。
// 完整的威脅模型見 otp.rs。

/// RFC 5321 的位址上限。再長的不是信箱，是想撐大資料庫的人。
const MAX_EMAIL_LEN: usize = 254;
/// 白名單標籤、裝置名稱等可顯示文字的上限。
const MAX_LABEL_LEN: usize = 128;
/// 不需要登入的端點（join/start、join/verify、join/invite，以及 register/start
/// 的未登入分支）共用的限流：每個來源 IP 每 10 分鐘 30 次、全域 200 次。
/// 名字沿用 join，計數是共用的。
///
/// 每 IP 30 次是給「整家人躲在同一個 NAT 後面同時開通」留的空間：寄碼、
/// 打錯幾次、重寄，一個人就可能用掉五六次。
const JOIN_LIMIT_WINDOW_SECS: i64 = 600;
const JOIN_LIMIT_PER_IP: u32 = 30;
const JOIN_LIMIT_GLOBAL: u32 = 200;

fn valid_email(s: &str) -> bool {
    s.len() <= MAX_EMAIL_LEN && s.contains('@') && !s.contains(char::is_whitespace)
}

/// 不需要登入的端點共用這道門。四支都會在失敗時寫一列稽核，而稽核有列數
/// 上限 —— 只擋其中一支，洪水換個門牌就能把真正的稽核軌跡整批擠掉。
///
/// 要在任何 DB 存取之前呼叫，被擋掉的請求才是真的什麼都沒碰到。
fn throttle_public(st: &Shared, ip: Option<&str>) -> ApiResult<()> {
    if !st.join_limiter.allow(ip.unwrap_or("?"), db::now()) {
        return Err(AppError(anyhow::anyhow!("請求太頻繁，請稍後再試")));
    }
    Ok(())
}

/// 可顯示文字的長度檢查。傳進來的要是**真的會存下去**的那個值。
fn check_label_len(label: Option<&str>) -> ApiResult<()> {
    if label.is_some_and(|l| l.chars().count() > MAX_LABEL_LEN) {
        return Err(AppError(anyhow::anyhow!("名稱最多 {MAX_LABEL_LEN} 個字")));
    }
    Ok(())
}

#[derive(Deserialize)]
struct EmailReq {
    email: String,
}

#[derive(Deserialize)]
struct VerifyReq {
    email: String,
    code: String,
}

#[derive(Deserialize)]
struct InviteTokenReq {
    token: String,
}

/// 兌換成功後畫面要顯示的東西：這條連結屬於誰、註冊完會拿到哪些平台。
/// 平台給代號，顯示名與顏色前端從 `/api/status` 的 platforms 對。
#[derive(Debug, Serialize)]
struct InviteOpened {
    email: String,
    platforms: Vec<String>,
}

#[derive(Debug, Serialize)]
struct JoinRes {
    /// 冷卻中剩餘秒數。1o 的「重新寄送（0:42）」倒數用這個值。
    cooldown: i64,
}

/// 寄出一組驗證碼。
///
/// 位址沒被登記時直接說「沒有被邀請」（設計 1k 就是這樣寫的）。
/// 這會洩漏「某個位址有沒有被登記」，但這是個封閉的家用系統 ——
/// 拿含糊訊息換取一點點防列舉，代價是家人打錯字時完全不知道發生什麼事。
async fn join_start(
    State(st): State<Shared>,
    headers: HeaderMap,
    Json(req): Json<EmailReq>,
) -> ApiResult<Json<JoinRes>> {
    let ip = client_ip(&headers);
    let email = req.email.trim().to_lowercase();
    if !valid_email(&email) {
        return Err(AppError(anyhow::anyhow!("請輸入完整的 Email 位址")));
    }
    throttle_public(&st, ip.as_deref())?;
    if !st.mailer.enabled() {
        return Err(AppError(anyhow::anyhow!("尚未設定寄信服務，請聯絡管理員")));
    }
    if db::find_user_by_email(&st.db, &email)?.is_some() {
        return Err(AppError(anyhow::anyhow!(
            "這個位址已經有帳號了，請直接用 Passkey 登入"
        )));
    }
    if !db::is_email_invited(&st.db, &email)? {
        db::audit(&st.db, None, "join_not_invited", Some(&email), ip.as_deref());
        return Err(AppError(anyhow::anyhow!(
            "這個位址沒有被邀請。請確認拼字與管理員登記的完全一致。"
        )));
    }

    // 冷卻擋的是「拿這支端點當寄信機去洗別人的信箱」
    let cooldown = db::otp_cooldown(&st.db, &email, otp::RESEND_COOLDOWN_SECS)?;
    if cooldown > 0 {
        return Err(AppError(anyhow::anyhow!("請等 {cooldown} 秒後再重新寄送")));
    }

    let code = otp::generate();
    // 先寫 DB 再寄信：反過來的話，寄成功但寫失敗會讓使用者拿著一組
    // 系統不認得的碼，而且冷卻也沒生效，可以無限重按。
    db::put_otp(&st.db, &email, &otp::hash(&st.db, &email, &code)?, otp::TTL_SECS)?;
    st.mailer.send_code(&email, &code, otp::TTL_SECS / 60).await?;

    db::audit(&st.db, None, "join_code_sent", Some(&email), ip.as_deref());
    Ok(Json(JoinRes { cooldown: otp::RESEND_COOLDOWN_SECS }))
}

/// 核對驗證碼。通過後才允許進入 Passkey 註冊。
async fn join_verify(
    State(st): State<Shared>,
    session: Session,
    headers: HeaderMap,
    Json(req): Json<VerifyReq>,
) -> ApiResult<Json<serde_json::Value>> {
    let ip = client_ip(&headers);
    throttle_public(&st, ip.as_deref())?;
    let email = req.email.trim().to_lowercase();
    let code = otp::normalize(&req.code);

    let hash = otp::hash(&st.db, &email, &code)?;
    match db::check_otp(&st.db, &email, &hash, otp::MAX_ATTEMPTS)? {
        db::OtpCheck::Ok => {
            // 證明綁在這個 session 上：全域的 verified_at 只說「有人驗過」，
            // 說不出是誰的瀏覽器驗的。
            session.insert(S_EMAIL_PROOF, &email).await?;
            db::audit(&st.db, None, "join_code_ok", Some(&email), ip.as_deref());
            Ok(Json(serde_json::json!({ "ok": true })))
        }
        db::OtpCheck::Wrong => Err(AppError(anyhow::anyhow!("驗證碼不正確"))),
        db::OtpCheck::Expired => Err(AppError(anyhow::anyhow!(
            "驗證碼已過期，請重新寄送一組"
        ))),
        db::OtpCheck::TooManyAttempts => {
            db::audit(&st.db, None, "join_code_locked", Some(&email), ip.as_deref());
            Err(AppError(anyhow::anyhow!(
                "錯誤次數過多，請重新寄送一組新的驗證碼"
            )))
        }
    }
}

/// 兌換邀請函裡的連結。
///
/// 做的事情跟「寄碼 + 輸入正確」一模一樣：把這個信箱標成剛剛證實過，
/// 接下來走的是同一條 Passkey 註冊路徑（見 `register_start`）。連結只寄得到
/// 那個位址，收得到它跟收得到寄去那裡的碼證明的是同一件事。
///
/// 位址由連結決定，不由呼叫端給 —— 前端沒有機會拿別人的權杖去換自己的信箱。
async fn join_invite(
    State(st): State<Shared>,
    session: Session,
    headers: HeaderMap,
    Json(req): Json<InviteTokenReq>,
) -> ApiResult<Json<InviteOpened>> {
    let ip = client_ip(&headers);
    throttle_public(&st, ip.as_deref())?;
    let hash = invite::hash(&st.db, &req.token)?;

    // 撤銷過、已註冊過、或根本不存在的權杖，在這裡都是同一種結果。
    // 訊息不分辨是哪一種：邀請連結會被轉傳，拿到它的未必是被邀請的人。
    let Some(row) = db::invited_email_by_token(&st.db, &hash)? else {
        db::audit(&st.db, None, "invite_link_bad", None, ip.as_deref());
        return Err(AppError(anyhow::anyhow!(
            "這個邀請連結無效或已經用過了。可以改用「用 Email 加入」，或請管理員重發一封。"
        )));
    };
    if db::find_user_by_email(&st.db, &row.email)?.is_some() {
        return Err(AppError(anyhow::anyhow!(
            "這個位址已經有帳號了，請直接用 Passkey 登入"
        )));
    }

    db::mark_email_verified(&st.db, &row.email)?;
    // 跟驗證碼那條路一樣：兌換連結的是哪個瀏覽器，證明就記在哪個 session。
    session.insert(S_EMAIL_PROOF, &row.email).await?;
    db::audit(&st.db, None, "invite_link_opened", Some(&row.email), ip.as_deref());
    Ok(Json(InviteOpened { email: row.email, platforms: row.platforms }))
}

// ── 註冊（Passkey）────────────────────────────────────

#[derive(Deserialize)]
struct RegisterStart {
    /// 新帳號用的信箱。加註備援 passkey 時省略 —— 那時用的是目前登入的帳號。
    email: Option<String>,
    /// 建立**第一個**帳號用的一次性碼（見容器啟動日誌）
    bootstrap_token: Option<String>,
    /// 這把 passkey 的名字，例如「iPhone 15」。之後在帳號頁用它辨認裝置。
    nickname: Option<String>,
}

/// 註冊流程在 session 裡帶的狀態
#[derive(Serialize, Deserialize)]
struct PendingReg {
    user_id: String,
    /// WebAuthn 的 user handle。新帳號建立時等於 email，之後**永不變動** ——
    /// 改了會讓已註冊的 passkey 對不上。顯示身分請看 email 欄位。
    username: String,
    email: Option<String>,
    is_new: bool,
    role: String,
    /// 要在 finish 階段才消耗的憑據。
    bootstrap_token: Option<String>,
    nickname: Option<String>,
}

impl PendingReg {
    /// 稽核與畫面上稱呼這個人的方式。
    fn label(&self) -> &str {
        self.email.as_deref().unwrap_or(&self.username)
    }
}

async fn register_start(
    State(st): State<Shared>,
    session: Session,
    headers: HeaderMap,
    Json(req): Json<RegisterStart>,
) -> ApiResult<Json<CreationChallengeResponse>> {
    let ip = client_ip(&headers);
    clear_auth_flows(&session).await?;
    let logged_in = current_user(&session).await;
    let email = req.email.as_deref().map(|e| e.trim().to_lowercase());
    let nickname = req.nickname.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    check_label_len(nickname.as_deref())?;

    // 三種合法情境，其餘一律拒絕（不開放自助註冊）：
    //   1. 已登入 → 在這台裝置加註備援 passkey
    //   2. 系統還沒有任何帳號 + 一次性碼 → 建立第一個帳號（admin）
    //   3. 登記過的信箱 + 剛通過 OTP → 家人建立自己的帳號（member）
    let pending = if let Some((uid, _)) = &logged_in {
        let u = db::get_user(&st.db, uid)?.context("帳號已不存在")?;
        PendingReg {
            user_id: u.id,
            username: u.username,
            email: u.email,
            is_new: false,
            role: u.role,
            bootstrap_token: None,
            nickname: nickname.clone(),
        }
    } else {
        // 只有這條分支不需要登入。已登入的人加註備援 passkey 不該跟公開
        // 流量搶同一份額度 —— 上面讀的 session 不碰 DB，這裡仍在任何
        // DB 存取之前。
        throttle_public(&st, ip.as_deref())?;
        let email = email.context("需要 Email 位址")?;
        if !valid_email(&email) {
            return Err(AppError(anyhow::anyhow!("請輸入完整的 Email 位址")));
        }
        if db::find_user_by_email(&st.db, &email)?.is_some() {
            return Err(AppError(anyhow::anyhow!("這個位址已經有帳號了")));
        }

        if db::user_count(&st.db)? == 0 {
            // 第一個帳號**刻意不要求 OTP**：面板還沒跑過，寄信服務未必
            // 已經設好，要求信件送達才能建第一個帳號是個死結。
            // 一次性碼只印在容器日誌裡，看得到它的人已經有主機權限。
            let token = req
                .bootstrap_token
                .as_deref()
                .context("需要一次性註冊碼（見容器啟動日誌）")?;
            if !db::peek_bootstrap(&st.db, token)? {
                db::audit(&st.db, None, "bootstrap_bad", None, ip.as_deref());
                return Err(AppError(anyhow::anyhow!("一次性註冊碼無效或已使用")));
            }
            PendingReg {
                user_id: Uuid::new_v4().to_string(),
                username: email.clone(),
                email: Some(email),
                is_new: true,
                role: "admin".into(),
                bootstrap_token: Some(token.to_string()),
                nickname: nickname.clone(),
            }
        } else {
            // 兩道關卡都要過：位址被登記過，而且**剛剛**證明過信箱是他的。
            if !db::is_email_invited(&st.db, &email)? {
                db::audit(&st.db, None, "register_not_invited", Some(&email), ip.as_deref());
                return Err(AppError(anyhow::anyhow!("這個位址沒有被邀請")));
            }
            // 全域旗標只證明「這個位址被驗過」，誰都能拿著它來註冊；
            // 再對一次 session 上的證明，才輪得到真正完成驗證的那個瀏覽器。
            let proven: Option<String> = session.get(S_EMAIL_PROOF).await?;
            if proven.as_deref() != Some(email.as_str())
                || !db::otp_recently_verified(&st.db, &email, otp::VERIFIED_WINDOW_SECS)?
            {
                return Err(AppError(anyhow::anyhow!(
                    "請先在這個瀏覽器完成信箱驗證，或重新寄送一組驗證碼"
                )));
            }
            PendingReg {
                user_id: Uuid::new_v4().to_string(),
                username: email.clone(),
                email: Some(email),
                is_new: true,
                role: "member".into(),
                bootstrap_token: None,
                nickname: nickname.clone(),
            }
        }
    };

    // 排除已註冊的憑證，避免同一把 passkey 在同一帳號重複註冊
    let exclude: Vec<CredentialID> = if pending.is_new {
        vec![]
    } else {
        db::credentials_for(&st.db, &pending.user_id)?
            .iter()
            .filter_map(|j| serde_json::from_str::<Passkey>(j).ok())
            .map(|p| p.cred_id().clone())
            .collect()
    };

    let uuid = Uuid::parse_str(&pending.user_id).unwrap_or_else(|_| Uuid::new_v4());
    let display = pending.label().to_string();
    let (ccr, reg_state) =
        st.webauthn
            .start_passkey_registration(uuid, &pending.username, &display, Some(exclude))?;

    session.insert(S_REG, &reg_state).await?;
    session.insert(S_REG_USER, &pending).await?;
    Ok(Json(ccr))
}

async fn register_finish(
    State(st): State<Shared>,
    session: Session,
    headers: HeaderMap,
    Json(cred): Json<RegisterPublicKeyCredential>,
) -> ApiResult<Json<serde_json::Value>> {
    let reg_state: PasskeyRegistration = session
        .get(S_REG)
        .await?
        .context("註冊工作階段已失效，請重新開始")?;
    let p: PendingReg = session
        .get(S_REG_USER)
        .await?
        .context("註冊工作階段已失效，請重新開始")?;

    let logged_in = current_user(&session).await;
    check_registration_owner(logged_in.as_ref().map(|(id, _)| id.as_str()), &p)?;

    // 先驗 WebAuthn，通過了才消耗憑據
    let passkey = st.webauthn.finish_passkey_registration(&cred, &reg_state)?;

    if p.is_new {
        // 真正消耗，UPDATE ... WHERE 條件保證併發時只有一個成功
        if let Some(t) = &p.bootstrap_token {
            if !db::consume_bootstrap(&st.db, t)? {
                return Err(AppError(anyhow::anyhow!("一次性註冊碼已被使用")));
            }
        }
        // bootstrap 建立的第一個帳號沒有對應的登記紀錄，跳過這一段。
        //
        // 消耗要排在 create_user **之前**：它是「這個位址還能不能用」的
        // 閘門，過不了就不該有帳號被建出來。
        let granted = if let (Some(email), None) = (&p.email, &p.bootstrap_token) {
            let Some(platforms) = db::consume_invited_email(&st.db, email, &p.user_id)? else {
                return Err(AppError(anyhow::anyhow!("這個位址已經被用來註冊過了")));
            };
            platforms
        } else {
            Vec::new()
        };

        // 建立帳號並授予登記時選好的平台。順序封在 db 那支函式裡 ——
        // 授權有外鍵指向 users，寫反會被擋下（這裡曾經寫反過，而且用
        // `let _ =` 吞掉錯誤，於是平台靜默地一個都沒授權）。
        db::create_user_with_platforms(
            &st.db,
            &p.user_id,
            &p.username,
            p.label(),
            &p.role,
            p.email.as_deref(),
            &granted,
        )?;
        // 碼用掉就清掉，不留給下一次註冊嘗試
        if let Some(email) = &p.email {
            let _ = db::clear_otp(&st.db, email);
        }
    }

    let cred_id = base64_url(passkey.cred_id().as_ref());
    db::add_credential(
        &st.db,
        &cred_id,
        &p.user_id,
        &serde_json::to_string(&passkey)?,
        p.nickname.as_deref(),
    )?;

    // 三把鍵都清，而且清不掉要報錯：殘留的目標是下一次攻擊的材料。
    // 排在 `add_credential` **之後**：憑證寫失敗就整個請求失敗，這時清掉
    // 信箱證明只會讓人連重試都不行 —— 帳號建了、邀請消耗了、passkey 卻沒進去。
    // 信箱證明不分流程無條件清（沒有那把鍵不算錯），加備援金鑰那條路
    // 順手把殘留的證明一起帶走。
    session.remove::<PasskeyRegistration>(S_REG).await?;
    session.remove::<PendingReg>(S_REG_USER).await?;
    session.remove::<String>(S_EMAIL_PROOF).await?;
    db::audit(
        &st.db,
        Some(p.label()),
        if p.is_new { "user_created" } else { "passkey_added" },
        Some(&if p.is_new { format!("role={}", p.role) } else { cred_id }),
        client_ip(&headers).as_deref(),
    );

    // 第一次註冊完直接視為登入
    if p.is_new {
        session.insert(S_USER, &p.user_id).await?;
        session.insert(S_NAME, p.label()).await?;
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

// ── 登入 ──────────────────────────────────────────────
//
// 兩條路，因為它們涵蓋的憑證不一樣：
//
//   1. **可探索登入**（不必輸入信箱）—— 裝置自己知道有哪些帳號。
//      這是設計 1j 想要的樣子。
//   2. **信箱 ＋ passkey** —— 退路。
//
// 為什麼退路不能拿掉：`start_passkey_registration` 送出的是
// `residentKey: "discouraged"`（webauthn-rs 0.5 寫死，高階 API 沒有
// 非 attested 的 resident key 入口）。iOS／Android／Chrome 的密碼管理器
// 實務上仍會存成可探索的，所以路徑 1 對它們有效；但硬體金鑰或設定較
// 嚴格的認證器可能不會，那些帳號只剩路徑 2 進得來。
//
// → 在能確定所有人的憑證都可探索之前，別把路徑 2 拿掉。

const S_DISC: &str = "disc_state";

/// 開一個不指定使用者的挑戰：`allowCredentials` 是空的，
/// 由裝置決定要用哪一把。
async fn login_any_start(
    State(st): State<Shared>,
    session: Session,
) -> ApiResult<Json<RequestChallengeResponse>> {
    clear_auth_flows(&session).await?;
    let (rcr, disc) = st.webauthn.start_discoverable_authentication()?;
    session.insert(S_DISC, &disc).await?;
    Ok(Json(rcr))
}

async fn login_any_finish(
    State(st): State<Shared>,
    session: Session,
    headers: HeaderMap,
    Json(cred): Json<PublicKeyCredential>,
) -> ApiResult<Json<serde_json::Value>> {
    let disc: DiscoverableAuthentication = session
        .get(S_DISC)
        .await?
        .context("登入工作階段已失效，請重新開始")?;

    // 回應裡帶著 user handle（我們註冊時放進去的 users.id），
    // 先用它找出是誰，才能取出那個人的憑證來驗簽。
    let (uuid, _) = st.webauthn.identify_discoverable_authentication(&cred)?;
    let user = db::get_user(&st.db, &uuid.to_string())?
        .context("這把 Passkey 對應的帳號已不存在")?;

    let keys: Vec<DiscoverableKey> = db::credentials_for(&st.db, &user.id)?
        .iter()
        .filter_map(|j| serde_json::from_str::<Passkey>(j).ok())
        .map(|p| DiscoverableKey::from(&p))
        .collect();

    let result = st
        .webauthn
        .finish_discoverable_authentication(&cred, disc, &keys)?;

    let cred_id = base64_url(result.cred_id().as_ref());
    let _ = db::touch_credential(&st.db, &cred_id);
    session.remove::<DiscoverableAuthentication>(S_DISC).await?;

    let label = user.label().to_string();
    session.insert(S_USER, &user.id).await?;
    session.insert(S_NAME, &label).await?;
    db::audit(&st.db, Some(&label), "login", Some("passkey"), client_ip(&headers).as_deref());
    Ok(Json(serde_json::json!({ "ok": true, "username": label })))
}

async fn login_start(
    State(st): State<Shared>,
    session: Session,
    Json(req): Json<EmailReq>,
) -> ApiResult<Json<RequestChallengeResponse>> {
    clear_auth_flows(&session).await?;
    let ident = req.email.trim().to_lowercase();
    let user = db::find_user_by_email(&st.db, &ident)?.context("查無此帳號")?;

    let passkeys: Vec<Passkey> = db::credentials_for(&st.db, &user.id)?
        .iter()
        .filter_map(|j| serde_json::from_str(j).ok())
        .collect();
    if passkeys.is_empty() {
        return Err(AppError(anyhow::anyhow!("此帳號沒有已註冊的 passkey")));
    }

    let (rcr, auth_state) = st.webauthn.start_passkey_authentication(&passkeys)?;
    session.insert(S_AUTH, &auth_state).await?;
    session
        .insert(
            S_LOGIN_USER,
            &PendingReg {
                user_id: user.id,
                username: user.username,
                email: user.email,
                is_new: false,
                role: user.role,
                bootstrap_token: None,
                nickname: None,
            },
        )
        .await?;
    Ok(Json(rcr))
}

async fn login_finish(
    State(st): State<Shared>,
    session: Session,
    headers: HeaderMap,
    Json(cred): Json<PublicKeyCredential>,
) -> ApiResult<Json<serde_json::Value>> {
    let auth_state: PasskeyAuthentication = session
        .get(S_AUTH)
        .await?
        .context("登入工作階段已失效，請重新開始")?;
    let p: PendingReg = session
        .get(S_LOGIN_USER)
        .await?
        .context("登入工作階段已失效，請重新開始")?;

    let result = st.webauthn.finish_passkey_authentication(&cred, &auth_state)?;
    let cred_id = base64_url(result.cred_id().as_ref());
    let _ = db::touch_credential(&st.db, &cred_id);
    session.remove::<PasskeyAuthentication>(S_AUTH).await?;
    session.remove::<PendingReg>(S_LOGIN_USER).await?;

    session.insert(S_USER, &p.user_id).await?;
    session.insert(S_NAME, p.label()).await?;
    db::audit(&st.db, Some(p.label()), "login", None, client_ip(&headers).as_deref());
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn logout(session: Session) -> ApiResult<Json<serde_json::Value>> {
    session.flush().await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

// ── 狀態 ──────────────────────────────────────────────

/// 白名單條目 ＋ 它的活躍度。
///
/// 只帶彙總數字，**不帶逐筆網域** —— 那份要另外打 `/api/allow/{ip}/queries`，
/// 而且只有該條目的擁有者拿得到。列表是 admin 也看得到的東西，
/// 家人查了哪些網域不該順手夾在裡面送出去。
#[derive(Serialize)]
struct AllowRow {
    #[serde(flatten)]
    entry: db::AllowEntry,
    queries: dnslog::Stats,
    /// 這條是不是呼叫者自己加的。決定前端要不要顯示「展開查詢」。
    mine: bool,
}

#[derive(Serialize)]
struct Status {
    logged_in: bool,
    username: Option<String>,
    my_ip: Option<String>,
    my_ip_allowed: bool,
    wan_ip: Option<String>,
    /// 本機的 LAN IP，**只在呼叫者看起來位於本層網路時**才給。
    ///
    /// 面板走 Cloudflare Tunnel，看到的一律是公網位址，沒辦法直接得知
    /// 對方在哪個網段。但同一個 NAT 後面的裝置出口 IP 會跟本機一樣 ——
    /// `my_ip == wan_ip` 就等於「在本層」。
    ///
    /// 這件事有實際後果：本層裝置若把 DNS 填成公網位址，封包會出去再
    /// 繞回來，而 TP-Link 未必支援 NAT hairpin（見 DECISIONS.md）。
    lan_ip: Option<String>,
    /// 一般成員只看得到自己加的；admin 看得到全部（設計 1d / 1l）。
    entries: Vec<AllowRow>,
    /// 每人額度上限，以及呼叫者自己目前用掉幾條。全域上限已在 v6 拿掉。
    max_per_user: i64,
    my_entry_count: i64,
    default_ttl_days: i64,
    /// 目前啟用中的平台，以及呼叫者被授權了哪些。
    platforms: Vec<platforms::Platform>,
    my_platforms: Vec<String>,
    needs_bootstrap: bool,
    /// 只有一把 passkey 時前端提示註冊備援
    passkey_count: i64,
    is_admin: bool,
    dot_host: String,
    /// DoT 是否已啟用，未啟用則不提供 iOS 描述檔
    dot_ready: bool,
    /// 信件接收端點是否已設定密鑰，未設則前端不顯示該區塊
    mail_enabled: bool,
    /// 「用 Email 加入」是否可用（要有寄信金鑰）
    join_enabled: bool,
    /// 轉發收件人的驗證狀態查得到嗎。false 時前端顯示「未查詢」，
    /// 不可顯示成「尚未驗證」—— 那是兩件不同的事。
    cf_enabled: bool,
}

/// 前端可以帶 `?ip=` 覆蓋「我現在在哪個網路」。
///
/// 面板走 Cloudflare Tunnel，連線來源是**開面板這台裝置**連到 Cloudflare
/// 用的位址；手機開著 IPv6 時那是一個 /128，拿它當「這個網路的出口」是錯的
/// （理由見 `web/src/lib/ip.js`）。所以由前端在瀏覽器端問出該網路的公網
/// IPv4 再帶進來。
#[derive(Deserialize)]
struct StatusQuery {
    ip: Option<String>,
}

async fn status(
    State(st): State<Shared>,
    session: Session,
    headers: HeaderMap,
    Query(q): Query<StatusQuery>,
) -> ApiResult<Json<Status>> {
    let user = current_user(&session).await;

    // ⚠️ 只在登入後採信 `?ip=` —— 否則這支端點會變成「某個 IP 在不在
    //    白名單裡」的探測器，而它是不需要登入就能打的。
    let my_ip = q
        .ip
        .as_deref()
        .filter(|_| user.is_some())
        .and_then(|s| s.parse::<IpAddr>().ok())
        .filter(is_public)
        .map(|ip| ip.to_string())
        .or_else(|| client_ip(&headers));
    let _ = db::purge_expired(&st.db);

    let wan_ip = read_wan_ip(&st.cfg.dynamic_conf);
    let all = db::list_allow(&st.db).unwrap_or_default();

    // 「我現在這個網路通不通」要看**全部**條目 —— 別人授權過的網路
    // 對我一樣有效，不能因為列表被過濾掉就說沒授權。
    let my_ip_allowed = my_ip.as_ref().is_some_and(|ip| all.iter().any(|e| &e.ip == ip));

    // 只有登入後才揭露白名單內容
    let (passkey_count, is_admin, my_platforms) = match &user {
        Some((uid, _)) => (
            db::credential_count(&st.db, uid).unwrap_or(0),
            db::get_user(&st.db, uid)?.map(|u| u.is_admin()).unwrap_or(false),
            db::platforms_for(&st.db, uid).unwrap_or_default(),
        ),
        None => (0, false, vec![]),
    };

    let now = db::now();
    let me = user.as_ref().map(|(_, n)| n.as_str());
    let entries: Vec<AllowRow> = match me {
        None => vec![],
        Some(name) => all
            .into_iter()
            .filter(|e| is_admin || e.added_by.as_deref() == Some(name))
            .map(|e| AllowRow {
                queries: st.dns.stats(&e.ip, DNS_RECENT_SECS, now),
                mine: e.added_by.as_deref() == Some(name),
                entry: e,
            })
            .collect(),
    };
    let my_entry_count = me.map_or(0, |n| db::allow_count_by(&st.db, n).unwrap_or(0));

    // 本層裝置（出口 IP 跟本機相同）要看 LAN IP，其他樓層看 WAN IP
    let lan_ip = match (&my_ip, &wan_ip) {
        (Some(mine), Some(wan)) if mine == wan => read_ip_header(&st.cfg.dynamic_conf, "LAN_IP"),
        _ => None,
    };

    Ok(Json(Status {
        logged_in: user.is_some(),
        username: user.map(|(_, n)| n),
        my_ip,
        my_ip_allowed,
        wan_ip,
        lan_ip,
        entries,
        max_per_user: st.cfg.max_per_user,
        my_entry_count,
        default_ttl_days: st.cfg.default_ttl_days,
        platforms: platforms::list(&st.cfg.domain_set_dir),
        my_platforms,
        needs_bootstrap: db::user_count(&st.db).unwrap_or(0) == 0,
        passkey_count,
        is_admin,
        dot_host: st.cfg.dot_host.clone(),
        dot_ready: dot_ready(&st.cfg.dot_conf),
        mail_enabled: !st.cfg.mail_secret.is_empty(),
        join_enabled: st.mailer.enabled(),
        cf_enabled: st.cf.enabled(),
    }))
}

/// 從 smartdns 正在用的設定檔讀出口 IP，而非即時查詢外部服務 ——
/// 面板要顯示的是 smartdns 實際在服務的值。
/// DoT 是否啟用同理，直接看設定檔有沒有 bind-tls。
fn dot_ready(path: &str) -> bool {
    std::fs::read_to_string(path)
        .map(|s| {
            s.lines()
                .any(|l| l.trim_start().starts_with("bind-tls"))
        })
        .unwrap_or(false)
}

/// 描述檔識別碼的前綴：把 RP ID 反寫成 reverse-DNS。
///
/// iOS 以 `PayloadIdentifier` + UUID 判斷是不是同一份描述檔，所以這個值**必須
/// 穩定** —— 一變，已安裝的人不會被取代，而是多出第二份描述檔搶 DNS 設定。
/// 拿 `rp_id` 當來源正是因為它不能事後改（改了所有 passkey 一起作廢），
/// 跟著它走等於跟著這個部署走，而且不必在程式裡寫死任何人的網域。
fn profile_prefix(rp_id: &str) -> String {
    let mut parts: Vec<&str> = rp_id
        .trim()
        .split('.')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    parts.reverse();
    if parts.is_empty() {
        // rp_id 不該是空的，但識別碼寧可退回一個固定值也不要變成空字串 ——
        // 空的 PayloadIdentifier 會讓 iOS 直接拒收整份描述檔。
        return "nfhh".to_string();
    }
    format!("{}.nfhh", parts.join(".").to_lowercase())
}

/// 產生 iOS / iPadOS 的 DNS 描述檔（.mobileconfig）。iOS 沒有可直接填主機名的欄位。
///
/// 刻意不填 ServerAddresses：留空時 iOS 會用網路提供的 DNS 解析 ServerName，
/// 填了等於又把 IP 寫死。
async fn dns_profile(
    State(st): State<Shared>,
    session: Session,
) -> ApiResult<Response> {
    require_user(&st, &session).await?;
    let host = &st.cfg.dot_host;
    let prefix = profile_prefix(&st.cfg.rp_id);

    // UUID 固定：iOS 以 PayloadIdentifier + UUID 判斷是否為同一份描述檔
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>PayloadContent</key>
  <array>
    <dict>
      <key>PayloadType</key><string>com.apple.dnsSettings.managed</string>
      <key>PayloadIdentifier</key><string>{prefix}.dns.payload</string>
      <key>PayloadUUID</key><string>7f3a1c62-8d54-4b19-9a70-2e5c8f10d3b1</string>
      <key>PayloadVersion</key><integer>1</integer>
      <key>PayloadDisplayName</key><string>加密 DNS（DoT）</string>
      <key>DNSSettings</key>
      <dict>
        <key>DNSProtocol</key><string>TLS</string>
        <key>ServerName</key><string>{host}</string>
      </dict>
    </dict>
  </array>
  <key>PayloadType</key><string>Configuration</string>
  <key>PayloadIdentifier</key><string>{prefix}.dns</string>
  <key>PayloadUUID</key><string>c81e5d04-6b2f-4a87-8c33-9f1d70a4e256</string>
  <key>PayloadVersion</key><integer>1</integer>
  <key>PayloadDisplayName</key><string>OTT Household DNS</string>
  <key>PayloadDescription</key><string>將本裝置的 DNS 指向 {host}（加密傳輸）。可隨時於「設定 &gt; 一般 &gt; VPN 與裝置管理」移除。</string>
  <key>PayloadOrganization</key><string>OTT Household</string>
  <key>PayloadRemovalDisallowed</key><false/>
</dict>
</plist>
"#
    );

    Ok((
        [
            (
                axum::http::header::CONTENT_TYPE,
                "application/x-apple-aspen-config",
            ),
            (
                axum::http::header::CONTENT_DISPOSITION,
                "attachment; filename=\"nfhh-dns.mobileconfig\"",
            ),
        ],
        xml,
    )
        .into_response())
}

fn read_wan_ip(path: &str) -> Option<String> {
    read_ip_header(path, "WAN_IP")
}

/// 從 apply-config.sh 產生的設定檔標頭讀出 IP。
/// 讀的是 smartdns **正在服務的值**，而不是即時去查外部服務 ——
/// 面板要顯示的是實際生效的設定，不是「現在對外看起來是什麼」。
fn read_ip_header(path: &str, key: &str) -> Option<String> {
    let s = std::fs::read_to_string(path).ok()?;
    let prefix = format!("# {key} = ");
    s.lines()
        .find_map(|l| l.strip_prefix(prefix.as_str()))
        .map(|v| v.split_whitespace().next().unwrap_or("").to_string())
        .filter(|v| !v.is_empty())
}

// ── 白名單 ────────────────────────────────────────────

#[derive(Deserialize)]
struct AllowReq {
    /// 面板一律帶著它 —— 送來的是前端在瀏覽器端問出的「這個網路的公網
    /// IPv4」，不是連線來源（見 `StatusQuery` 與 `web/src/lib/ip.js`）。
    /// 省略則退回呼叫端的連線位址。
    ///
    /// 誰授權了哪個 IP、從哪連進來的，兩個都寫進稽核 —— 連線位址記在
    /// `client_ip` 欄，跟這裡的值不一樣是正常的。
    ip: Option<String>,
    label: Option<String>,
    ttl_days: Option<i64>,
}

async fn allow_add(
    State(st): State<Shared>,
    session: Session,
    headers: HeaderMap,
    Json(req): Json<AllowReq>,
) -> ApiResult<Json<serde_json::Value>> {
    let (uid, username) = require_user(&st, &session).await?;
    let caller_ip = client_ip(&headers);

    // 存下去的就是 req.label 本身（沒有 trim），檢查的也是它
    check_label_len(req.label.as_deref())?;

    let ip_str = req
        .ip
        .clone()
        .or_else(|| caller_ip.clone())
        .context("取不到來源 IP，請手動指定")?;
    let ip: IpAddr = ip_str.parse().context("IP 格式不正確")?;

    // 白名單比對的是公網來源 IP，私有位址不收
    if !is_public(&ip) {
        return Err(AppError(anyhow::anyhow!(
            "{ip} 不是公網位址，加進白名單不會有效果"
        )));
    }

    db::purge_expired(&st.db)?;

    let me = db::get_user(&st.db, &uid)?.context("帳號已不存在")?;

    // 額度只在「全新的 IP」時扣：已在自己名下的是延長；別人的會在下面被
    // SQL 拒絕，也不該先扣掉呼叫者的額度。
    let exists = db::list_allow(&st.db)?.iter().any(|e| e.ip == ip_str);
    if !exists {
        let mine = db::allow_count_by(&st.db, &username)?;
        if mine >= st.cfg.max_per_user {
            return Err(AppError(anyhow::anyhow!(
                "你的額度已滿（{mine} / {}），請先移除不用的網路",
                st.cfg.max_per_user
            )));
        }
    }

    let ttl_days = req.ttl_days.unwrap_or(st.cfg.default_ttl_days).clamp(1, 30);
    let expires_at = db::now() + ttl_days * 86400;
    if !db::upsert_allow_owned(
        &st.db,
        &ip_str,
        req.label.as_deref(),
        &username,
        expires_at,
        ttl_days,
        me.is_admin(),
    )? {
        return Err(AppError(anyhow::anyhow!(
            "{ip_str} 不是你新增的，只有新增者或管理員能修改"
        )));
    }

    let n = nft::sync(&st.db, &st.cfg.clients_nft)?;
    db::audit(
        &st.db,
        Some(&username),
        "allow_add",
        Some(&format!("{ip_str} ttl={ttl_days}d")),
        caller_ip.as_deref(),
    );
    Ok(Json(serde_json::json!({ "ok": true, "ip": ip_str, "active": n })))
}

async fn allow_remove(
    State(st): State<Shared>,
    session: Session,
    headers: HeaderMap,
    Path(ip): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let (uid, username) = require_user(&st, &session).await?;
    let me = db::get_user(&st.db, &uid)?.context("帳號已不存在")?;

    // 一般成員只能移除自己加的
    if !me.is_admin() {
        let owner = db::list_allow(&st.db)?
            .into_iter()
            .find(|e| e.ip == ip)
            .and_then(|e| e.added_by);
        if owner.as_deref() != Some(username.as_str()) {
            return Err(AppError(anyhow::anyhow!("只能移除自己新增的項目")));
        }
    }

    let removed = db::remove_allow(&st.db, &ip)?;
    let n = nft::sync(&st.db, &st.cfg.clients_nft)?;
    db::audit(
        &st.db,
        Some(&username),
        "allow_remove",
        Some(&ip),
        client_ip(&headers).as_deref(),
    );
    Ok(Json(serde_json::json!({ "ok": removed > 0, "active": n })))
}

#[derive(Deserialize)]
struct LabelReq {
    label: Option<String>,
}

/// 重新命名。純標記操作，**不動到期時間** ——
/// 改名不該偷偷幫這條續命（設計 1d 把改名和授權做成兩顆獨立按鈕）。
async fn allow_rename(
    State(st): State<Shared>,
    session: Session,
    headers: HeaderMap,
    Path(ip): Path<String>,
    Json(req): Json<LabelReq>,
) -> ApiResult<Json<serde_json::Value>> {
    let (uid, username) = require_user(&st, &session).await?;
    let me = db::get_user(&st.db, &uid)?.context("帳號已不存在")?;

    let owner = db::list_allow(&st.db)?
        .into_iter()
        .find(|e| e.ip == ip)
        .context("查無此白名單條目")?
        .added_by;
    if !me.is_admin() && owner.as_deref() != Some(username.as_str()) {
        return Err(AppError(anyhow::anyhow!("只能重新命名自己新增的項目")));
    }

    let label = req.label.as_deref().map(str::trim).filter(|s| !s.is_empty());
    check_label_len(label)?;
    db::rename_allow(&st.db, &ip, label)?;
    db::audit(
        &st.db,
        Some(&username),
        "allow_renamed",
        Some(&format!("{ip} → {}", label.unwrap_or("(清空)"))),
        client_ip(&headers).as_deref(),
    );
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn audit_list(
    State(st): State<Shared>,
    session: Session,
) -> ApiResult<Json<Vec<db::AuditRow>>> {
    require_user(&st, &session).await?;
    Ok(Json(db::recent_audit(&st.db, 100)?))
}

// ── 驗證碼信件 ────────────────────────────────────────

/// ingest 的錯誤要讓 Worker 分得出「拒收」與「面板掛了」：前者不該退回
/// 未過濾的 FORWARD_MAP，後者才該。一般 `AppError` 一律回 400，分不出來。
///
/// 對應到 Worker 的三種處置：
///   - 5xx（含未啟用）→ 面板不可用，Worker 走 FORWARD_MAP
///   - 401／422 → 面板拒收，Worker 只轉 FALLBACK_TO
#[derive(Debug)]
enum IngestError {
    /// 端點未啟用（`NFHH_MAIL_SECRET` 為空）—— 面板刻意不參與，
    /// Worker 應照 FORWARD_MAP 轉發
    Disabled,
    /// 密鑰不符 —— 永久性，Worker 不得 fail-open
    Unauthorized,
    /// 這封信本身解析不了 —— 永久性，重送也不會變
    Unprocessable(String),
    /// 面板自己的問題（DB 等）—— 暫時性，Worker 可走退路
    Internal(anyhow::Error),
}

impl IntoResponse for IngestError {
    fn into_response(self) -> Response {
        let (status, msg) = match self {
            Self::Disabled => (StatusCode::SERVICE_UNAVAILABLE, "信件端點未啟用".to_string()),
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "未授權".to_string()),
            Self::Unprocessable(m) => (StatusCode::UNPROCESSABLE_ENTITY, m),
            Self::Internal(e) => {
                tracing::error!("ingest 失敗: {e:#}");
                (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
            }
        };
        (status, Json(serde_json::json!({ "error": msg }))).into_response()
    }
}

impl From<anyhow::Error> for IngestError {
    fn from(e: anyhow::Error) -> Self {
        Self::Internal(e)
    }
}

/// Worker 用的共用密鑰認證（機器對機器，Worker 做不了 WebAuthn）。
/// 密鑰未設定時相關端點一律停用。
fn require_mail_secret(st: &Shared, headers: &HeaderMap) -> std::result::Result<(), IngestError> {
    if st.cfg.mail_secret.is_empty() {
        tracing::warn!("信件端點未啟用（NFHH_MAIL_SECRET 為空），已回 503 讓 Worker 照 FORWARD_MAP 轉發");
        return Err(IngestError::Disabled);
    }
    let given = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");

    // 定時比對，避免用回應時間反推密鑰
    let expect = st.cfg.mail_secret.as_bytes();
    let ok = given.len() == expect.len()
        && given
            .as_bytes()
            .iter()
            .zip(expect)
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0;
    if !ok {
        tracing::warn!("共用密鑰不符，已拒絕");
        return Err(IngestError::Unauthorized);
    }
    Ok(())
}

/// 寄件者宣告的日期只當顯示用，而且要夾在合理範圍：保留期之前、一小時之後
/// 一律改用現在。排序與保留期另外看 `ingested_at`（見 db::migrate_v12）。
///
/// 下限跟著保留期走而不是寫死一年：面板只留 `keep_days` 天，卻讓一封剛收到的
/// 信顯示「300 天前」，是拿自己的 UI 幫寄件者說謊。
fn clamp_received(claimed: Option<i64>, now: i64, keep_days: i64) -> i64 {
    match claimed {
        Some(t) if t <= now + 3600 && t >= now - keep_days * 86400 => t,
        _ => now,
    }
}

/// 接收 Worker 推來的原始信件，並回覆這封要轉給誰。
///
/// Worker 先推這裡、拿到 `forward_to` 才轉發 —— 篩選器的關鍵字要比對內文，
/// 而只有這裡解析得到內文。
///
/// ⚠️ 這支掛掉不能讓信轉不出去：Worker 逾時或收到 5xx（含未啟用的 503）會退回
///    FORWARD_MAP 照送。但 401／422 是拒收，Worker 只會轉給 FALLBACK_TO ——
///    所以拒絕要拒得準。
async fn mail_ingest(
    State(st): State<Shared>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> std::result::Result<Json<serde_json::Value>, IngestError> {
    require_mail_secret(&st, &headers)?;

    // 任務 6 已修掉已知的 panic；這層是防線：解析器再出問題也只影響這封信，
    // 而且回的是 422，Worker 會當「拒收」而不是「面板掛了」。
    let authserv = st.cfg.mail_authserv_id.clone();
    let p = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| mail::parse(&body, &authserv)))
        .map_err(|_| IngestError::Unprocessable("信件解析失敗".into()))?;
    // 以面板設定為準：環境變數只是首次啟動的種子（見 seed_settings）。
    // 以前這裡讀 Config，UI 撤銷的網域會一直被信任到下次重啟。
    let verified = p.auth.is_trusted(&db::get_setting_list(&st.db, db::keys::SENDER_DOMAINS));

    let mailbox = routing_mailbox(&headers, p.recipient.as_deref());

    // 平台決定誰看得到這封信的驗證碼。認不出來就留 None ——
    // 那封信只進管理收件匣，不會出現在任何人的驗證碼分頁。
    let known = platforms::list(&st.cfg.domain_set_dir);
    let senders = db::get_setting_map(&st.db, db::keys::PLATFORM_SENDERS);
    let mailboxes = db::get_setting_str_map(&st.db, db::keys::PLATFORM_MAILBOXES);
    let platform = platforms::classify(p.sender.as_deref(), &mailbox, &senders, &mailboxes, &known);
    let skip_reason = p.code.is_none().then_some("未擷取到驗證碼");

    let received = clamp_received(p.date, db::now(), st.cfg.mail_keep_days);
    let is_new = db::insert_mail(
        &st.db,
        p.message_id.as_deref(),
        received,
        p.sender.as_deref(),
        p.recipient.as_deref(),
        p.subject.as_deref(),
        p.code.as_deref(),
        Some(&p.body),
        p.html.as_deref(),
        &p.links,
        verified,
        platform.as_deref(),
        skip_reason,
    )?;

    // 讀 DB 而非環境變數，面板那顆「未通過驗證的信也轉發」開關才生效
    let withhold = !verified
        && db::get_setting(&st.db, db::keys::FORWARD_ENFORCE)?.unwrap_or_else(|| "1".into()) == "1";

    // 跟驗證碼分頁用同一支判斷，家人收到的與面板顯示的才會一致
    let actionable = mail::is_actionable(
        p.subject.as_deref(),
        Some(&p.body),
        p.code.is_some(),
        &db::get_setting_list(&st.db, db::keys::CODE_KEYWORDS),
        &db::get_setting_list(&st.db, db::keys::CODE_EXCLUDES),
    );

    let forward_to = forward_targets(
        withhold,
        actionable,
        db::enabled_recipients_for(&st.db, &mailbox)?,
    );

    if !verified {
        // 觀察期用：記錄真實的 header.d
        tracing::warn!(
            "寄件者未通過驗證 mailbox={} {} → {}",
            mailbox,
            p.auth.summary(),
            if withhold { "未轉發給家人" } else { "觀察期，照常轉發" }
        );
        db::audit(
            &st.db,
            None,
            "mail_sender_unverified",
            Some(&format!(
                "{} / {} / {}",
                p.sender.as_deref().unwrap_or("?"),
                p.auth.summary(),
                if withhold { "未轉發給家人" } else { "觀察期，照常轉發" }
            )),
            None,
        );
    }

    if is_new {
        tracing::info!(
            "收到信件 from={:?} subject={:?} code={} verified={} 篩選器={} 轉發={}",
            p.sender,
            p.subject,
            if p.code.is_some() { "有" } else { "無" },
            verified,
            if actionable { "通過" } else { "擋下" },
            forward_to.len()
        );
        db::audit(
            &st.db,
            None,
            "mail_received",
            Some(&format!(
                "{} / {} / 轉發 {} 人",
                p.sender.as_deref().unwrap_or("?"),
                p.subject.as_deref().unwrap_or("(無主旨)"),
                forward_to.len()
            )),
            None,
        );
    }
    // 通知裡放的是碼本身，所以跟的是**面板顯示**那條規則
    // （`sender_verify_mode`），不是轉發那條 —— 兩者會分岔，跟錯邊就是
    // 「通知有碼、點進面板什麼都沒有」。重送的信不推。
    let mode = db::get_setting(&st.db, db::keys::SENDER_MODE)?
        .unwrap_or_else(|| "observe".into());

    if is_new && actionable && mode_allows(&mode, Some(verified)) {
        if let Some(pf) = platform.clone() {
            // 背景送，不擋回應。Worker 正在等 forward_to。
            tokio::spawn(push_new_code(st.clone(), pf, p.code.clone()));
        }
    }

    let _ = db::purge_old_mails(&st.db, st.cfg.mail_keep_days);
    Ok(Json(serde_json::json!({
        "ok": true,
        "new": is_new,
        "code_found": p.code.is_some(),
        "verified": verified,
        // 空的 forward_to 有兩種成因（沒通過驗證 vs 被篩選器擋下），日誌要分得出來
        "actionable": actionable,
        // Worker 會在這份清單之外自行加上 FALLBACK_TO
        "forward_to": forward_to,
    })))
}

// ── 推送通知 ──────────────────────────────────────────

/// 一個人實際上有幾台裝置。訂閱永久寫入 SQLite，沒有上限就是免費的磁碟。
const MAX_PUSH_SUBS_PER_USER: i64 = 8;
const MAX_ENDPOINT_LEN: usize = 2048;
/// 同時存在的推送 task 數（也就是同時在飛的連線數）。
const PUSH_FANOUT_CONCURRENCY: usize = 8;
/// 整批扇出的總 deadline。每個請求各有 10 秒 timeout，但分輪送時會疊加。
const PUSH_FANOUT_DEADLINE_SECS: u64 = 60;

/// 把新驗證碼推給有這個平台授權的人。
///
/// ⚠️ 絕不能擋在 ingest 的回應路徑上：Worker 只等 5 秒，逾時就退回
///    FORWARD_MAP 自己送。呼叫端一律 `tokio::spawn`。
async fn push_new_code(st: Shared, platform: String, code: Option<String>) {
    let name = platforms::list(&st.cfg.domain_set_dir)
        .into_iter()
        .find(|p| p.code == platform)
        .map(|p| p.name)
        .unwrap_or_else(|| platform.clone());

    let n = push::Notification {
        title: format!("{name} 新驗證碼"),
        // 抽不到碼的也要推 —— Netflix 的「暫時存取碼」碼在連結後面。
        body: match &code {
            Some(c) => c.clone(),
            None => "收到一封需要處理的信，點開面板查看".into(),
        },
        tag: platform.clone(),
        url: "/".into(),
        code,
    };

    let subs = match db::push_subs_for_platform(&st.db, &platform) {
        Ok(s) => s,
        Err(e) => return tracing::error!("讀取推送對象失敗: {e:#}"),
    };
    fan_out(&st, subs, &n).await;
}

/// 送給一組訂閱，順帶清掉已死的。並行送，免得最慢的那台拖住全部。
///
/// 並行度有上限，而且限的是**同時存在的 task 數**而不只是同時在飛的連線：
/// 滿了就先等一個做完再開下一個，幾千筆訂閱不會先變成幾千個 task 排隊。
/// 整批再套一個 deadline —— 假 endpoint 各自吃滿 10 秒的話，分輪會疊加。
/// `JoinSet` 隨 `run` 一起被 drop 時會 abort 未完成的 task，逾時之後不會
/// 留下殘留連線。代價是全都是死 endpoint 時（每輪吃滿 10 秒，60 秒只夠
/// 6 輪）大約第 48 筆之後就送不到了 —— 由 `fail_count` 把那些訂閱逐出
/// 扇出來收斂，不是靠把 deadline 調大。
///
/// task 的結果一定要收：`join_all` 會把 panic 原地重拋，而呼叫端之一是
/// `renew_active` 那條長命的迴圈 —— 它被 unwind 掉就再也不會續期。
async fn fan_out(st: &Shared, subs: Vec<db::PushSub>, n: &push::Notification) {
    let run = async {
        let mut set = tokio::task::JoinSet::new();
        for sub in subs {
            if set.len() >= PUSH_FANOUT_CONCURRENCY {
                // 滿了就收掉一個再開下一個
                if let Some(Err(e)) = set.join_next().await {
                    tracing::warn!("推送 task 異常結束: {e}");
                }
            }
            let (st, n) = (st.clone(), n.clone());
            set.spawn(async move {
                match st.push.send(&st.db, &sub, &n).await {
                    // 訂閱不存在了，當場清掉 —— 留著只會每次都失敗且無法自癒
                    Ok(false) => {
                        let _ = db::delete_push_sub_by_endpoint(&st.db, &sub.endpoint);
                        tracing::info!("訂閱已失效，已移除（user={}）", sub.user_id);
                    }
                    Ok(true) => {
                        let _ = db::mark_push_ok(&st.db, sub.id);
                    }
                    Err(e) => {
                        let _ = db::bump_push_fail(&st.db, sub.id);
                        tracing::warn!("推送失敗（user={}）: {e:#}", sub.user_id);
                    }
                }
            });
        }
        while let Some(r) = set.join_next().await {
            if let Err(e) = r {
                tracing::warn!("推送 task 異常結束: {e}");
            }
        }
    };
    let deadline = std::time::Duration::from_secs(PUSH_FANOUT_DEADLINE_SECS);
    if tokio::time::timeout(deadline, run).await.is_err() {
        tracing::warn!("推送扇出超過 {PUSH_FANOUT_DEADLINE_SECS} 秒，剩餘請求已放棄");
    }
}

/// 這封信要扇出給誰。兩個否決條件，任一成立就不轉給家人。
///
/// 回空陣列不代表沒人收得到 —— Worker 會補上 FALLBACK_TO。
fn forward_targets(withhold: bool, actionable: bool, enabled: Vec<String>) -> Vec<String> {
    if withhold || !actionable {
        Vec::new()
    } else {
        enabled
    }
}

/// 決定用哪個信箱去查轉發名單。
///
/// 優先用 Worker 帶來的信封收件位址（SMTP 實際投遞目標），退回 `To:` 表頭
/// 以相容尚未更新的 Worker。兩者都沒有就回空字串，查不到任何收件人。
fn routing_mailbox(headers: &HeaderMap, parsed_to: Option<&str>) -> String {
    headers
        .get("x-nfhh-mailbox")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or(parsed_to)
        .map(|s| s.trim().to_lowercase())
        .unwrap_or_default()
}

/// `sender_verify_mode` 對一封信的裁決。
///
/// `verified` 為 None 是 v5 之前的舊信 —— 那是「無驗證資訊」而不是
/// 「未通過」，observe 下一樣顯示，只是前端標成灰色而非琥珀色。
fn mode_allows(mode: &str, verified: Option<bool>) -> bool {
    match mode {
        // 未通過的擋掉，只進管理收件匣
        "enforce" => verified == Some(true),
        // off：不驗證；observe（預設）：未通過也顯示，前端標琥珀色
        _ => true,
    }
}

/// member 對一封信的可見範圍。清單、單封讀取、刪除三處共用同一條規則 ——
/// 分開寫過一次，結果是清單看不到的信可以用猜 id 的方式讀到與刪掉。
struct MailScope {
    granted: Vec<String>,
    mode: String,
    keywords: Vec<String>,
    excludes: Vec<String>,
}

impl MailScope {
    fn load(st: &Shared, uid: &str) -> Result<Self> {
        Ok(Self {
            granted: db::platforms_for(&st.db, uid)?,
            mode: db::get_setting(&st.db, db::keys::SENDER_MODE)?.unwrap_or_else(|| "observe".into()),
            keywords: db::get_setting_list(&st.db, db::keys::CODE_KEYWORDS),
            excludes: db::get_setting_list(&st.db, db::keys::CODE_EXCLUDES),
        })
    }

    fn allows(
        &self,
        platform: Option<&str>,
        verified: Option<bool>,
        subject: Option<&str>,
        body: Option<&str>,
        has_code: bool,
    ) -> bool {
        platform.is_some_and(|p| self.granted.iter().any(|g| g == p))
            && mode_allows(&self.mode, verified)
            // 有碼、或命中關鍵字。後者是為了 Netflix 的「暫時存取碼」——
            // 碼不在信裡而在「取得存取碼」那顆按鈕後面，抽不到數字不代表
            // 這封信對家人沒用。
            && mail::is_actionable(subject, body, has_code, &self.keywords, &self.excludes)
    }

    fn allows_mail(&self, m: &db::Mail) -> bool {
        self.allows(m.platform.as_deref(), m.verified, m.subject.as_deref(), m.body.as_deref(), m.code.is_some())
    }
}

/// 只有「寄件者通過驗證」且「連結 host 落在該平台網域」才准畫品牌按鈕。
/// 平台是由收件信箱分類的，跟寄件者是誰無關 —— 沒有這道檢查，任何人寄到
/// netflix@ 信箱的釣魚連結都會穿上 Netflix 的外衣。
///
/// host 交給 `url` crate 判讀（跟瀏覽器同一套 WHATWG 規則：`\\` 當 `/`、
/// host 小寫、IDN 轉 punycode）；帶帳密的 URL 一律拒絕。
fn brand_link_allowed(verified: Option<bool>, link: &str, domains: &[String]) -> bool {
    if verified != Some(true) {
        return false;
    }
    let Ok(u) = url::Url::parse(link) else { return false };
    if u.scheme() != "https" || !u.username().is_empty() || u.password().is_some() {
        return false;
    }
    let Some(host) = u.host_str() else { return false };
    // `.netflix.com` 會過 ends_with 檢查、卡片卻印出一個前導點；尾點是另一個 origin。
    // 兩種都不是平台會寄的連結，直接拒絕。
    if host.starts_with('.') || host.ends_with('.') {
        return false;
    }
    domains.iter().any(|d| host == d || host.ends_with(&format!(".{d}")))
}

/// 不合格就把 `primary_link` 拿掉。清單與單封走的是兩條程式路徑，判斷式只寫這一份。
fn withhold_unbranded(link: &mut Option<String>, verified: Option<bool>, domains: &[String]) {
    if link.as_deref().is_some_and(|l| !brand_link_allowed(verified, l, domains)) {
        *link = None;
    }
}

/// 對一批摘要套用 `brand_link_allowed`，不合格的把 `primary_link` 拿掉。
fn strip_unbranded_links(st: &Shared, mails: &mut [db::MailSummary]) {
    let mut cache: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for m in mails.iter_mut() {
        if m.primary_link.is_none() {
            continue;
        }
        let code = m.platform.clone().unwrap_or_default();
        let domains = cache
            .entry(code.clone())
            .or_insert_with(|| platforms::domains(&st.cfg.domain_set_dir, &code));
        withhold_unbranded(&mut m.primary_link, m.verified, domains);
    }
}

/// 驗證碼分頁的內容。
///
/// 能不能看到一封信由 [`MailScope`] 說了算 —— 平台分權、顯示策略、可用性
/// 三個條件都寫在那裡，清單與刪除共用同一份。這支只負責取最近的信、
/// 套上那條規則、截成一頁。
///
/// admin 想看全部要去管理的收件匣，那支端點是另一條路徑。
///
/// 回的是摘要（[`db::MailSummary`]）而不是全文：這支每 20 秒被每個開著的
/// 分頁輪詢一次，body / html / links 一起送等於讓寄件者決定要吃多少流量。
/// 全文走 [`mail_get`]，點開原始信件時才拿一封。
async fn mail_list(
    State(st): State<Shared>,
    session: Session,
) -> ApiResult<Json<Vec<db::MailSummary>>> {
    let (uid, _) = require_user(&st, &session).await?;
    let _ = db::purge_old_mails(&st.db, st.cfg.mail_keep_days);

    let scope = MailScope::load(&st, &uid)?;
    let mut mails: Vec<_> = db::recent_mail_summaries(&st.db, Some(&scope.granted), 60)?
        .into_iter()
        .filter(|m| {
            scope.allows(
                m.platform.as_deref(),
                m.verified,
                m.subject.as_deref(),
                m.body.as_deref(),
                m.code.is_some(),
            )
        })
        .take(30)
        .collect();
    strip_unbranded_links(&st, &mut mails);

    Ok(Json(mails))
}

/// 管理收件匣：不做平台過濾也不做驗證過濾，什麼都看得到。
/// 這是 admin 診斷「為什麼某封信沒出現在驗證碼分頁」的地方（設計 1n）。
async fn mail_inbox(
    State(st): State<Shared>,
    session: Session,
) -> ApiResult<Json<Vec<db::MailSummary>>> {
    require_admin(&st, &session).await?;
    let mut mails = db::recent_mail_summaries(&st.db, None, 60)?;
    // admin 也不該看到假品牌按鈕 —— 這支是診斷用的，越是要照實呈現
    strip_unbranded_links(&st, &mut mails);
    Ok(Json(mails))
}

/// 全文的唯一出口。授權跟清單、刪除同一個 [`MailScope`] ——
/// 分開寫的話，清單看不到的信可以用猜 id 的方式讀到。
async fn mail_get(
    State(st): State<Shared>,
    session: Session,
    Path(id): Path<i64>,
) -> ApiResult<Json<db::Mail>> {
    let (uid, _) = require_user(&st, &session).await?;
    let me = db::get_user(&st.db, &uid)?.context("帳號已不存在")?;
    let mut m = db::get_mail(&st.db, id)?.context("查無此信件")?;
    if !me.is_admin() && !MailScope::load(&st, &uid)?.allows_mail(&m) {
        // 不分辨「不存在」與「不是你的」：不給枚舉 id 的人存在性 oracle
        return Err(AppError(anyhow::anyhow!("查無此信件")));
    }
    if m.primary_link.is_some() {
        let domains = platforms::domains(&st.cfg.domain_set_dir, m.platform.as_deref().unwrap_or(""));
        withhold_unbranded(&mut m.primary_link, m.verified, &domains);
    }
    Ok(Json(m))
}

async fn mail_delete(
    State(st): State<Shared>,
    session: Session,
    Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    let (uid, _) = require_user(&st, &session).await?;
    let me = db::get_user(&st.db, &uid)?.context("帳號已不存在")?;
    let n = if me.is_admin() {
        db::delete_mail_if(&st.db, id, |_| true)?
    } else {
        let scope = MailScope::load(&st, &uid)?;
        db::delete_mail_if(&st.db, id, |m| scope.allows_mail(m))?
    };
    Ok(Json(serde_json::json!({ "ok": n > 0 })))
}

/// 「全部刪除」。限管理員 —— 這會清掉所有人的驗證碼，
/// 不該由任何一個成員單方面觸發。
async fn mail_delete_all(
    State(st): State<Shared>,
    session: Session,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let me = require_admin(&st, &session).await?;
    let n = db::delete_all_mails(&st.db)?;
    db::audit(
        &st.db,
        Some(me.label()),
        "mail_purged",
        Some(&format!("{n} 封")),
        client_ip(&headers).as_deref(),
    );
    Ok(Json(serde_json::json!({ "ok": true, "deleted": n })))
}

// ── 轉發收件人（僅管理員；內容是家人信箱，屬個資）───────

#[derive(Deserialize)]
struct RecipientReq {
    mailbox: String,
    address: String,
    label: Option<String>,
}

#[derive(Deserialize)]
struct ToggleReq {
    enabled: bool,
}

/// Cloudflare 的驗證狀態多久重查一次。這個值幾乎不會變 ——
/// 一個位址驗證過就是永遠驗證過，只有新增的才需要盡快反映。
const CF_REFRESH_SECS: i64 = 3600;

async fn recipient_list(
    State(st): State<Shared>,
    session: Session,
) -> ApiResult<Json<serde_json::Value>> {
    require_admin(&st, &session).await?;

    // 懶惰更新：有人打開這頁而且快取過期了才去問 Cloudflare。
    // 不開背景任務 —— 這份資料只有這一頁在用，沒必要一直輪詢。
    if st.cf.enabled() {
        let now = db::now();
        let stale = db::list_recipients(&st.db)?
            .iter()
            .any(|r| r.cf_checked_at.is_none_or(|t| now - t > CF_REFRESH_SECS));

        if stale {
            // Cloudflare 掛掉只該讓狀態欄顯示舊值，不該讓整頁打不開。
            match st.cf.destinations().await {
                // 整份覆寫，不是逐筆更新 —— Cloudflare 沒回傳的位址代表它
                // 根本沒有那個目的地，那筆的轉發一定會失敗，必須看得出來。
                Ok(list) => {
                    let found: Vec<_> =
                        list.into_iter().map(|d| (d.email, d.verified_at)).collect();
                    if let Err(e) = db::sync_cf_status(&st.db, &found) {
                        tracing::warn!("寫入 Cloudflare 驗證狀態失敗: {e:#}");
                    }
                }
                Err(e) => tracing::warn!("查詢 Cloudflare 驗證狀態失敗: {e:#}"),
            }
        }
    }

    // 一起回「平台 → 收件信箱」的對應：那頁的分組就是照它排的，
    // 少了它前端只能回去猜 `代號@網域`，而那正是要修掉的東西。
    Ok(Json(serde_json::json!({
        "recipients": db::list_recipients(&st.db)?,
        "mailboxes": db::get_setting_str_map(&st.db, db::keys::PLATFORM_MAILBOXES),
        "default_domain": db::get_setting(&st.db, db::keys::MAIL_DOMAIN)?
            .unwrap_or_else(|| st.cfg.mail_domain.clone()),
    })))
}

async fn recipient_add(
    State(st): State<Shared>,
    session: Session,
    headers: HeaderMap,
    Json(req): Json<RecipientReq>,
) -> ApiResult<Json<serde_json::Value>> {
    let me = require_admin(&st, &session).await?;
    let mailbox = req.mailbox.trim();
    let address = req.address.trim();
    // 只做最低限度格式檢查；真正的護欄是 Cloudflare Email Routing
    // 的目的地位址驗證
    if !mailbox.contains('@') || !address.contains('@') {
        return Err(AppError(anyhow::anyhow!("信箱格式不正確")));
    }
    db::add_recipient(&st.db, mailbox, address, req.label.as_deref(), &me.username)?;

    // 順手在 Cloudflare 建立目的地位址。不做的話會留下一筆「面板上開著、
    // Cloudflare 根本沒有」的收件人 —— 轉發一定退信，而且從面板上看不出來
    // （那正是 cf_present 這一欄存在的理由）。POST 是冪等的。
    let mut warn = None;
    if st.cf.enabled() {
        match st.cf.create_destination(address).await {
            Ok(true) => {}
            Ok(false) => {}
            Err(e) => {
                tracing::warn!("建立 Cloudflare 目的地位址失敗: {e:#}");
                warn = Some(format!("{e}。位址已加進名單，但要先在 Cloudflare 完成驗證才收得到轉發"));
            }
        }
    }

    db::audit(
        &st.db,
        Some(&me.username),
        "recipient_added",
        Some(&format!("{mailbox} → {address}")),
        client_ip(&headers).as_deref(),
    );
    Ok(Json(serde_json::json!({ "ok": true, "warn": warn })))
}

/// 幫某筆收件人在 Cloudflare 建立位址／重寄驗證信。
///
/// 給既有的壞資料用：面板上有這個人、Cloudflare 沒有（`cf_present = 0`），
/// 或有但沒驗證。兩種情況同一支 POST 都能處理。
#[derive(Deserialize)]
struct MailboxReq {
    platform: String,
    /// 空字串 = 取消這個平台的對應。
    mailbox: String,
}

/// 設定「這個平台的驗證碼信寄到哪個信箱」。
///
/// ⚠️ 以前靠 `代號@網域` 推，而那對 Disney+ 是錯的（代號 disneyplus、
/// 信箱 disney@）。改成 admin 明說，新增轉發與信件分類都以它為準。
async fn mailbox_set(
    State(st): State<Shared>,
    session: Session,
    headers: HeaderMap,
    Json(req): Json<MailboxReq>,
) -> ApiResult<Json<serde_json::Value>> {
    let me = require_admin(&st, &session).await?;
    let known = platforms::list(&st.cfg.domain_set_dir);
    if !known.iter().any(|p| p.code == req.platform) {
        return Err(AppError(anyhow::anyhow!("沒有這個平台：{}", req.platform)));
    }

    let mailbox = req.mailbox.trim().to_lowercase();
    let mut boxes = db::get_setting_str_map(&st.db, db::keys::PLATFORM_MAILBOXES);
    if mailbox.is_empty() {
        boxes.remove(&req.platform);
    } else {
        if !mailbox.contains('@') {
            return Err(AppError(anyhow::anyhow!("請輸入完整的收件信箱")));
        }
        // 一個信箱只能對應一個平台，否則分類會挑到不確定的那個
        if let Some((other, _)) = boxes.iter().find(|(c, m)| *c != &req.platform && *m == &mailbox) {
            return Err(AppError(anyhow::anyhow!("{mailbox} 已經對應到 {other}")));
        }
        boxes.insert(req.platform.clone(), mailbox.clone());
    }
    db::set_setting_str_map(&st.db, db::keys::PLATFORM_MAILBOXES, &boxes, Some(me.label()))?;
    db::audit(
        &st.db,
        Some(me.label()),
        "platform_mailbox_set",
        Some(&format!("{} → {}", req.platform, if mailbox.is_empty() { "（取消）" } else { &mailbox })),
        client_ip(&headers).as_deref(),
    );
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// 永久刪掉一個信箱底下的所有轉發登記。給「這個信箱一開始就設錯」用。
async fn mailbox_purge(
    State(st): State<Shared>,
    session: Session,
    headers: HeaderMap,
    Path(mailbox): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let me = require_admin(&st, &session).await?;
    let n = db::delete_recipients_for_mailbox(&st.db, &mailbox)?;

    // 順手把對應也拿掉，否則畫面上那個信箱還會以「已設定」的樣子回來
    let mut boxes = db::get_setting_str_map(&st.db, db::keys::PLATFORM_MAILBOXES);
    let before = boxes.len();
    boxes.retain(|_, m| m.trim().to_lowercase() != mailbox.trim().to_lowercase());
    if boxes.len() != before {
        db::set_setting_str_map(&st.db, db::keys::PLATFORM_MAILBOXES, &boxes, Some(me.label()))?;
    }

    db::audit(
        &st.db,
        Some(me.label()),
        "mailbox_purged",
        Some(&format!("{mailbox} · {n} 筆")),
        client_ip(&headers).as_deref(),
    );
    Ok(Json(serde_json::json!({ "ok": true, "removed": n })))
}

async fn recipient_verify(
    State(st): State<Shared>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    let me = require_admin(&st, &session).await?;
    if !st.cf.enabled() {
        return Err(AppError(anyhow::anyhow!("面板未設定 Cloudflare 帳戶或 token")));
    }
    let r = db::list_recipients(&st.db)?
        .into_iter()
        .find(|r| r.id == id)
        .context("找不到這筆收件人")?;

    let sent = st.cf.create_destination(&r.address).await?;
    db::audit(
        &st.db,
        Some(me.label()),
        "recipient_verify_sent",
        Some(&format!("{} {}", r.address, if sent { "已寄出" } else { "未寄出（已驗證或冷卻中）" })),
        client_ip(&headers).as_deref(),
    );
    Ok(Json(serde_json::json!({ "ok": true, "sent": sent })))
}

async fn recipient_toggle(
    State(st): State<Shared>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(req): Json<ToggleReq>,
) -> ApiResult<Json<serde_json::Value>> {
    let me = require_admin(&st, &session).await?;
    let n = db::set_recipient_enabled(&st.db, id, req.enabled)?;
    db::audit(
        &st.db,
        Some(&me.username),
        if req.enabled { "recipient_enabled" } else { "recipient_disabled" },
        Some(&format!("id={id}")),
        client_ip(&headers).as_deref(),
    );
    Ok(Json(serde_json::json!({ "ok": n > 0 })))
}

async fn recipient_remove(
    State(st): State<Shared>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    let me = require_admin(&st, &session).await?;
    let n = db::delete_recipient(&st.db, id)?;
    db::audit(
        &st.db,
        Some(&me.username),
        "recipient_removed",
        Some(&format!("id={id}")),
        client_ip(&headers).as_deref(),
    );
    Ok(Json(serde_json::json!({ "ok": n > 0 })))
}

// ── 登記邀請 Email（僅管理員）─────────────────────────
//
// 取代 v6 之前的邀請碼連結。連結可以被轉傳，登記的位址不行 ——
// 對方必須輸入完全相同的位址，並收得到寄到該信箱的驗證碼。
//
// 刻意不設有效期：家人可能隔幾個月才想起來要註冊。要收回是 admin 的
// 明確動作（撤銷），不是時間到了自己消失。

#[derive(Deserialize)]
struct InviteEmailReq {
    email: String,
    /// 註冊完成時要自動授予的平台。省略 = 什麼都不給。
    #[serde(default)]
    platforms: Vec<String>,
}

async fn invite_list(
    State(st): State<Shared>,
    session: Session,
) -> ApiResult<Json<Vec<db::InvitedEmail>>> {
    require_admin(&st, &session).await?;
    Ok(Json(db::list_invited_emails(&st.db)?))
}

#[derive(Serialize)]
struct InviteRes {
    email: String,
    /// 邀請連結。**只有這一刻拿得到** —— DB 只存雜湊，重看只能重新登記
    /// 換一條。所以就算信寄出去了也一併回，admin 想改用別的管道傳都行。
    link: String,
    /// 邀請函寄出去了嗎。
    sent: bool,
    /// 沒寄成的原因。登記本身已經成立，這是提醒不是失敗。
    warn: Option<String>,
}

async fn invite_create(
    State(st): State<Shared>,
    session: Session,
    headers: HeaderMap,
    Json(req): Json<InviteEmailReq>,
) -> ApiResult<Json<InviteRes>> {
    let me = require_admin(&st, &session).await?;
    let ip = client_ip(&headers);
    let email = req.email.trim().to_lowercase();
    if !email.contains('@') {
        return Err(AppError(anyhow::anyhow!("請輸入完整的 Email 位址")));
    }
    if db::find_user_by_email(&st.db, &email)?.is_some() {
        return Err(AppError(anyhow::anyhow!("這個位址已經有帳號了")));
    }

    // 只收目前實際存在的平台，避免打錯字產生一筆永遠不會生效的授權
    let known = platforms::list(&st.cfg.domain_set_dir);
    if let Some(bad) = req.platforms.iter().find(|c| !known.iter().any(|p| &&p.code == c)) {
        return Err(AppError(anyhow::anyhow!("沒有這個平台：{bad}")));
    }

    db::invite_email(&st.db, &email, me.label(), &req.platforms)?;

    // 登記完成後才掛連結。重新登記會先把舊的清掉（見 db::invite_email），
    // 所以同一個位址永遠只有最新那一條連結有效。
    let token = invite::generate();
    if !db::set_invite_token(&st.db, &email, &invite::hash(&st.db, &token)?)? {
        return Err(AppError(anyhow::anyhow!("登記已被撤銷或註冊，請重新登記")));
    }
    let link = invite::link(&st.cfg.origin, &token);

    // 依選取的平台順帶登記轉發信箱，讓對方一註冊完就收得到碼 ——
    // 不必等 admin 再回來設一次。`add_recipient` 的 ON CONFLICT 會把
    // 已存在的那筆恢復啟用，「若已存在則忽略」不必自己判斷。
    //
    // ⚠️ 信箱一定要查 admin 設定的對應，**不能用 `代號@網域` 推**。
    //    那個約定對 Netflix 剛好成立、對 Disney+ 就是錯的
    //    （代號 `disneyplus`、信箱 `disney@`），推錯會建出一個
    //    根本收不到信的轉發目標，而且完全沒有徵兆。
    let boxes = db::get_setting_str_map(&st.db, db::keys::PLATFORM_MAILBOXES);
    let mut unmapped: Vec<&str> = Vec::new();
    for code in &req.platforms {
        match boxes.get(code) {
            Some(mailbox) => {
                db::add_recipient(&st.db, mailbox, &email, None, me.label())?;
            }
            // 沒設對應就不猜。少建一筆轉發，admin 補完設定再登記一次就好；
            // 猜錯的那筆會安靜地什麼都收不到。
            None => unmapped.push(code),
        }
    }

    // 位址要先在 Cloudflare 完成驗證才收得到轉發，所以順手建一下 ——
    // 那支 POST 是冪等的：新位址會寄出驗證信，已驗證的原封不動。
    // 失敗不回滾登記，理由跟寄信失敗一樣（見下方）。
    let mut cf_warn = None;
    if !unmapped.is_empty() {
        cf_warn = Some(format!(
            "{} 還沒設定收件信箱，沒有建立轉發（到「轉發收件人」頁設定）",
            unmapped.join("、")
        ));
    }
    if !req.platforms.is_empty() && st.cf.enabled() {
        match st.cf.create_destination(&email).await {
            Ok(true) => tracing::info!("已為 {email} 建立 Cloudflare 目的地位址並寄出驗證信"),
            Ok(false) => tracing::info!("{email} 的 Cloudflare 位址已驗證或驗證信剛寄過"),
            Err(e) => {
                tracing::warn!("建立 Cloudflare 目的地位址失敗: {e:#}");
                cf_warn = Some("轉發位址未能在 Cloudflare 建立，對方要先完成驗證才收得到轉發".to_string());
            }
        }
    }

    db::audit(
        &st.db,
        Some(me.label()),
        "invite_email_registered",
        Some(&format!("{email} 平台={} 轉發={}",
            if req.platforms.is_empty() { "無".to_string() } else { req.platforms.join(",") },
            if req.platforms.is_empty() { "未新增" } else { "已新增" })),
        ip.as_deref(),
    );

    // 寄信失敗不回滾登記：位址已經可以用了，對方走「用 Email 加入」照樣
    // 進得來，admin 也還有連結可以自己傳。回 200 帶 warn，而不是讓整個
    // 動作看起來沒發生。
    let (sent, mut warn) = if !st.mailer.enabled() {
        (false, Some("尚未設定寄信服務，請自行把連結傳給對方".to_string()))
    } else {
        match st
            .mailer
            .send_invite(&email, &link, &platform_names(&known, &req.platforms))
            .await
        {
            Ok(()) => {
                db::audit(&st.db, Some(me.label()), "invite_mail_sent", Some(&email), ip.as_deref());
                (true, None)
            }
            Err(e) => {
                db::audit(
                    &st.db,
                    Some(me.label()),
                    "invite_mail_failed",
                    Some(&format!("{email}：{e}")),
                    ip.as_deref(),
                );
                (false, Some(format!("{e}，請改用下面的連結自行傳給對方")))
            }
        }
    };

    // Cloudflare 那邊的問題也要說出來 —— 對方收得到邀請函卻收不到驗證碼
    // 是最難查的那種壞法，admin 現在就該知道。
    if let Some(w) = cf_warn {
        warn = Some(match warn {
            Some(prev) => format!("{prev}；{w}"),
            None => w,
        });
    }

    Ok(Json(InviteRes { email, link, sent, warn }))
}

/// 邀請函上那一行「可用服務」。
///
/// 用顯示名而不是代號 —— 收信的人不知道 `disneyplus` 是什麼。沒指定平台是
/// 合法的登記，那時要講明白之後才會開通，不能留一塊空白讓人以為都有。
fn platform_names(known: &[platforms::Platform], codes: &[String]) -> String {
    let names: Vec<&str> = codes
        .iter()
        .map(|c| {
            known
                .iter()
                .find(|p| &p.code == c)
                .map_or(c.as_str(), |p| p.name.as_str())
        })
        .collect();
    if names.is_empty() {
        "尚未指定（管理員稍後開通）".to_string()
    } else {
        names.join("、")
    }
}

/// 撤銷。只對還沒用掉的位址有效 —— 已註冊的要移除的是帳號本身，
/// 不是這筆登記紀錄（見 db::revoke_invited_email）。
async fn invite_revoke(
    State(st): State<Shared>,
    session: Session,
    headers: HeaderMap,
    Path(email): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let me = require_admin(&st, &session).await?;
    let n = db::revoke_invited_email(&st.db, &email)?;
    if n == 0 {
        return Err(AppError(anyhow::anyhow!(
            "撤銷失敗：這個位址不存在，或已經註冊完成"
        )));
    }
    db::audit(
        &st.db,
        Some(me.label()),
        "invite_email_revoked",
        Some(&email),
        client_ip(&headers).as_deref(),
    );
    Ok(Json(serde_json::json!({ "ok": true })))
}

// ── 個人的 Passkey ────────────────────────────────────
//
// 一律只能操作**自己的**。這裡沒有管理員視角：admin 可以把某個成員降權或
// 看他加了哪些 IP，但不能碰別人的憑證 —— 那是登入手段本身，不是設定。

// ── 自己的轉發設定 ───────────────────────────────────
//
// admin 那頁管全部人、以 mailbox 為單位；這裡是「我自己收不收」，
// 一顆總開關切掉名下所有 mailbox。兩者操作同一張表。

/// 這個人的信箱在轉發名單上的狀態。
/// `address` 為 None = 這個帳號還沒有 email（v6 之前註冊的）。
async fn my_forwarding(
    State(st): State<Shared>,
    session: Session,
) -> ApiResult<Json<serde_json::Value>> {
    let (uid, _) = require_user(&st, &session).await?;
    let user = db::get_user(&st.db, &uid)?.context("帳號已不存在")?;
    let Some(address) = user.email.clone() else {
        return Ok(Json(serde_json::json!({ "address": null })));
    };

    let rows = db::recipients_for_address(&st.db, &address)?;
    Ok(Json(serde_json::json!({
        "address": address,
        // 沒有任何登記 = admin 還沒幫他設定，不是「關掉了」
        "registered": !rows.is_empty(),
        // 有一個開著就算開著。「部分開啟」只可能來自 admin 那頁的細部調整
        "enabled": rows.iter().any(|r| r.enabled),
        "mailboxes": rows.iter().map(|r| &r.mailbox).collect::<Vec<_>>(),
        "cf_enabled": st.cf.enabled(),
        // 同一個位址在各 mailbox 底下共用驗證狀態，取第一筆即可
        "cf_verified_at": rows.first().and_then(|r| r.cf_verified_at),
        "cf_checked_at": rows.first().and_then(|r| r.cf_checked_at),
        // false = Cloudflare 根本沒有這個位址，轉發一定失敗
        "cf_present": rows.first().and_then(|r| r.cf_present),
    })))
}

async fn my_forwarding_set(
    State(st): State<Shared>,
    session: Session,
    headers: HeaderMap,
    Json(req): Json<ToggleReq>,
) -> ApiResult<Json<serde_json::Value>> {
    let (uid, _) = require_user(&st, &session).await?;
    let user = db::get_user(&st.db, &uid)?.context("帳號已不存在")?;
    let address = user.email.clone().context("這個帳號還沒有 Email")?;

    let n = db::set_recipients_enabled_for_address(&st.db, &address, req.enabled)?;
    if n == 0 {
        return Err(AppError(anyhow::anyhow!("你的信箱還沒有被登記為轉發對象")));
    }
    db::audit(
        &st.db,
        Some(user.label()),
        if req.enabled { "forwarding_self_enabled" } else { "forwarding_self_disabled" },
        Some(&format!("{n} 個信箱")),
        client_ip(&headers).as_deref(),
    );
    Ok(Json(serde_json::json!({ "ok": true, "changed": n })))
}

/// 重寄 Cloudflare 的目的地位址驗證信 —— 就是再打一次建立位址那支
/// （見 `cloudflare.rs` 檔頭）。節流由 Cloudflare 做，面板不疊第二層。
async fn my_forwarding_resend(
    State(st): State<Shared>,
    session: Session,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let (uid, _) = require_user(&st, &session).await?;
    let user = db::get_user(&st.db, &uid)?.context("帳號已不存在")?;
    let address = user.email.clone().context("這個帳號還沒有 Email")?;

    if !st.cf.enabled() {
        return Err(AppError(anyhow::anyhow!(
            "面板未設定 Cloudflare，請自行到 Cloudflare 儀表板重發"
        )));
    }
    if db::recipients_for_address(&st.db, &address)?.is_empty() {
        return Err(AppError(anyhow::anyhow!("你的信箱還沒有被登記為轉發對象")));
    }

    let sent = st.cf.create_destination(&address).await?;
    db::audit(
        &st.db,
        Some(user.label()),
        "forwarding_verify_resent",
        Some(if sent { "已寄出" } else { "未寄出（已驗證或冷卻中）" }),
        client_ip(&headers).as_deref(),
    );
    Ok(Json(serde_json::json!({ "ok": true, "sent": sent })))
}

// ── 推送訂閱端點 ─────────────────────────────────────
//
// 一律只操作自己的，跟 passkey 那邊同一個原則。

/// 前端訂閱要用的 VAPID 公鑰。公鑰不是機密，但也沒理由讓未登入的人拿。
async fn push_key(State(st): State<Shared>, session: Session) -> ApiResult<Json<serde_json::Value>> {
    require_user(&st, &session).await?;
    let (_, public) = push::vapid_keys(&st.db)?;
    Ok(Json(serde_json::json!({ "key": public })))
}

#[derive(Deserialize)]
struct SubscribeReq {
    endpoint: String,
    p256dh: String,
    auth: String,
    label: Option<String>,
}

async fn push_subscribe(
    State(st): State<Shared>,
    session: Session,
    Json(req): Json<SubscribeReq>,
) -> ApiResult<Json<serde_json::Value>> {
    let (uid, _) = require_user(&st, &session).await?;

    // endpoint 決定面板往哪裡送 POST。推送服務一律是 https，而且這個字串
    // 會永久留在資料庫裡 —— 沒有長度上限就是讓人免費寫磁碟。
    let endpoint = req.endpoint.trim();
    if !endpoint.starts_with("https://") || endpoint.len() > MAX_ENDPOINT_LEN {
        return Err(AppError(anyhow::anyhow!(
            "推送 endpoint 必須是 https 且不超過 {MAX_ENDPOINT_LEN} 字元"
        )));
    }
    // 非空不等於能用：金鑰材料要真的解得開、p256dh 要在曲線上，
    // 否則存進去的是一筆每次扇出都在送出前就失敗的垃圾。
    let (p256dh, auth) = (req.p256dh.trim(), req.auth.trim());
    if !push::valid_keys(p256dh, auth) {
        return Err(AppError(anyhow::anyhow!("加密金鑰材料格式不正確")));
    }
    let label = req.label.as_deref().map(str::trim).filter(|s| !s.is_empty());
    check_label_len(label)?;

    if !db::add_push_sub(&st.db, &uid, endpoint, p256dh, auth, label, MAX_PUSH_SUBS_PER_USER)? {
        return Err(AppError(anyhow::anyhow!(
            "這個帳號的裝置訂閱已達上限（{MAX_PUSH_SUBS_PER_USER} 台），請先移除不用的"
        )));
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn push_list(
    State(st): State<Shared>,
    session: Session,
) -> ApiResult<Json<Vec<db::PushSub>>> {
    let (uid, _) = require_user(&st, &session).await?;
    Ok(Json(db::list_push_subs(&st.db, &uid)?))
}

#[derive(Deserialize)]
struct EndpointReq {
    endpoint: String,
}

/// 這台裝置的訂閱還算不算數。
///
/// 瀏覽器端的訂閱撤不掉別人的：別台裝置在設定裡把它停掉之後，這台的
/// `getSubscription()` 依然回一個物件，面板就一直顯示「已開啟」，
/// 但推播早就送不到了。所以要能回來問一句。
///
/// 只回一個布林 —— 問的人得先知道 endpoint（那是它自己建的），
/// 拿不到任何原本不外流的東西。
async fn push_check(
    State(st): State<Shared>,
    session: Session,
    Json(req): Json<EndpointReq>,
) -> ApiResult<Json<serde_json::Value>> {
    let (uid, _) = require_user(&st, &session).await?;
    Ok(Json(serde_json::json!({
        "registered": db::push_sub_exists(&st.db, &req.endpoint, &uid)?
    })))
}

/// 裝置自己退訂。endpoint 刻意不外流（見 `db::PushSub` 的 serde(skip)），
/// 前端只知道自己的 endpoint、不知道 id，所以這條路跟下面那支不重複。
async fn push_unsubscribe_self(
    State(st): State<Shared>,
    session: Session,
    Json(req): Json<EndpointReq>,
) -> ApiResult<Json<serde_json::Value>> {
    let (uid, _) = require_user(&st, &session).await?;
    Ok(Json(serde_json::json!({
        "ok": db::delete_push_sub_for_user(&st.db, &req.endpoint, &uid)? > 0
    })))
}

async fn push_unsubscribe(
    State(st): State<Shared>,
    session: Session,
    Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    let (uid, _) = require_user(&st, &session).await?;
    Ok(Json(serde_json::json!({
        "ok": db::delete_push_sub(&st.db, id, &uid)? > 0
    })))
}

#[derive(Deserialize)]
struct NotifyPrefsReq {
    codes: bool,
    expiry: bool,
}

async fn notify_prefs_get(
    State(st): State<Shared>,
    session: Session,
) -> ApiResult<Json<serde_json::Value>> {
    let (uid, _) = require_user(&st, &session).await?;
    let (codes, expiry) = db::notify_prefs(&st.db, &uid)?;
    Ok(Json(serde_json::json!({ "codes": codes, "expiry": expiry })))
}

async fn notify_prefs_set(
    State(st): State<Shared>,
    session: Session,
    Json(req): Json<NotifyPrefsReq>,
) -> ApiResult<Json<serde_json::Value>> {
    let (uid, _) = require_user(&st, &session).await?;
    db::set_notify_prefs(&st.db, &uid, req.codes, req.expiry)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn passkey_list(
    State(st): State<Shared>,
    session: Session,
) -> ApiResult<Json<Vec<db::Credential>>> {
    let (uid, _) = require_user(&st, &session).await?;
    Ok(Json(db::list_credentials(&st.db, &uid)?))
}

/// 撤銷一把。
///
/// ⚠️ 擋掉「刪到剩零把」。這個系統沒有密碼、沒有信箱救援可以繞過 Passkey ——
/// 刪光了就永遠登不進來，而且沒有任何介面能救（只能進資料庫手改）。
/// 剩最後一把時要換裝置的正確順序是：先在新裝置註冊，再撤銷舊的。
async fn passkey_delete(
    State(st): State<Shared>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let (uid, name) = require_user(&st, &session).await?;

    if db::credential_count(&st.db, &uid)? <= 1 {
        return Err(AppError(anyhow::anyhow!(
            "這是你唯一的 Passkey，撤銷之後就再也登不進來了。\
             請先在另一台裝置註冊一把，再回來撤銷這把。"
        )));
    }

    let n = db::delete_credential(&st.db, &uid, &id)?;
    if n == 0 {
        return Err(AppError(anyhow::anyhow!("找不到這把 Passkey")));
    }
    db::audit(
        &st.db,
        Some(&name),
        "passkey_revoked",
        Some(&id),
        client_ip(&headers).as_deref(),
    );
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn passkey_rename(
    State(st): State<Shared>,
    session: Session,
    Path(id): Path<String>,
    Json(req): Json<LabelReq>,
) -> ApiResult<Json<serde_json::Value>> {
    let (uid, _) = require_user(&st, &session).await?;
    let label = req.label.as_deref().map(str::trim).filter(|s| !s.is_empty());
    check_label_len(label)?;
    let n = db::rename_credential(&st.db, &uid, &id, label)?;
    Ok(Json(serde_json::json!({ "ok": n > 0 })))
}

// ── 面板設定（僅管理員）───────────────────────────────
//
// 這些以前是環境變數，改一次要重啟容器。搬進 DB 之後 UI 存檔即生效，
// 環境變數退居「首次啟動的種子值」（見 main() 裡的 seed_setting）。

/// ⚠️ `deny_unknown_fields`：前端送了一個這裡沒有的欄位時要**當場失敗**。
///
/// 沒有它的時候 serde 會安靜地把多的欄位丟掉 —— `forward_enforce` 就是這樣
/// 漏了一整版：畫面上按得動、存檔回 200、重讀之後又跳回原樣，沒有任何一處
/// 講得出哪裡不對。寧可回一個看得懂的 400。
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Settings {
    /// "off" | "observe" | "enforce"。**只管顯示，不管轉發。**
    ///
    /// 轉發由 `forward_enforce` 那一項決定。兩者刻意分開：面板上想看到
    /// 可疑的信（才查得出問題），不代表要把它轉給家人。這裡三檔的意思是
    /// 「未通過寄件者驗證的信要不要出現在驗證碼分頁」：
    ///   off     不驗證，全部當通過
    ///   observe 未通過也顯示，標琥珀色（預設）
    ///   enforce 未通過不顯示，只留在管理收件匣
    sender_mode: String,
    /// 可信的寄件品牌網域，比對 DKIM 的 header.d
    sender_domains: Vec<String>,
    code_keywords: Vec<String>,
    code_excludes: Vec<String>,
    /// 平台代號 → 寄件者位址／網域。收件信箱推不出平台時（例如用同一個
    /// catch-all 收全部）靠這份對應判定。
    platform_senders: std::collections::BTreeMap<String, Vec<String>>,
    /// 未通過寄件者驗證的信要不要**收掉轉發**（`true` = 收掉）。
    ///
    /// 對應 DB 的 `forward_enforce_sender`，也就是 `/api/mail/ingest` 回給
    /// Worker 的 `forward_to` 要不要清空。UI 上是反過來問的
    /// （「未通過驗證的信也轉發」= `!forward_enforce`）—— 開關要描述會發生
    /// 什麼事，不是描述某個旗標的值。
    forward_enforce: bool,
}

/// 設定頁附帶的診斷資訊：已收到但認不出平台的寄件位址。
/// 讓管理員從「憑記憶輸入」變成「從實際收過的挑」。
#[derive(Serialize)]
struct SettingsView {
    #[serde(flatten)]
    settings: Settings,
    unmatched_senders: Vec<UnmatchedSender>,
}

#[derive(Serialize)]
struct UnmatchedSender {
    address: String,
    count: i64,
}

async fn settings_get(
    State(st): State<Shared>,
    session: Session,
) -> ApiResult<Json<SettingsView>> {
    require_admin(&st, &session).await?;
    Ok(Json(SettingsView {
        settings: Settings {
            sender_mode: db::get_setting(&st.db, db::keys::SENDER_MODE)?
                .unwrap_or_else(|| "observe".into()),
            sender_domains: db::get_setting_list(&st.db, db::keys::SENDER_DOMAINS),
            code_keywords: db::get_setting_list(&st.db, db::keys::CODE_KEYWORDS),
            code_excludes: db::get_setting_list(&st.db, db::keys::CODE_EXCLUDES),
            platform_senders: db::get_setting_map(&st.db, db::keys::PLATFORM_SENDERS),
            // 預設收掉：跟 mail_ingest 讀這個鍵時的預設值一致，
            // 兩邊不一致的話畫面顯示的會不是實際生效的行為
            forward_enforce: db::get_setting(&st.db, db::keys::FORWARD_ENFORCE)?
                .unwrap_or_else(|| "1".into())
                == "1",
        },
        unmatched_senders: db::unmatched_senders(&st.db)?
            .into_iter()
            .map(|(address, count)| UnmatchedSender { address, count })
            .collect(),
    }))
}

async fn settings_put(
    State(st): State<Shared>,
    session: Session,
    headers: HeaderMap,
    Json(req): Json<Settings>,
) -> ApiResult<Json<serde_json::Value>> {
    let me = require_admin(&st, &session).await?;
    if !matches!(req.sender_mode.as_str(), "off" | "observe" | "enforce") {
        return Err(AppError(anyhow::anyhow!(
            "處理模式只能是 off / observe / enforce"
        )));
    }

    // 網域一律正規化後存，比對時才不會因為大小寫或空白漏掉
    let domains: Vec<String> = req
        .sender_domains
        .iter()
        .map(|d| d.trim().to_lowercase())
        .filter(|d| !d.is_empty())
        .collect();

    let by = Some(me.label());
    db::set_setting(&st.db, db::keys::SENDER_MODE, &req.sender_mode, by)?;
    db::set_setting_list(&st.db, db::keys::SENDER_DOMAINS, &domains, by)?;
    db::set_setting_list(&st.db, db::keys::CODE_KEYWORDS, &req.code_keywords, by)?;
    db::set_setting_list(&st.db, db::keys::CODE_EXCLUDES, &req.code_excludes, by)?;

    // 位址清單一律正規化後存，比對時才不會因為大小寫或空白漏掉
    let senders: std::collections::BTreeMap<String, Vec<String>> = req
        .platform_senders
        .iter()
        .map(|(k, v)| {
            let vals = v
                .iter()
                .map(|s| s.trim().to_lowercase())
                .filter(|s| !s.is_empty())
                .collect();
            (k.clone(), vals)
        })
        .collect();
    db::set_setting_map(&st.db, db::keys::PLATFORM_SENDERS, &senders, by)?;
    db::set_setting(
        &st.db,
        db::keys::FORWARD_ENFORCE,
        if req.forward_enforce { "1" } else { "0" },
        by,
    )?;

    // 對應改了要立刻生效在既有信件上，否則管理員填完看不到任何變化，
    // 也就無從判斷自己填對了沒。
    //
    // **只重判 platform 為 NULL 的**：已經歸屬的不動。改對應不該讓一封信
    // 從某個人的驗證碼分頁憑空消失。
    let known = platforms::list(&st.cfg.domain_set_dir);
    let mailboxes = db::get_setting_str_map(&st.db, db::keys::PLATFORM_MAILBOXES);
    let mut fixed = 0;
    for (id, sender, recipient) in db::mails_without_platform(&st.db)? {
        let mailbox = recipient.unwrap_or_default();
        if let Some(code) =
            platforms::classify(sender.as_deref(), &mailbox, &senders, &mailboxes, &known)
        {
            let _ = db::update_mail_platform(&st.db, id, Some(&code));
            fixed += 1;
        }
    }
    if fixed > 0 {
        tracing::info!("設定變更後重新判定，{fixed} 封信有了平台歸屬");
    }

    db::audit(
        &st.db,
        by,
        "settings_changed",
        Some(&format!("mode={} domains={}", req.sender_mode, domains.join(","))),
        client_ip(&headers).as_deref(),
    );
    Ok(Json(serde_json::json!({ "ok": true, "reclassified": fixed })))
}

// ── 成員管理（僅管理員）───────────────────────────────

#[derive(Serialize)]
struct MemberRow {
    id: String,
    label: String,
    role: String,
    platforms: Vec<String>,
    /// 這個人加了幾條白名單，以及他加了哪些。
    /// 只有 IP 與名稱，**不含查詢明細** —— 那份 admin 也拿不到。
    entries: Vec<db::AllowEntry>,
    passkey_count: i64,
}

async fn member_list(
    State(st): State<Shared>,
    session: Session,
) -> ApiResult<Json<Vec<MemberRow>>> {
    require_admin(&st, &session).await?;

    let grants = db::all_platform_grants(&st.db)?;
    let allow = db::list_allow(&st.db)?;

    let rows = db::list_users(&st.db)?
        .into_iter()
        .map(|u| MemberRow {
            platforms: grants
                .iter()
                .filter(|(uid, _)| uid == &u.id)
                .map(|(_, p)| p.clone())
                .collect(),
            entries: allow
                .iter()
                .filter(|e| e.added_by.as_deref() == Some(u.label()))
                .cloned()
                .collect(),
            passkey_count: db::credential_count(&st.db, &u.id).unwrap_or(0),
            label: u.label().to_string(),
            id: u.id,
            role: u.role,
        })
        .collect();
    Ok(Json(rows))
}

#[derive(Deserialize)]
struct RoleReq {
    role: String,
}

/// 升降角色。
///
/// 擋掉「拿掉最後一個 admin」—— 那會讓面板永久失去管理能力，
/// 而且沒有任何介面能救回來（只能進資料庫手改）。
async fn member_set_role(
    State(st): State<Shared>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<RoleReq>,
) -> ApiResult<Json<serde_json::Value>> {
    let me = require_admin(&st, &session).await?;
    let role = match req.role.as_str() {
        "admin" => "admin",
        "member" => "member",
        _ => return Err(AppError(anyhow::anyhow!("角色只能是 admin 或 member"))),
    };

    let target = db::get_user(&st.db, &id)?.context("查無此帳號")?;
    if target.is_admin() && role == "member" && db::admin_count(&st.db)? <= 1 {
        return Err(AppError(anyhow::anyhow!(
            "這是最後一個管理員，降權之後就沒有人能管理面板了"
        )));
    }

    db::set_user_role(&st.db, &id, role)?;
    db::audit(
        &st.db,
        Some(me.label()),
        "member_role_changed",
        Some(&format!("{} → {role}", target.label())),
        client_ip(&headers).as_deref(),
    );
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// 移除一個帳號。
///
/// 三道護欄，每一道擋的是不同的事：
///   1. **不能刪自己** —— 你會在下一次請求時被登出，而且多半是誤按。
///      真的要退出的話請另一個 admin 來刪。
///   2. **不能刪掉最後一個 admin** —— 跟降權同一個理由：面板會永久失去
///      管理能力，沒有任何介面能救。
///   3. 對方**新增的白名單一併移除** —— 移除一個人卻留著他授權的網路，
///      等於沒有移除。
async fn member_delete(
    State(st): State<Shared>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let me = require_admin(&st, &session).await?;
    if me.id == id {
        return Err(AppError(anyhow::anyhow!(
            "不能移除自己。要退出請讓另一位管理員操作。"
        )));
    }

    let target = db::get_user(&st.db, &id)?.context("查無此帳號")?;
    if target.is_admin() && db::admin_count(&st.db)? <= 1 {
        return Err(AppError(anyhow::anyhow!(
            "這是最後一個管理員，移除之後就沒有人能管理面板了"
        )));
    }

    let label = target.label().to_string();
    let removed = db::delete_user(&st.db, &id, &label)?;

    // 白名單被動過，nft set 要跟上，否則那些網路還能繼續用
    let active = nft::sync(&st.db, &st.cfg.clients_nft)?;

    db::audit(
        &st.db,
        Some(me.label()),
        "member_removed",
        Some(&format!(
            "{label} · 白名單 {} 筆 · 轉發 {} 筆",
            removed.entries, removed.recipients
        )),
        client_ip(&headers).as_deref(),
    );
    tracing::warn!(
        "已移除帳號 {label}，一併移除白名單 {} 筆、轉發登記 {} 筆，nft 現有 {active} 筆",
        removed.entries,
        removed.recipients
    );

    Ok(Json(serde_json::json!({
        "ok": true,
        "removed_entries": removed.entries,
        "removed_recipients": removed.recipients,
    })))
}

#[derive(Deserialize)]
struct PlatformReq {
    platform: String,
}

/// 授予平台。
///
/// ⚠️ 這只影響「誰看得到驗證碼、誰收得到轉發」。網路層不分平台 ——
/// 授權過的 IP 對所有平台的網域都有效。理由見 DECISIONS.md。
async fn member_grant(
    State(st): State<Shared>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<PlatformReq>,
) -> ApiResult<Json<serde_json::Value>> {
    let me = require_admin(&st, &session).await?;
    let target = db::get_user(&st.db, &id)?.context("查無此帳號")?;

    // 只接受目前實際存在的平台，避免打錯字產生一筆永遠不會生效的授權
    let known = platforms::list(&st.cfg.domain_set_dir);
    if !known.iter().any(|p| p.code == req.platform) {
        return Err(AppError(anyhow::anyhow!("沒有這個平台：{}", req.platform)));
    }

    db::grant_platform(&st.db, &id, &req.platform, me.label())?;
    db::audit(
        &st.db,
        Some(me.label()),
        "platform_granted",
        Some(&format!("{} ← {}", target.label(), req.platform)),
        client_ip(&headers).as_deref(),
    );
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn member_revoke(
    State(st): State<Shared>,
    session: Session,
    headers: HeaderMap,
    Path((id, platform)): Path<(String, String)>,
) -> ApiResult<Json<serde_json::Value>> {
    let me = require_admin(&st, &session).await?;
    let target = db::get_user(&st.db, &id)?.context("查無此帳號")?;
    let n = db::revoke_platform(&st.db, &id, &platform)?;
    db::audit(
        &st.db,
        Some(me.label()),
        "platform_revoked",
        Some(&format!("{} ✕ {platform}", target.label())),
        client_ip(&headers).as_deref(),
    );
    Ok(Json(serde_json::json!({ "ok": n > 0 })))
}

// ── 查詢明細 ──────────────────────────────────────────

/// 某個白名單條目最近查了哪些網域。
///
/// ⚠️ **只有加這條的人拿得到，admin 也不例外。**
///
/// 這是面板裡唯一會揭露「家人在看什麼」的地方。彙總數字（幾筆、最後一次
/// 何時）在列表上人人可見，那個足以回答「這個網路還在用嗎」；逐筆網域
/// 只回答「他在看什麼」，那不是管白名單需要知道的事。
///
/// 這個分級是刻意的，見 DECISIONS.md。要改之前先想清楚為什麼。
async fn allow_queries(
    State(st): State<Shared>,
    session: Session,
    Path(ip): Path<String>,
) -> ApiResult<Json<Vec<dnslog::Query>>> {
    let (_, name) = require_user(&st, &session).await?;

    let entry = db::list_allow(&st.db)?
        .into_iter()
        .find(|e| e.ip == ip)
        .context("查無此白名單條目")?;

    if entry.added_by.as_deref() != Some(name.as_str()) {
        return Err(AppError(anyhow::anyhow!(
            "只有新增這個網路的人看得到它的查詢明細"
        )));
    }

    Ok(Json(st.dns.recent(&ip, 20, db::now(), DNS_RECENT_SECS)))
}

// ── 自動續期 ──────────────────────────────────────────

/// 多久檢查一次。必須遠小於 `DNS_WINDOW_SECS`，否則檢查跑到時視窗已經
/// 被 prune 清空，還在用的網路會被誤判成閒置。
const RENEW_INTERVAL_SECS: u64 = 600;
/// 剩多久以內才續期。太早續等於「永不過期」，失去了 TTL 的意義；
/// 太晚續則可能在兩次檢查之間就掉了。
const RENEW_WHEN_LEFT_SECS: i64 = 86400;

/// 把「還在用」的白名單條目自動延長。
///
/// 判斷依據是 smartdns 的查詢活躍度：這個 IP 在視窗內有查過東西，就代表
/// 那個網路上還有裝置在用，值得留著。沒有查詢的就讓它照常到期 ——
/// 設計稿 1d 的「近 5 分鐘無查詢 · 到期後自動移除」講的就是這件事。
///
/// 續期用條目自己的 `ttl_days`，不是全域預設：那個天數是使用者授權當下
/// 對「這個網路」的判斷，續期不該偷偷改成別的值。
async fn renew_active(st: Shared) {
    let mut tick = tokio::time::interval(std::time::Duration::from_secs(RENEW_INTERVAL_SECS));
    // 第一拍立刻觸發，但此時視窗還是空的，不會誤續任何東西
    loop {
        tick.tick().await;
        let now = db::now();
        let entries = match db::list_allow(&st.db) {
            Ok(e) => e,
            Err(e) => {
                tracing::error!("自動續期讀取白名單失敗: {e:#}");
                continue;
            }
        };

        for e in entries {
            if e.expires_at - now > RENEW_WHEN_LEFT_SECS {
                continue;
            }
            if st.dns.stats(&e.ip, DNS_WINDOW_SECS, now).count == 0 {
                // 沒有查詢活動 = 不會自動續期 = 真的會到期。
                // 這才是值得提醒的情況；有活動的會自己續下去，提醒只是噪音。
                notify_expiring(&st, &e).await;
                continue;
            }
            match db::renew_allow(&st.db, &e.ip) {
                Ok(Some(_)) => {
                    db::audit(
                        &st.db,
                        None,
                        "allow_renewed",
                        Some(&format!("{} ttl={}d 仍有查詢活動", e.ip, e.ttl_days)),
                        None,
                    );
                    tracing::info!("自動續期 {} 延長 {} 天", e.ip, e.ttl_days);
                }
                Ok(None) => {}
                Err(err) => tracing::error!("自動續期 {} 失敗: {err:#}", e.ip),
            }
        }
    }
}

/// 提醒擁有者「這個授權快到期，而且沒有活動不會自動續期」。
///
/// 靠 `claim_expiry_notice` 去重：10 分鐘一輪 × 24 小時窗 = 不去重會提醒 144 次。
async fn notify_expiring(st: &Shared, e: &db::AllowEntry) {
    let Some(owner) = e.added_by.as_deref() else { return };
    let Ok(Some(user)) = db::find_user_by_email(&st.db, owner) else { return };

    match db::claim_expiry_notice(&st.db, &e.ip) {
        Ok(true) => {}
        Ok(false) => return, // 這一輪已經有人通知過了
        Err(err) => return tracing::error!("認領到期提醒失敗: {err:#}"),
    }

    let n = push::Notification {
        title: "授權快到期了".into(),
        body: format!(
            "{} 剩不到 24 小時，而且最近沒有連線 —— 到期後那個網路會失去存取。",
            e.label.as_deref().unwrap_or(&e.ip)
        ),
        // 每條白名單各自一則，不互相覆蓋
        tag: format!("expiry:{}", e.ip),
        url: "/".into(),
        code: None,
    };

    match db::push_subs_for_expiry(&st.db, &user.id) {
        Ok(subs) if !subs.is_empty() => fan_out(st, subs, &n).await,
        Ok(_) => {}
        Err(err) => tracing::error!("讀取到期提醒對象失敗: {err:#}"),
    }
}

/// 首次啟動時從既有的轉發登記回填「平台 → 收件信箱」對應。
///
/// 只回填推得出來的（local part 等於平台代號）。推不出來的留空，
/// 在畫面上顯示成「沒有對應到平台」等 admin 指派。
/// ⚠️ 猜錯比留空更糟 —— 猜錯的對應看起來像設定好了。
fn seed_platform_mailboxes(db: &db::Db, cfg: &Config) {
    if db::get_setting(db, db::keys::PLATFORM_MAILBOXES).ok().flatten().is_some() {
        return;
    }
    let known = platforms::list(&cfg.domain_set_dir);
    let Ok(recipients) = db::list_recipients(db) else { return };

    let mut boxes = std::collections::BTreeMap::new();
    for r in &recipients {
        if let Some(code) = platforms::of_mailbox(&r.mailbox, &known) {
            boxes.insert(code, r.mailbox.clone());
        }
    }
    let unmapped: Vec<_> = recipients
        .iter()
        .map(|r| r.mailbox.as_str())
        .filter(|m| !boxes.values().any(|v| v == m))
        .collect();
    if !unmapped.is_empty() {
        tracing::warn!(
            "以下收件信箱推不出平台，請到「轉發收件人」頁指派：{}",
            unmapped.join("、")
        );
    }
    let _ = db::set_setting_str_map(db, db::keys::PLATFORM_MAILBOXES, &boxes, None);
}

/// 環境變數只是**種子**：面板改過的設定不該被下一次重啟蓋回去，所以一律
/// 只在鍵不存在時寫入。測試的 state 也要走這裡，否則 ingest 讀到空清單。
fn seed_settings(db: &db::Db, cfg: &Config) {
    let _ = db::seed_setting(db, db::keys::SENDER_MODE, if cfg.mail_enforce_sender { "enforce" } else { "observe" });
    let _ = db::set_setting_list_if_absent(db, db::keys::SENDER_DOMAINS, &cfg.mail_allowed_senders);
    let _ = db::set_setting_list_if_absent(db, db::keys::CODE_KEYWORDS, &[]);
    let _ = db::set_setting_list_if_absent(db, db::keys::CODE_EXCLUDES, &[]);
    // 預設只轉發通過寄件者驗證的信
    let _ = db::seed_setting(db, db::keys::FORWARD_ENFORCE, "1");
    let _ = db::seed_setting(db, db::keys::MAIL_DOMAIN, &cfg.mail_domain);
    seed_platform_mailboxes(db, cfg);
}

/// 排除私有／保留位址。
fn is_public(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            !(v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_unspecified()
                || v4.octets()[0] == 100 && (v4.octets()[1] & 0xc0) == 64) // CGNAT 100.64/10
        }
        IpAddr::V6(v6) => {
            !(v6.is_loopback()
                || v6.is_unspecified()
                || (v6.segments()[0] & 0xfe00) == 0xfc00   // ULA fc00::/7
                || (v6.segments()[0] & 0xffc0) == 0xfe80)  // link-local fe80::/10
        }
    }
}

fn base64_url(b: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b)
}

// ── 入口 ──────────────────────────────────────────────

/// 前端產物。`web/` 用 Vite 建置後輸出到 `static/`，這裡整包打進執行檔 ——
/// 部署物仍然是一顆 binary，不必額外掛載目錄或跑第二個容器。
///
/// debug 建置時 rust-embed 改成從磁碟即時讀，所以 `bun run build` 之後
/// 不用重編 Rust 就能看到新畫面。
#[derive(rust_embed::Embed)]
#[folder = "static/"]
struct Assets;

/// 帶雜湊檔名的產物可以永久快取；`index.html` 絕對不行 ——
/// 它引用的正是那些雜湊檔名，快取住等於部署新版之後永遠看到舊的。
fn cache_control(path: &str) -> &'static str {
    if path.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    }
}

async fn serve_asset(uri: axum::http::Uri) -> Response {
    use axum::http::header;

    let path = uri.path().trim_start_matches('/');

    // API 路徑走到這裡代表沒有對應的路由。回 404 而不是 index.html ——
    // 讓前端拿到「這支端點不存在」而不是一坨 HTML 解析失敗。
    if path.starts_with("api/") {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "端點不存在" })),
        )
            .into_response();
    }

    // 單頁應用：找不到的路徑一律回 index.html，交給前端路由決定顯示什麼
    let path = if path.is_empty() { "index.html" } else { path };
    let (path, file) = match Assets::get(path) {
        Some(f) => (path, f),
        None => match Assets::get("index.html") {
            Some(f) => ("index.html", f),
            None => {
                // 前端還沒建置過。講清楚要跑什麼，而不是回一個空白的 404。
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Html("前端尚未建置。請在 app/control/web 執行 <code>bun run build</code>。"),
                )
                    .into_response();
            }
        },
    };

    (
        [
            (header::CONTENT_TYPE, file.metadata.mimetype().to_string()),
            (header::CACHE_CONTROL, cache_control(path).to_string()),
        ],
        file.data,
    )
        .into_response()
}

/// 路由表。抽成函式讓測試也能建構它 —— axum 對衝突的路由是在
/// 建構時 panic，用測試建一次就能擋住「兩條路徑撞在一起」的意外。
fn routes(state: Shared) -> Router {
    Router::new()
        .route("/api/status", get(status))
        .route("/api/join/start", post(join_start))
        .route("/api/join/verify", post(join_verify))
        .route("/api/join/invite", post(join_invite))
        .route("/api/register/start", post(register_start))
        .route("/api/register/finish", post(register_finish))
        .route("/api/login/any/start", post(login_any_start))
        .route("/api/login/any/finish", post(login_any_finish))
        .route("/api/login/start", post(login_start))
        .route("/api/login/finish", post(login_finish))
        .route("/api/logout", post(logout))
        .route("/api/allow", post(allow_add))
        .route("/api/allow/{ip}", delete(allow_remove).post(allow_rename))
        // 逐筆網域只給條目擁有者，admin 也不例外
        .route("/api/allow/{ip}/queries", get(allow_queries))
        .route("/api/audit", get(audit_list))
        .route("/api/settings", get(settings_get).put(settings_put))
        .route("/api/passkeys", get(passkey_list))
        .route("/api/push/key", get(push_key))
        .route("/api/push/subs", get(push_list).post(push_subscribe))
        .route("/api/push/subs/{id}", delete(push_unsubscribe))
        .route("/api/push/unsubscribe", post(push_unsubscribe_self))
        .route("/api/push/check", post(push_check))
        .route("/api/me/notify", get(notify_prefs_get).post(notify_prefs_set))
        .route("/api/me/forwarding", get(my_forwarding).post(my_forwarding_set))
        .route("/api/me/forwarding/resend", post(my_forwarding_resend))
        .route("/api/passkeys/{id}", delete(passkey_delete).post(passkey_rename))
        .route("/api/members", get(member_list))
        .route("/api/members/{id}", delete(member_delete))
        .route("/api/members/{id}/role", post(member_set_role))
        .route("/api/members/{id}/platforms", post(member_grant))
        .route("/api/members/{id}/platforms/{platform}", delete(member_revoke))
        .route("/api/dns-profile", get(dns_profile))
        // 信件可能夾帶圖片，放寬到 8 MB（其餘路由維持預設 2 MB）
        .route(
            "/api/mail/ingest",
            post(mail_ingest).layer(axum::extract::DefaultBodyLimit::max(8 * 1024 * 1024)),
        )
        .route("/api/mail", get(mail_list).delete(mail_delete_all))
        .route("/api/mail/inbox", get(mail_inbox))
        .route("/api/mail/{id}", get(mail_get).delete(mail_delete))
        .route("/api/recipients", get(recipient_list).post(recipient_add))
        .route("/api/recipients/{id}", delete(recipient_remove))
        .route("/api/recipients/{id}/enabled", post(recipient_toggle))
        .route("/api/recipients/{id}/verify", post(recipient_verify))
        .route("/api/mailboxes", post(mailbox_set))
        .route("/api/mailboxes/{mailbox}", delete(mailbox_purge))
        .route("/api/invite", post(invite_create).get(invite_list))
        .route("/api/invite/{email}", delete(invite_revoke))
        .layer(
            SessionManagerLayer::new(MemoryStore::default())
                .with_name("nfhh_session")
                .with_secure(true)
                .with_http_only(true),
        )
        .fallback(serve_asset)
        .with_state(state)
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cfg = Config::from_env()?;

    // 拒絕綁在非 loopback 位址（見檔頭）
    let bind_ip = cfg
        .bind
        .rsplit_once(':')
        .map(|(h, _)| h.trim_matches(['[', ']']).to_string())
        .unwrap_or_default();
    if let Ok(parsed) = bind_ip.parse::<IpAddr>() {
        if !parsed.is_loopback() {
            anyhow::bail!(
                "拒絕啟動：NFHH_BIND={} 不是 loopback 位址。\n\
                 本服務必須只能經由 Cloudflare Tunnel 抵達，否則 CF-Connecting-IP \n\
                 可被偽造，任何人都能把自己的 IP 寫進防火牆白名單。",
                cfg.bind
            );
        }
    }

    nft::preflight()?;
    let db = db::open(&cfg.db_path)?;

    // v6 遷移路徑（補填 Email、username 登入）已移除。這種帳號仍然可以用
    // 可探索登入進來，只是不能用 Email 登入、也對不上平台分權與轉發。
    match db::users_without_email(&db) {
        Ok(list) if !list.is_empty() => tracing::error!(
            "{} 個帳號沒有 email（{}）：無法用 Email 登入，面板會以舊 username 稱呼；請刪除後重新邀請",
            list.len(),
            list.join(", ")
        ),
        Ok(_) => {}
        Err(e) => tracing::warn!("檢查缺 email 的帳號失敗: {e:#}"),
    }

    // 還沒有任何使用者時，發一組一次性註冊碼並印在日誌
    if db::user_count(&db)? == 0 && !db::has_unused_bootstrap(&db)? {
        let token = Uuid::new_v4().to_string();
        db::create_bootstrap(&db, &token)?;
        tracing::warn!("");
        tracing::warn!("尚未建立任何帳號。首次註冊用的一次性碼：");
        tracing::warn!("    {token}");
        tracing::warn!("用完即失效。可用 docker logs nfhh-control 再次查看。");
        tracing::warn!("");
    }

    // 用當前規則重新抽取既有信件的驗證碼
    match db::mails_for_reextract(&db) {
        Ok(rows) => {
            let mut changed = 0;
            for (id, text, old) in rows {
                let new = mail::extract_code(&text);
                if new.as_deref() != old.as_deref() {
                    let _ = db::update_mail_code(&db, id, new.as_deref());
                    changed += 1;
                }
            }
            if changed > 0 {
                tracing::info!("依當前規則重新抽取，修正了 {changed} 封信的驗證碼");
            }
        }
        Err(e) => tracing::warn!("重新抽取驗證碼失敗: {e:#}"),
    }

    // v6 之前的信件沒有平台歸屬，而驗證碼分頁是靠平台過濾的 ——
    // 不回填的話那些信對所有人都會消失。啟動時補一次。
    {
        let known = platforms::list(&cfg.domain_set_dir);
        match db::mails_missing_platform(&db) {
            Ok(rows) => {
                let mut filled = 0;
                for (id, mailbox) in rows {
                    if let Some(p) = platforms::of_mailbox(&mailbox, &known) {
                        let _ = db::update_mail_platform(&db, id, Some(&p));
                        filled += 1;
                    }
                }
                if filled > 0 {
                    tracing::info!("回填了 {filled} 封既有信件的平台歸屬");
                }
            }
            Err(e) => tracing::warn!("回填信件平台失敗: {e:#}"),
        }
    }

    let webauthn = WebauthnBuilder::new(&cfg.rp_id, &Url::parse(&cfg.origin)?)?
        .rp_name("OTT Household")
        .build()?;

    // 首次上線時先把 clients.nft 既有條目收進 DB，再同步
    if let Err(e) = nft::import_legacy(&db, &cfg.clients_nft, cfg.default_ttl_days) {
        tracing::error!("匯入既有白名單失敗: {e:#}");
    }

    // 開機後把 DB 的白名單同步進 nft
    match nft::sync(&db, &cfg.clients_nft) {
        Ok(n) => tracing::info!("白名單已同步至 nft，{n} 筆生效中"),
        Err(e) => tracing::error!("白名單同步失敗: {e:#}"),
    }

    // 背景工作：定期清除過期條目並重新同步
    {
        let db = db.clone();
        let path = cfg.clients_nft.clone();
        // cfg 稍後會被搬進 AppState，先把要用的兩個數字抄走
        let (keep, max) = (cfg.audit_keep_days, cfg.audit_max_rows);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(300));
            loop {
                tick.tick().await;
                match db::purge_old_audit(&db, keep, max) {
                    Ok(n) if n > 0 => tracing::debug!("清掉 {n} 列逾期或超量的稽核"),
                    Ok(_) => {}
                    Err(e) => tracing::warn!("清理稽核失敗: {e:#}"),
                }
                if let Err(e) = nft::sync(&db, &path) {
                    tracing::error!("定期同步失敗: {e:#}");
                }
            }
        });
    }

    seed_settings(&db, &cfg);

    // 寄件者驗證的信任根。印出來，部署後的 canary 才有東西可以對照。
    tracing::info!("寄件者驗證只採信 authserv-id = {}", cfg.mail_authserv_id);

    let mailer = mailer::Mailer::new(
        cfg.resend_key.clone(),
        cfg.mail_from.clone(),
        cfg.invite_template.clone(),
    );
    if !mailer.enabled() {
        tracing::warn!("未設定 NFHH_RESEND_KEY，「用 Email 加入」將停用");
    }

    let bind = cfg.bind.clone();
    let audit_path = cfg.dns_audit.clone();
    let dns = Arc::new(dnslog::Window::new(DNS_WINDOW_SECS));
    let cf = cloudflare::Cloudflare::new(cfg.cf_account.clone(), cfg.cf_token.clone());
    if !cf.enabled() {
        tracing::warn!("未設定 Cloudflare 帳戶或 token，轉發收件人的驗證狀態將顯示「未查詢」");
    }

    let push = push::Push::new(&cfg.mail_from);
    let state = Arc::new(AppState {
        db,
        webauthn,
        cfg,
        mailer,
        cf,
        push,
        dns: dns.clone(),
        join_limiter: ratelimit::Limiter::new(
            JOIN_LIMIT_WINDOW_SECS,
            JOIN_LIMIT_PER_IP,
            JOIN_LIMIT_GLOBAL,
        ),
    });

    // 背景工作：tail smartdns 稽核檔，餵查詢視窗
    tokio::spawn(dnslog::tail(dns, audit_path));
    // 背景工作：把還在用的白名單條目自動續期
    tokio::spawn(renew_active(state.clone()));

    let app = routes(state);

    let listener = tokio::net::TcpListener::bind(&bind)
        .await
        .with_context(|| format!("無法綁定 {bind}"))?;
    tracing::info!("面板啟動於 http://{bind}（僅限 Cloudflare Tunnel 存取）");
    axum::serve(listener, app).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    // push::B64 的 encode 要有 Engine 這個 trait 在作用域裡
    use base64::Engine as _;
    use p256::elliptic_curve::sec1::ToEncodedPoint;

    /// 描述檔識別碼是 reverse-DNS，且不含任何寫死的網域。
    #[test]
    fn the_profile_identifier_is_derived_from_the_rp_id() {
        assert_eq!(profile_prefix("dnf.example.com"), "com.example.dnf.nfhh");
        assert_eq!(profile_prefix("panel.example.org"), "org.example.panel.nfhh");
        // 大小寫與多餘的點不該產生第二種識別碼
        assert_eq!(profile_prefix("DNF.Example.COM"), "com.example.dnf.nfhh");
        assert_eq!(profile_prefix(".dnf.example.com."), "com.example.dnf.nfhh");
    }

    /// ⚠️ 識別碼一變，已安裝的人不會被取代而是多一份描述檔，兩份搶 DNS 設定。
    /// 同一個 rp_id 必須永遠給出同一個值。
    #[test]
    fn the_profile_identifier_is_stable_and_never_empty() {
        let a = profile_prefix("dnf.example.com");
        assert_eq!(a, profile_prefix("dnf.example.com"));
        // 空的 PayloadIdentifier 會讓 iOS 拒收整份描述檔
        assert_eq!(profile_prefix(""), "nfhh");
        assert_eq!(profile_prefix("..."), "nfhh");
        assert!(!profile_prefix("localhost").is_empty());
    }

    fn hdrs(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(
                axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                v.parse().unwrap(),
            );
        }
        h
    }

    /// 信封收件位址優先於 To: 表頭。
    #[test]
    fn envelope_recipient_wins_over_to_header() {
        let h = hdrs(&[("x-nfhh-mailbox", "netflix@share.example.com")]);
        assert_eq!(
            routing_mailbox(&h, Some("someone-else@example.com")),
            "netflix@share.example.com"
        );
    }

    /// 沒有信封位址時退回 To: 表頭。
    #[test]
    fn falls_back_to_parsed_recipient() {
        assert_eq!(
            routing_mailbox(&HeaderMap::new(), Some("Netflix@Share.Example.com")),
            "netflix@share.example.com"
        );
    }

    /// 空的信封位址標頭不得蓋掉 To:。
    #[test]
    fn blank_header_does_not_shadow_fallback() {
        let h = hdrs(&[("x-nfhh-mailbox", "   ")]);
        assert_eq!(routing_mailbox(&h, Some("netflix@x.tw")), "netflix@x.tw");
    }

    /// 兩者皆無時回空字串，不套用任何預設名單。
    #[test]
    fn no_source_yields_empty() {
        assert_eq!(routing_mailbox(&HeaderMap::new(), None), "");
    }

    fn family() -> Vec<String> {
        vec!["a@example.com".into(), "b@example.com".into()]
    }

    /// 通過驗證又通過篩選器才扇出給家人。
    #[test]
    fn forwards_when_verified_and_actionable() {
        assert_eq!(forward_targets(false, true, family()), family());
    }

    /// 未通過寄件者驗證且設定為不轉發 —— 收掉扇出。
    #[test]
    fn withheld_mail_fans_out_to_nobody() {
        assert!(forward_targets(true, true, family()).is_empty());
    }

    /// 通過驗證但被篩選器擋下（廣告信）—— 一樣不扇出。
    /// 這是 code_keywords / code_excludes 對轉發生效的地方。
    #[test]
    fn filtered_mail_fans_out_to_nobody() {
        assert!(forward_targets(false, false, family()).is_empty());
    }

    /// 兩個條件同時不成立也只是空清單，不是錯誤。
    #[test]
    fn withheld_and_filtered_is_still_just_empty() {
        assert!(forward_targets(true, false, family()).is_empty());
    }

    /// 一封通過 DKIM 的 Netflix 信，主旨與內文照參數組。
    fn eml(subject: &str, body: &str, id: &str) -> axum::body::Bytes {
        axum::body::Bytes::from(format!(
            "From: Netflix <info@account.netflix.com>\r\n\
             To: netflix@share.example.com\r\n\
             Subject: {subject}\r\n\
             Message-ID: <{id}@netflix.com>\r\n\
             Authentication-Results: mx.cloudflare.net; dkim=pass header.d=netflix.com\r\n\
             Content-Type: text/plain; charset=utf-8\r\n\r\n{body}\r\n"
        ))
    }

    /// 篩選器真的接在 ingest 上 —— `forward_targets` 的單測只保證判斷本身，
    /// 這條保證 handler 有把 code_keywords / code_excludes 讀進去。
    #[tokio::test]
    async fn ingest_runs_the_code_filter_before_answering() {
        let mut cfg = Config::from_env().unwrap();
        cfg.mail_secret = "s3cret".into();
        let st = state_with(cfg);

        db::set_setting_list(&st.db, db::keys::CODE_KEYWORDS, &["驗證碼".into()], None).unwrap();
        db::set_setting_list(&st.db, db::keys::CODE_EXCLUDES, &["同戶".into()], None).unwrap();
        db::add_recipient(&st.db, "netflix@share.example.com", "a@example.com", None, "test").unwrap();

        let cases: &[(&str, &str, &str, bool)] = &[
            // 抽得到碼 —— 不必命中關鍵字
            ("Netflix sign-in code", "Your verification code is 471203.", "c1", true),
            // 主旨命中排除字 —— 一律擋下，即使內文有碼
            ("關於同戶裝置的說明", "驗證碼 000000 僅為範例。", "c2", false),
            // 抽不到碼但內文命中關鍵字 —— Netflix「暫時存取碼」信，碼在按鈕後面
            ("您的 Netflix 暫時存取碼", "請點選下方按鈕以取得驗證碼。", "c3", true),
            // 既沒碼也沒命中關鍵字 —— 廣告信
            ("New shows this week", "Check out what is new.", "c4", false),
        ];

        for (subject, body, id, should_forward) in cases {
            let h = hdrs(&[
                ("authorization", "Bearer s3cret"),
                ("x-nfhh-mailbox", "netflix@share.example.com"),
            ]);
            let out = mail_ingest(State(st.clone()), h, eml(subject, body, id))
                .await
                .unwrap()
                .0;

            assert_eq!(
                out["verified"], true,
                "{subject}：DKIM 應該過，否則測到的是驗證而不是篩選器"
            );
            assert_eq!(out["actionable"], *should_forward, "{subject}：篩選器判定不符");
            let n = out["forward_to"].as_array().unwrap().len();
            assert_eq!(
                n > 0,
                *should_forward,
                "{subject}：forward_to 有 {n} 人，預期{}",
                if *should_forward { "有人" } else { "沒人" }
            );
        }
    }

    /// 被擋下的信仍要存進 DB —— 管理收件匣是查「為什麼沒轉出去」的地方。
    #[tokio::test]
    async fn filtered_mail_is_still_recorded() {
        let mut cfg = Config::from_env().unwrap();
        cfg.mail_secret = "s3cret".into();
        let st = state_with(cfg);
        db::set_setting_list(&st.db, db::keys::CODE_EXCLUDES, &["同戶".into()], None).unwrap();

        let h = hdrs(&[
            ("authorization", "Bearer s3cret"),
            ("x-nfhh-mailbox", "netflix@share.example.com"),
        ]);
        let out = mail_ingest(State(st.clone()), h, eml("關於同戶裝置", "說明。", "x1"))
            .await
            .unwrap()
            .0;

        assert_eq!(out["actionable"], false);
        assert_eq!(db::recent_mails(&st.db, 60).unwrap().len(), 1, "信要留在管理收件匣");
    }

    /// Worker 要分得出「拒收」與「面板掛了」。以前兩者都是 400，Worker 只能
    /// 一律走 fail-open 的 FORWARD_MAP，拒收的信反而被無過濾轉發。
    #[tokio::test]
    async fn ingest_errors_carry_a_status_the_worker_can_classify() {
        let mut cfg = Config::from_env().unwrap();
        cfg.mail_secret = "s3cret".into();
        let st = state_with(cfg);

        let err = mail_ingest(State(st), hdrs(&[("authorization", "Bearer nope")]), eml("x", "y", "e1"))
            .await
            .unwrap_err();
        assert_eq!(err.into_response().status(), StatusCode::UNAUTHORIZED);

        // 密鑰留空是「面板刻意不收信」，不是拒收 —— 回 5xx，Worker 才會照
        // FORWARD_MAP 轉發，而不是把家人的碼收掉。
        let mut off = Config::from_env().unwrap();
        off.mail_secret = String::new();
        let err = mail_ingest(
            State(state_with(off)),
            hdrs(&[("authorization", "Bearer anything")]),
            eml("x", "y", "e2"),
        )
        .await
        .unwrap_err();
        assert_eq!(err.into_response().status(), StatusCode::SERVICE_UNAVAILABLE);

        assert_eq!(
            IngestError::Unprocessable("壞信".into()).into_response().status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert_eq!(
            IngestError::Internal(anyhow::anyhow!("db")).into_response().status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    /// 管理介面存的是 DB，ingest 以前讀的是啟動時的環境變數 —— 兩個權威來源。
    /// UI 移除的網域必須立刻失效，新增的必須立刻生效，不需要重啟。
    #[tokio::test]
    async fn sender_domains_edited_in_the_panel_take_effect_immediately() {
        let mut cfg = Config::from_env().unwrap();
        cfg.mail_secret = "s3cret".into();
        let st = state_with(cfg);
        let h = || hdrs(&[("authorization", "Bearer s3cret"), ("x-nfhh-mailbox", "netflix@share.example.com")]);
        let ingest = |id: &'static str| {
            let st = st.clone();
            async move {
                mail_ingest(State(st), h(), eml("code", "code 123456", id)).await.unwrap().0
            }
        };

        db::set_setting_list(&st.db, db::keys::SENDER_DOMAINS, &["example.org".into()], None).unwrap();
        assert_eq!(ingest("d1").await["verified"], false, "UI 移除 netflix.com 後要立刻不信任");

        db::set_setting_list(&st.db, db::keys::SENDER_DOMAINS, &["netflix.com".into()], None).unwrap();
        assert_eq!(ingest("d2").await["verified"], true);
    }

    /// 刪除是全域硬刪，規則必須跟清單一模一樣：同平台但清單看不到的信
    /// （非驗證碼信、enforce 下未通過的信）也不能靠猜 id 刪掉。
    #[tokio::test]
    async fn members_can_only_delete_mail_they_could_see() {
        let st = test_state();
        db::create_user_with_platforms(&st.db, "m", "m@x", "m@x", "member", Some("m@x"), &["netflix".into()]).unwrap();
        let ins = |id: &str, pf: Option<&str>, subject: &str, code: Option<&str>| {
            db::insert_mail(&st.db, Some(id), db::now(), None, None, Some(subject), code, None, None, &[], true, pf, None).unwrap()
        };
        ins("n", Some("netflix"), "code", Some("123456")); // 看得到
        ins("ad", Some("netflix"), "新片上架", None); // 同平台但不是驗證碼信：清單看不到
        ins("d", Some("disneyplus"), "code", Some("111111")); // 別的平台
        ins("x", None, "diag", None); // 管理診斷
        let ids: Vec<i64> = db::recent_mails(&st.db, 10).unwrap().into_iter().map(|m| m.id).collect();

        let session = test_session();
        session.insert(S_USER, &"m".to_string()).await.unwrap();
        session.insert(S_NAME, &"m@x".to_string()).await.unwrap();
        let mut deleted = 0;
        for id in &ids {
            let out = mail_delete(State(st.clone()), session.clone(), Path(*id)).await.map_err(|e| e.0).unwrap().0;
            if out["ok"] == true {
                deleted += 1;
            }
        }
        assert_eq!(deleted, 1, "只有清單看得到的那封能刪");

        // 只數數量的話，規則整個反過來也會通過 —— 得指認活下來的是哪三封。
        let left = db::recent_mails(&st.db, 10).unwrap();
        assert_eq!(left.len(), 3);
        assert!(
            left.iter().all(|m| m.code.as_deref() != Some("123456")),
            "被刪的必須是 netflix 那封驗證碼信"
        );
        let mut subjects: Vec<&str> = left.iter().filter_map(|m| m.subject.as_deref()).collect();
        subjects.sort_unstable();
        assert_eq!(subjects, ["code", "diag", "新片上架"], "留下的是清單看不到的那三封");
    }

    /// 單封端點是全文的唯一出口，授權要跟清單一模一樣（同一個 MailScope）。
    #[tokio::test]
    async fn mail_detail_reapplies_the_list_scope() {
        let st = test_state();
        db::create_user_with_platforms(&st.db, "m", "m@x", "m@x", "member", Some("m@x"), &["netflix".into()]).unwrap();
        let ins = |id: &str, pf: &str, subject: &str, code: Option<&str>, verified: bool| {
            db::insert_mail(&st.db, Some(id), db::now(), None, None, Some(subject), code, None, None, &[], verified, Some(pf), None).unwrap()
        };
        ins("n", "netflix", "code", Some("123456"), false);
        ins("ad", "netflix", "新片上架", None, true);
        ins("d", "disneyplus", "code", Some("111111"), true);
        // 兩封的主旨都是 code —— key 要連平台一起，否則查到的是別人平台那封
        let ids: std::collections::BTreeMap<(String, String), i64> = db::recent_mails(&st.db, 10)
            .unwrap()
            .into_iter()
            .map(|m| ((m.platform.unwrap(), m.subject.unwrap()), m.id))
            .collect();
        let id = |pf: &str, subject: &str| ids[&(pf.to_string(), subject.to_string())];

        let session = test_session();
        session.insert(S_USER, &"m".to_string()).await.unwrap();
        session.insert(S_NAME, &"m@x".to_string()).await.unwrap();
        let get = |id: i64| {
            let (st, s) = (st.clone(), session.clone());
            async move { mail_get(State(st), s, Path(id)).await.map_err(|e| e.0) }
        };

        let full = get(id("netflix", "code")).await.expect("自己平台、observe 模式：看得到");
        assert_eq!(full.0.code.as_deref(), Some("123456"), "單封回的是全文那顆 Mail");
        assert!(
            get(id("netflix", "新片上架")).await.is_err(),
            "同平台但清單看不到的信，單封也看不到"
        );
        assert!(get(id("disneyplus", "code")).await.is_err(), "別的平台");

        db::set_setting(&st.db, db::keys::SENDER_MODE, "enforce", None).unwrap();
        assert!(get(id("netflix", "code")).await.is_err(), "enforce 下未通過驗證的信不給看");
    }

    /// admin 走的是繞過 [`MailScope`] 的那條分支：收件匣列得出來的信
    /// —— 認不出平台的、別人平台的 —— 點「原始信件」也必須拿得到全文，
    /// 否則診斷頁上有列表卻讀不到內容。
    #[tokio::test]
    async fn admins_read_every_mail_the_inbox_lists() {
        let st = test_state();
        db::create_user_with_platforms(&st.db, "a", "a@x", "a@x", "admin", Some("a@x"), &[]).unwrap();
        let ins = |id: &str, pf: Option<&str>, subject: &str| {
            db::insert_mail(&st.db, Some(id), db::now(), None, None, Some(subject), None, None, None, &[], true, pf, None).unwrap()
        };
        ins("u", None, "認不出平台"); // 只在管理收件匣出現
        ins("d", Some("disneyplus"), "別人的平台");

        let session = test_session();
        session.insert(S_USER, &"a".to_string()).await.unwrap();
        session.insert(S_NAME, &"a@x".to_string()).await.unwrap();

        // 這位 admin 一個平台都沒被授權 —— 讀得到就證明走的是 admin 分支，
        // 不是碰巧通過了 MailScope
        assert!(db::platforms_for(&st.db, "a").unwrap().is_empty());
        let listed = mail_inbox(State(st.clone()), session.clone()).await.map_err(|e| e.0).unwrap().0;
        assert_eq!(listed.len(), 2, "收件匣兩封都列得出來");
        for m in listed {
            let got = mail_get(State(st.clone()), session.clone(), Path(m.id))
                .await
                .map_err(|e| e.0)
                .unwrap_or_else(|e| panic!("admin 讀不到「{:?}」：{e}", m.subject));
            assert_eq!(got.0.id, m.id);
        }
    }

    /// 邀請連結兌換之後，接上的是跟驗證碼**完全一樣**的那道關卡 ——
    /// 這條測試盯的就是「跳過驗證碼」不等於「跳過檢查」。
    #[tokio::test]
    async fn invite_link_opens_the_same_gate_as_a_code() {
        let st = test_state();
        db::invite_email(&st.db, "mei@example.com", "admin", &["netflix".into()]).unwrap();
        let token = invite::generate();
        let hash = invite::hash(&st.db, &token).unwrap();
        db::set_invite_token(&st.db, "mei@example.com", &hash).unwrap();

        let redeem = |t: String| {
            let st = st.clone();
            async move {
                join_invite(State(st), test_session(), hdrs(&[]), Json(InviteTokenReq { token: t }))
                    .await
                    .map_err(|e| e.0)
            }
        };

        let out = redeem(token.clone()).await.unwrap().0;
        assert_eq!(out.email, "mei@example.com");
        assert_eq!(out.platforms, vec!["netflix"], "畫面要講明註冊完會拿到什麼");
        assert!(
            db::otp_recently_verified(&st.db, "mei@example.com", otp::VERIFIED_WINDOW_SECS).unwrap(),
            "註冊那關讀的是這個旗標"
        );

        // 猜出來的權杖什麼都換不到
        let err = redeem("deadbeef".into()).await.unwrap_err();
        assert!(err.to_string().contains("無效"), "拿到的訊息是：{err}");

        // 註冊完成之後，同一條連結不能再換第二個帳號
        db::consume_invited_email(&st.db, "mei@example.com", "u1").unwrap();
        assert!(redeem(token).await.is_err());
    }

    /// 驗證碼／邀請連結證明的是「這個瀏覽器的人擁有信箱」，證明不能被
    /// 另一個瀏覽器拿去用 —— 否則攻擊者只要等真正持有人驗證完就能搶先建帳號。
    #[tokio::test]
    async fn email_proof_is_bound_to_the_session_that_earned_it() {
        let st = test_state();
        // 面板上得先有人（是 admin 發的邀請），否則 `register_start` 會走
        // 「建立第一個帳號」那條路，根本碰不到信箱驗證這道關卡。
        db::create_user_with_platforms(&st.db, "admin", "admin@x", "admin@x", "admin", Some("admin@x"), &[]).unwrap();
        db::invite_email(&st.db, "mei@example.com", "admin", &[]).unwrap();
        let token = invite::generate();
        db::set_invite_token(&st.db, "mei@example.com", &invite::hash(&st.db, &token).unwrap()).unwrap();
        // 另一位也被邀請的人 —— 他手上會有一份**別的信箱**的證明
        db::invite_email(&st.db, "yu@example.com", "admin", &[]).unwrap();
        let other = invite::generate();
        db::set_invite_token(&st.db, "yu@example.com", &invite::hash(&st.db, &other).unwrap()).unwrap();

        let redeem = |s: Session, t: String| {
            let st = st.clone();
            async move {
                let _ = join_invite(State(st), s, hdrs(&[]), Json(InviteTokenReq { token: t }))
                    .await
                    .map_err(|e| e.0)
                    .unwrap();
            }
        };
        let victim = test_session();
        let neighbour = test_session();
        redeem(victim.clone(), token).await;
        redeem(neighbour.clone(), other).await;

        let start = |s: Session| {
            let st = st.clone();
            async move {
                register_start(
                    State(st), s, hdrs(&[]),
                    Json(RegisterStart { email: Some("mei@example.com".into()), bootstrap_token: None, nickname: None }),
                )
                .await
                .map_err(|e| e.0)
            }
        };

        let err = start(test_session()).await.unwrap_err();
        assert!(err.to_string().contains("在這個瀏覽器"), "拿到的訊息是：{err}");

        // 證明不是「有沒有」而是「是哪個信箱」：換過另一個位址的 session
        // 一樣進不來，否則把檢查放寬成「有證明就好」也能矇混過去。
        let err = start(neighbour).await.unwrap_err();
        assert!(err.to_string().contains("在這個瀏覽器"), "拿到的訊息是：{err}");

        let _ = start(victim).await.expect("驗證過的那個 session 要能拿到 challenge");
    }

    /// 邀請連結那條路綁定了，驗證碼這條路也要 —— 兩支端點是同一道關卡的
    /// 兩個入口，只補其中一個等於沒補。
    #[tokio::test]
    async fn email_proof_from_a_code_is_bound_to_the_session_too() {
        let st = test_state();
        // 同上：面板上先有人，`register_start` 才會走到信箱驗證那一關。
        db::create_user_with_platforms(&st.db, "admin", "admin@x", "admin@x", "admin", Some("admin@x"), &[]).unwrap();
        db::invite_email(&st.db, "mei@example.com", "admin", &[]).unwrap();
        db::put_otp(
            &st.db,
            "mei@example.com",
            &otp::hash(&st.db, "mei@example.com", "482913").unwrap(),
            otp::TTL_SECS,
        )
        .unwrap();

        // 收到碼的人在自己的瀏覽器輸入它
        let victim = test_session();
        let out = join_verify(
            State(st.clone()), victim.clone(), hdrs(&[]),
            Json(VerifyReq { email: "mei@example.com".into(), code: "482913".into() }),
        )
        .await
        .map_err(|e| e.0)
        .unwrap()
        .0;
        assert_eq!(out["ok"], true);

        let start = |s: Session| {
            let st = st.clone();
            async move {
                register_start(
                    State(st), s, hdrs(&[]),
                    Json(RegisterStart { email: Some("mei@example.com".into()), bootstrap_token: None, nickname: None }),
                )
                .await
                .map_err(|e| e.0)
            }
        };

        // 旁觀者知道信箱、也知道有人剛驗過，但證明不在他的 session 上
        let err = start(test_session()).await.unwrap_err();
        assert!(err.to_string().contains("在這個瀏覽器"), "拿到的訊息是：{err}");
        let _ = start(victim).await.expect("輸入驗證碼的那個 session 要能拿到 challenge");
    }

    /// 設定頁的欄位就是前後端的契約。少一個欄位不會有人報錯 ——
    /// 畫面上按得動、存檔回 200、重讀又跳回原樣（`forward_enforce` 真的
    /// 這樣漏過一版）。這條把兩個方向都釘住。
    #[test]
    fn settings_round_trip_keeps_the_forward_switch() {
        let body = serde_json::json!({
            "sender_mode": "observe",
            "sender_domains": ["netflix.com"],
            "code_keywords": [],
            "code_excludes": [],
            "platform_senders": {},
            "forward_enforce": false,
        });
        let parsed: Settings = serde_json::from_value(body.clone()).expect("前端送的要收得下");
        assert!(!parsed.forward_enforce, "關掉就是關掉，不能被預設值蓋回去");
        assert_eq!(
            serde_json::to_value(&parsed).unwrap(),
            body,
            "回給前端的欄位要跟收進來的一模一樣，少一個開關就會永遠顯示預設值"
        );

        // 拼錯欄位名不能安靜地被吞掉
        let typo = serde_json::json!({
            "sender_mode": "observe", "sender_domains": [], "code_keywords": [],
            "code_excludes": [], "platform_senders": {}, "forward_enforc": false,
        });
        assert!(serde_json::from_value::<Settings>(typo).is_err());
    }

    /// 登記邀請時就把轉發信箱一起建好 —— 對方註冊完立刻收得到碼，
    /// 不必等 admin 再回來設一次。
    #[test]
    fn registering_an_invite_creates_the_forwarding_rows() {
        let st = test_state();
        // seed_settings 在建 state 時就把 MAIL_DOMAIN 種進 DB 了，這裡讀到的
        // 一定是 Some —— 不必再像「預設網域」欄位那樣退回 Config 的種子值。
        let domain = db::get_setting(&st.db, db::keys::MAIL_DOMAIN).unwrap().unwrap();

        for code in ["netflix", "disneyplus"] {
            db::add_recipient(&st.db, &format!("{code}@{domain}"), "mei@x.tw", None, "admin")
                .unwrap();
        }

        let rows = db::recipients_for_address(&st.db, "mei@x.tw").unwrap();
        assert_eq!(rows.len(), 2, "兩個平台各一筆");
        assert!(rows.iter().all(|r| r.enabled), "預設開啟");
    }

    /// 重複登記同一個位址不該產生第二筆，而且會恢復啟用 ——
    /// 「若已存在則可忽略」靠 ON CONFLICT 拿到，不必自己判斷。
    #[test]
    fn re_registering_revives_instead_of_duplicating() {
        let st = test_state();
        db::add_recipient(&st.db, "netflix@share.example.com", "mei@x.tw", None, "admin").unwrap();
        db::set_recipients_enabled_for_address(&st.db, "mei@x.tw", false).unwrap();

        db::add_recipient(&st.db, "netflix@share.example.com", "mei@x.tw", None, "admin").unwrap();

        let rows = db::recipients_for_address(&st.db, "mei@x.tw").unwrap();
        assert_eq!(rows.len(), 1, "不該產生第二筆");
        assert!(rows[0].enabled, "重新登記要恢復啟用");
    }

    /// 使用者的總開關一次切掉名下所有 mailbox，而且碰不到別人的。
    #[test]
    fn the_self_switch_covers_every_mailbox_but_only_your_own() {
        let st = test_state();
        db::add_recipient(&st.db, "netflix@share.example.com", "mei@x.tw", None, "admin").unwrap();
        db::add_recipient(&st.db, "disneyplus@share.example.com", "mei@x.tw", None, "admin").unwrap();
        db::add_recipient(&st.db, "netflix@share.example.com", "ann@x.tw", None, "admin").unwrap();

        let n = db::set_recipients_enabled_for_address(&st.db, "mei@x.tw", false).unwrap();
        assert_eq!(n, 2, "名下兩個 mailbox 都要切到");

        assert!(db::recipients_for_address(&st.db, "mei@x.tw")
            .unwrap()
            .iter()
            .all(|r| !r.enabled));
        assert!(
            db::recipients_for_address(&st.db, "ann@x.tw").unwrap()[0].enabled,
            "不該動到別人的"
        );
    }

    /// 關掉之後就不該再出現在 Worker 拿到的轉發名單裡 ——
    /// 這是整個自助開關唯一真正要成立的事。
    #[test]
    fn turning_your_forwarding_off_removes_you_from_routing() {
        let st = test_state();
        db::add_recipient(&st.db, "netflix@share.example.com", "mei@x.tw", None, "admin").unwrap();
        db::add_recipient(&st.db, "netflix@share.example.com", "ann@x.tw", None, "admin").unwrap();

        db::set_recipients_enabled_for_address(&st.db, "mei@x.tw", false).unwrap();

        let routed = db::enabled_recipients_for(&st.db, "netflix@share.example.com").unwrap();
        assert_eq!(routed, vec!["ann@x.tw"]);
    }

    fn test_state() -> Shared {
        state_with(Config::from_env().unwrap())
    }

    /// 寄信服務**停用**的 state。`cfg.resend_key` 一律忽略 —— 開發機的
    /// shell 匯出了 NFHH_RESEND_KEY 時，其他測試不該因此換一條路走。
    fn state_with(cfg: Config) -> Shared {
        state_with_key(cfg, "")
    }

    /// 寄信服務啟用的 state。未受邀的位址在 `join_not_invited` 就結束，
    /// 走不到真的要連外的 `send_code` —— 稽核那一列卻已經寫進去了。
    fn state_with_mailer() -> Shared {
        state_with_key(Config::from_env().unwrap(), "dummy")
    }

    /// `set_var` 在 edition 2024 是 unsafe 且會干擾並行測試，改設定就直接組 Config。
    fn state_with_key(cfg: Config, resend_key: &str) -> Shared {
        let webauthn = WebauthnBuilder::new("localhost", &Url::parse("http://localhost").unwrap())
            .unwrap()
            .build()
            .unwrap();
        let db = db::test_db();
        seed_settings(&db, &cfg);
        Arc::new(AppState {
            db,
            webauthn,
            cfg,
            mailer: mailer::Mailer::new(resend_key.into(), "a@b.c".into(), "tpl".into()),
            push: push::Push::new("a@b.c"),
            cf: cloudflare::Cloudflare::new(String::new(), String::new()),
            dns: Arc::new(dnslog::Window::new(DNS_WINDOW_SECS)),
            join_limiter: ratelimit::Limiter::new(
                JOIN_LIMIT_WINDOW_SECS,
                JOIN_LIMIT_PER_IP,
                JOIN_LIMIT_GLOBAL,
            ),
        })
    }

    fn uri(p: &str) -> axum::http::Uri {
        p.parse().unwrap()
    }

    /// 單頁應用：前端路由的路徑在後端沒有對應的檔案，必須回 index.html，
    /// 否則使用者重新整理任何一頁都會 404。
    #[tokio::test]
    async fn unknown_paths_fall_back_to_the_spa() {
        let res = serve_asset(uri("/admin/members")).await;
        assert_eq!(res.status(), StatusCode::OK);
        let ct = res.headers().get("content-type").unwrap().to_str().unwrap();
        assert!(ct.starts_with("text/html"), "SPA fallback 要回 HTML，拿到 {ct}");
    }

    /// 但 /api/ 底下不能套用同一條規則 —— 前端拿到一坨 HTML 會在
    /// JSON 解析時炸開，錯誤訊息會完全誤導人。
    #[tokio::test]
    async fn unknown_api_paths_return_json_404() {
        let res = serve_asset(uri("/api/does-not-exist")).await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
        let ct = res.headers().get("content-type").unwrap().to_str().unwrap();
        assert!(ct.starts_with("application/json"), "拿到 {ct}");
    }

    /// 通知裡放的是碼本身，所以它跟的是**面板顯示**那條規則，
    /// 不是轉發那條。兩者分岔時推錯邊的後果是「通知有碼、
    /// 點進面板什麼都沒有」。
    #[test]
    fn push_visibility_follows_the_display_rule_not_the_forward_rule() {
        // (顯示策略, 寄件者通過驗證嗎, 該不該推)
        let cases: &[(&str, bool, bool)] = &[
            ("observe", false, true),  // 觀察期：面板照樣顯示 → 推
            ("observe", true, true),
            ("off", false, true),      // 不驗證：全部當通過 → 推
            ("enforce", false, false), // 收掉：面板看不到 → 不該推
            ("enforce", true, true),
        ];
        for (mode, verified, want) in cases {
            // 打的是 mail_ingest 真正用的那支判斷式。在測試裡重寫一遍規則，
            // 盯的就只是那份重寫，生產程式碼改壞了也照樣綠。
            assert_eq!(mode_allows(mode, Some(*verified)), *want, "mode={mode} verified={verified}");
        }
    }

    /// 卡片替連結畫平台品牌，等於替它背書。host 不在該平台網域清單內、
    /// 或寄件者根本沒通過驗證，就不能有那顆按鈕。host 的判讀交給 `url` crate ——
    /// 手寫的 parser 已經被 `\\@` 繞過一次（瀏覽器把 `\\` 當 `/`）。
    #[test]
    fn only_verified_links_on_platform_domains_get_the_branded_button() {
        let nf = vec!["netflix.com".to_string(), "nflxext.com".to_string()];
        assert!(brand_link_allowed(Some(true), "https://www.netflix.com/account/access", &nf));
        assert!(brand_link_allowed(Some(true), "https://NETFLIX.com:443/x", &nf));
        assert!(!brand_link_allowed(Some(false), "https://www.netflix.com/account/access", &nf), "未驗證");
        assert!(!brand_link_allowed(None, "https://www.netflix.com/x", &nf), "舊信無驗證資訊也不背書");
        assert!(!brand_link_allowed(Some(true), "https://netflix.com.evil.example/x", &nf));
        assert!(!brand_link_allowed(Some(true), "https://netflix.com@evil.example/x", &nf), "userinfo");
        assert!(!brand_link_allowed(Some(true), "https://evil.example\\@netflix.com/x", &nf), "反斜線：瀏覽器的 host 是 evil.example");
        assert!(!brand_link_allowed(Some(true), "https://evil.example/?u=netflix.com", &nf));
        assert!(!brand_link_allowed(Some(true), "https://nétflix.com/x", &nf), "IDN 同形字：host 會變 punycode，對不上");
        assert!(!brand_link_allowed(Some(true), "http://www.netflix.com/x", &nf), "只接受 https");
        assert!(!brand_link_allowed(Some(true), "https://.netflix.com/x", &nf), "前導點會過 ends_with，卡片卻印出 .netflix.com");
        assert!(!brand_link_allowed(Some(true), "https://www.netflix.com./x", &nf), "尾點是另一個 origin");
    }

    /// 純函式測試只證明判斷式，不證明「handler 真的把連結拿掉了」。
    /// 這條走 handler：清單與單封都不得把不合格的連結交到前端手上。
    #[tokio::test]
    async fn the_handlers_withhold_links_that_have_not_earned_the_brand() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("netflix.list"), "# platform-name: Netflix\nnetflix.com\n").unwrap();
        let st = state_with(Config {
            domain_set_dir: dir.path().to_str().unwrap().to_string(),
            ..Config::from_env().unwrap()
        });
        db::create_user_with_platforms(&st.db, "m", "m@x", "m@x", "member", Some("m@x"), &["netflix".into()]).unwrap();
        // 沒有碼的信才走到那顆按鈕，所以靠關鍵字讓它進得了清單
        db::set_setting_list(&st.db, db::keys::CODE_KEYWORDS, &["存取碼".into()], None).unwrap();

        let ins = |subject: &str, link: &str, verified: bool| {
            db::insert_mail(&st.db, Some(subject), db::now(), None, None, Some(subject), None, None,
                None, &[link.to_string()], verified, Some("netflix"), None).unwrap()
        };
        ins("存取碼 A", "https://www.netflix.com/account/access", true);
        ins("存取碼 B", "https://netflix.com.evil.example/x", true);
        ins("存取碼 C", "https://www.netflix.com/account/access", false);

        let session = test_session();
        session.insert(S_USER, &"m".to_string()).await.unwrap();
        session.insert(S_NAME, &"m@x".to_string()).await.unwrap();

        let listed = mail_list(State(st.clone()), session.clone()).await.map_err(|e| e.0).unwrap().0;
        let link = |subject: &str| {
            listed.iter().find(|m| m.subject.as_deref() == Some(subject))
                .unwrap_or_else(|| panic!("清單少了「{subject}」")).primary_link.clone()
        };
        assert_eq!(link("存取碼 A").as_deref(), Some("https://www.netflix.com/account/access"));
        assert_eq!(link("存取碼 B"), None, "host 不在平台網域，清單不得帶連結");
        assert_eq!(link("存取碼 C"), None, "未通過寄件者驗證，清單不得帶連結");

        // 單封是另一條程式路徑（沒有 cache），得各自證明
        for (subject, want) in [("存取碼 A", true), ("存取碼 B", false), ("存取碼 C", false)] {
            let id = listed.iter().find(|m| m.subject.as_deref() == Some(subject)).unwrap().id;
            let got = mail_get(State(st.clone()), session.clone(), Path(id)).await.map_err(|e| e.0).unwrap().0;
            assert_eq!(got.primary_link.is_some(), want, "單封端點對「{subject}」的裁決要跟清單一致");
        }
    }

    /// 扇出對每個訂閱開一個 task 且沒有上限：一個 member 先堆幾千筆訂閱，
    /// 再寄一封信給自己，就能同時打開幾千條連線。這裡用本機假推送服務量
    /// 「同時在飛的請求數」。
    ///
    /// 假服務只接 TCP、不講 TLS：`push::audience` 只認 https，所以 endpoint
    /// 必須是 https（http 的會在 `vapid_header` 就失敗，一條連線都不會開，
    /// 量到的峰值永遠是 0）。要量的是「同時被接受的連線數」，握手則在
    /// 200ms 後隨 socket 一起斷 —— 那正好就是「慢或惡意的 endpoint」。
    #[tokio::test]
    async fn fan_out_never_exceeds_the_concurrency_cap() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let inflight = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let (i2, p2) = (inflight.clone(), peak.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((sock, _)) = listener.accept().await {
                let (i, p) = (i2.clone(), p2.clone());
                tokio::spawn(async move {
                    let n = i.fetch_add(1, Ordering::SeqCst) + 1;
                    p.fetch_max(n, Ordering::SeqCst);
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                    i.fetch_sub(1, Ordering::SeqCst);
                    drop(sock);
                });
            }
        });

        // 真實可用的金鑰材料：encrypt 要對 p256dh 做 ECDH，隨便塞會在送出前就失敗
        let ua = p256::SecretKey::random(&mut p256::elliptic_curve::rand_core::OsRng);
        let p256dh = push::B64.encode(ua.public_key().to_encoded_point(false).as_bytes());
        let auth = push::B64.encode([7u8; 16]);

        let st = test_state();
        let subs: Vec<db::PushSub> = (0..20)
            .map(|i| db::PushSub {
                id: i, user_id: "u".into(), endpoint: format!("https://{addr}/push"),
                p256dh: p256dh.clone(), auth: auth.clone(), label: None,
                created_at: 0, last_ok_at: None, fail_count: 0,
            })
            .collect();
        let n = push::Notification { title: "t".into(), body: "b".into(), tag: "netflix".into(), url: "/".into(), code: None };

        let started = std::time::Instant::now();
        fan_out(&st, subs, &n).await;

        // 剛好等於上限：前 8 筆在碰到閘門之前就都 spawn 出去了，而每條連線
        // 都被握住 200ms —— 少於 8 表示扇出根本沒真的送，多於 8 表示閘門沒關住
        assert_eq!(peak.load(Ordering::SeqCst), PUSH_FANOUT_CONCURRENCY, "峰值");
        assert!(started.elapsed() >= std::time::Duration::from_millis(500), "20 筆分 3 輪，扣掉排程誤差也該有 500ms");
    }

    /// 金鑰材料要是真的：長度先擋、base64 要解得開、p256dh 要是曲線上的點。
    #[test]
    fn push_key_material_must_be_real() {
        let ua = p256::SecretKey::random(&mut p256::elliptic_curve::rand_core::OsRng);
        let good = push::B64.encode(ua.public_key().to_encoded_point(false).as_bytes());
        let auth = push::B64.encode([1u8; 16]);
        assert!(push::valid_keys(&good, &auth));
        assert!(!push::valid_keys("not-base64!", "AAAA"));
        assert!(!push::valid_keys(&push::B64.encode([4u8; 10]), &auth), "長度不對");
        assert!(!push::valid_keys(&push::B64.encode([4u8; 65]), &auth), "65 bytes 但不在曲線上");
        assert!(!push::valid_keys(&push::B64.encode(ua.public_key().to_encoded_point(true).as_bytes()), &auth), "壓縮點不收");
        assert!(!push::valid_keys(&good, &push::B64.encode([1u8; 8])));
        assert!(!push::valid_keys(&"A".repeat(4096), &auth), "先擋字串長度，不先解碼");
    }

    /// ⚠️ 被移除的成員必須**當場**失去存取，不能等到容器重啟。
    ///
    /// session 存在記憶體，帳號刪掉之後那份 session 還活得好好的。
    /// 這個檢查以前只有 `require_admin` 做，所以 member 層級的動作
    /// （授權 IP、看驗證碼）在帳號被刪之後照樣做得到 —— 那正是
    /// 「移除成員」要阻止的事。
    #[tokio::test]
    async fn a_deleted_member_loses_access_immediately() {
        use tower_sessions::{MemoryStore, Session};

        let st = test_state();
        db::create_user_with_platforms(&st.db, "u1", "mei@x.tw", "mei", "member", Some("mei@x.tw"), &[]).unwrap();

        let session = Session::new(None, std::sync::Arc::new(MemoryStore::default()), None);
        session.insert(S_USER, "u1".to_string()).await.unwrap();
        session.insert(S_NAME, "mei@x.tw".to_string()).await.unwrap();

        assert!(require_user(&st, &session).await.is_ok(), "帳號還在就該通過");

        db::delete_user(&st.db, "u1", "mei@x.tw").unwrap();

        // session 完全沒動過 —— 這正是重點
        let err = require_user(&st, &session).await.err().expect("帳號沒了就不該通過");
        assert!(
            err.0.to_string().contains("帳號已不存在"),
            "錯誤要說得出原因，拿到的是：{}",
            err.0
        );
    }

    /// 不經 HTTP 層直接餵給 handler 的 session。每個測試自己開一個，
    /// 跨 session 的攻擊情境就用兩個。
    fn test_session() -> Session {
        Session::new(None, Arc::new(MemoryStore::default()), None)
    }

    /// 這把新 Passkey 要寫給誰，必須跟「誰在這個 session 上」一致。
    /// 任何一邊對不上，都代表 session 裡的目標被另一條流程改寫過。
    #[test]
    fn a_registration_may_only_land_on_the_session_owner() {
        let mine = PendingReg {
            user_id: "u1".into(), username: "a@x".into(), email: None, is_new: false,
            role: "member".into(), bootstrap_token: None, nickname: None,
        };
        assert!(check_registration_owner(Some("u1"), &mine).is_ok());
        assert!(check_registration_owner(Some("u2"), &mine).is_err(), "別人的目標");
        assert!(check_registration_owner(None, &mine).is_err(), "沒登入卻在加備援金鑰");

        let fresh = PendingReg { is_new: true, ..mine };
        assert!(check_registration_owner(None, &fresh).is_ok());
        assert!(check_registration_owner(Some("u1"), &fresh).is_err(), "登入中不能建新帳號");
    }

    /// 攻擊鏈的第一步是「啟動登入時舊的註冊 challenge 還在」。
    /// 清除必須在查帳號**之前**發生，查不到帳號也要清。
    #[tokio::test]
    async fn starting_a_login_wipes_any_pending_registration() {
        let st = test_state();
        let session = test_session();
        let (_, reg_state) = st
            .webauthn
            .start_passkey_registration(Uuid::new_v4(), "a@x", "a@x", None)
            .unwrap();
        session.insert(S_REG, &reg_state).await.unwrap();
        session
            .insert(S_REG_USER, &PendingReg {
                user_id: "u1".into(), username: "a@x".into(), email: None, is_new: false,
                role: "member".into(), bootstrap_token: None, nickname: None,
            })
            .await
            .unwrap();

        let _ = login_start(State(st.clone()), session.clone(), Json(EmailReq { email: "admin@x".into() })).await;

        assert!(session.get::<PasskeyRegistration>(S_REG).await.unwrap().is_none());
        assert!(session.get::<PendingReg>(S_REG_USER).await.unwrap().is_none());
    }

    /// 完整重播報告裡的攻擊鏈：member 先開「新增 Passkey」、再對 admin 的 Email
    /// 啟動登入、最後提交第一步拿到的註冊回應。憑證不得寫進 admin 列。
    #[tokio::test]
    async fn a_member_cannot_attach_a_passkey_to_the_admin_via_login_start() {
        use webauthn_authenticator_rs::{softpasskey::SoftPasskey, WebauthnAuthenticator};
        let st = test_state();
        let origin = Url::parse("http://localhost").unwrap();
        db::create_user_with_platforms(&st.db, "admin", "admin@x", "admin@x", "admin", Some("admin@x"), &[]).unwrap();
        db::create_user_with_platforms(&st.db, "mem", "mem@x", "mem@x", "member", Some("mem@x"), &[]).unwrap();

        // admin 要先有一把 passkey，login_start 才發得出 challenge
        let mut admin_key = WebauthnAuthenticator::new(SoftPasskey::new(true));
        let (ccr, reg) = st.webauthn.start_passkey_registration(Uuid::new_v4(), "admin@x", "admin@x", None).unwrap();
        let pk = st
            .webauthn
            .finish_passkey_registration(&admin_key.do_registration(origin.clone(), ccr).unwrap(), &reg)
            .unwrap();
        db::add_credential(&st.db, &base64_url(pk.cred_id().as_ref()), "admin", &serde_json::to_string(&pk).unwrap(), None).unwrap();

        // 攻擊者：已登入的 member
        let session = test_session();
        session.insert(S_USER, &"mem".to_string()).await.unwrap();
        session.insert(S_NAME, &"mem@x".to_string()).await.unwrap();
        let mut attacker_key = WebauthnAuthenticator::new(SoftPasskey::new(true));

        // 1. 新增 Passkey：拿到自己的註冊 challenge
        let ccr = register_start(
            State(st.clone()), session.clone(), hdrs(&[]),
            Json(RegisterStart { email: None, bootstrap_token: None, nickname: None }),
        )
        .await
        .map_err(|e| e.0)
        .unwrap()
        .0;
        // 2. 對 admin 的 Email 啟動登入
        let _ = login_start(State(st.clone()), session.clone(), Json(EmailReq { email: "admin@x".into() }))
            .await
            .map_err(|e| e.0)
            .unwrap();
        // 3. 提交第 1 步的註冊回應
        let cred = attacker_key.do_registration(origin, ccr).unwrap();
        let res = register_finish(State(st.clone()), session.clone(), hdrs(&[]), Json(cred)).await;

        let err = res.expect_err("跨流程的 finish 必須失敗");
        // 釘住失敗的**原因**：註冊狀態被 login_start 清掉了。
        // 只驗 is_err() 的話，任何不相干的壞掉都能讓這個測試假裝通過。
        assert!(err.0.to_string().contains("已失效"), "拿到的是：{}", err.0);
        assert_eq!(db::credentials_for(&st.db, "admin").unwrap().len(), 1, "admin 不得多出憑證");
        assert!(db::credentials_for(&st.db, "mem").unwrap().is_empty(), "也不該偷偷寫給 member 自己");
    }

    /// 遷移期結束：登入只認 email，不再退回 username。退路留著，等於一個
    /// 不需要信箱證明就能指定登入目標的入口。
    #[tokio::test]
    async fn login_no_longer_falls_back_to_username() {
        let st = test_state();
        db::create_user_with_platforms(&st.db, "u1", "alex", "alex", "member", Some("alex@example.com"), &[]).unwrap();

        let err = login_start(State(st.clone()), test_session(), Json(EmailReq { email: "alex".into() }))
            .await
            .map_err(|e| e.0)
            .unwrap_err();
        assert!(err.to_string().contains("查無此帳號"), "{err}");

        // 用 email 查得到（沒有 passkey 所以停在下一關，錯誤訊息不同）
        let err = login_start(State(st.clone()), test_session(), Json(EmailReq { email: "alex@example.com".into() }))
            .await
            .map_err(|e| e.0)
            .unwrap_err();
        assert!(err.to_string().contains("沒有已註冊的 passkey"), "{err}");
    }

    #[test]
    fn email_must_look_like_an_address_and_fit_rfc_5321() {
        assert!(valid_email("a@b.c"));
        assert!(!valid_email("no-at"));
        assert!(!valid_email("a b@c"));
        assert!(!valid_email(&format!("{}@x", "a".repeat(MAX_EMAIL_LEN))));
    }

    fn audit_rows(db: &db::Db) -> i64 {
        db.lock().unwrap().query_row("SELECT count(*) FROM audit", [], |r| r.get(0)).unwrap()
    }

    /// 公開的 join/start 每次失敗都寫一列稽核；沒有限流就是一台免費寫入機。
    #[tokio::test]
    async fn join_start_is_rate_limited_per_ip() {
        let st = state_with_mailer();
        let h = || hdrs(&[("cf-connecting-ip", "203.0.113.9")]);
        let mut last = None;
        for _ in 0..(JOIN_LIMIT_PER_IP + 1) {
            last = Some(join_start(State(st.clone()), h(), Json(EmailReq { email: "x@y.z".into() })).await.map_err(|e| e.0).unwrap_err());
        }
        assert!(last.unwrap().to_string().contains("太頻繁"));
        assert_eq!(
            audit_rows(&st.db),
            JOIN_LIMIT_PER_IP as i64,
            "放行的每一次都該寫一列，被限流的那次一列都不能寫"
        );
    }

    /// join/verify 也是公開端點，而且鎖住的碼每被戳一次就寫一列
    /// `join_code_locked` —— 不限流的話，一組作廢的碼就是一台寫入機。
    #[tokio::test]
    async fn join_verify_is_rate_limited_too() {
        let st = test_state();
        // 直接把次數推到上限：驗證碼本身不是這條測試的重點，
        // 而且用真的猜錯去燒次數會先把限流額度花掉。
        db::put_otp(&st.db, "x@y.z", "hash-good", 600).unwrap();
        st.db
            .lock()
            .unwrap()
            .execute(&format!("UPDATE email_otp SET attempts = {}", otp::MAX_ATTEMPTS), [])
            .unwrap();

        let h = || hdrs(&[("cf-connecting-ip", "203.0.113.11")]);
        let body = || VerifyReq { email: "x@y.z".into(), code: "123456".into() };
        let mut last = None;
        for _ in 0..JOIN_LIMIT_PER_IP {
            last = Some(
                join_verify(State(st.clone()), test_session(), h(), Json(body()))
                    .await
                    .map_err(|e| e.0)
                    .unwrap_err(),
            );
        }
        assert!(last.unwrap().to_string().contains("錯誤次數過多"), "前 N 次要真的跑到業務邏輯");
        let before = audit_rows(&st.db);
        assert_eq!(before, JOIN_LIMIT_PER_IP as i64, "每一次被鎖都寫一列");

        let err = join_verify(State(st.clone()), test_session(), h(), Json(body()))
            .await
            .map_err(|e| e.0)
            .unwrap_err();
        assert!(err.to_string().contains("太頻繁"), "{err}");
        assert_eq!(audit_rows(&st.db), before, "被限流的請求不能再寫稽核");
    }

    /// 加註備援 passkey 要先登入，額度該留給真正打不開門的人 ——
    /// 一家人躲在同一個 NAT 後面時，這條路不能被公開流量的額度拖下水。
    #[tokio::test]
    async fn adding_a_backup_passkey_while_logged_in_is_not_throttled() {
        let st = test_state();
        db::create_user_with_platforms(&st.db, "u1", "a@x.tw", "a@x.tw", "member", Some("a@x.tw"), &[])
            .unwrap();
        let h = || hdrs(&[("cf-connecting-ip", "203.0.113.12")]);
        for i in 0..(JOIN_LIMIT_PER_IP + 5) {
            let session = test_session();
            session.insert(S_USER, &"u1".to_string()).await.unwrap();
            session.insert(S_NAME, &"a@x.tw".to_string()).await.unwrap();
            let res = register_start(
                State(st.clone()),
                session,
                h(),
                Json(RegisterStart { email: None, bootstrap_token: None, nickname: None }),
            )
            .await;
            if let Err(e) = res {
                panic!("第 {} 次就被擋了: {}", i + 1, e.0);
            }
        }
    }

    /// register/start 與 join/invite 同樣不需要登入、同樣每次失敗寫一列稽核。
    /// 只擋 join/start 等於把洪水改個門牌就放進來 —— 而列數上限會讓這波洪水
    /// 在下一次清理時把真正的稽核軌跡整批擠掉。
    #[tokio::test]
    async fn register_start_and_join_invite_are_rate_limited_too() {
        // register/start：庫裡先有帳號，未受邀的位址才走得到 register_not_invited
        let st = test_state();
        db::create_user_with_platforms(&st.db, "u1", "a@x.tw", "a@x.tw", "admin", Some("a@x.tw"), &[])
            .unwrap();
        let h = || hdrs(&[("cf-connecting-ip", "203.0.113.7")]);
        let body = || RegisterStart {
            email: Some("nobody@x.tw".into()),
            bootstrap_token: None,
            nickname: None,
        };
        let mut last = None;
        for _ in 0..JOIN_LIMIT_PER_IP {
            last = Some(
                register_start(State(st.clone()), test_session(), h(), Json(body()))
                    .await
                    .map_err(|e| e.0)
                    .unwrap_err(),
            );
        }
        assert!(last.unwrap().to_string().contains("沒有被邀請"), "前 N 次要真的跑到業務邏輯");
        let before = audit_rows(&st.db);
        assert_eq!(before, JOIN_LIMIT_PER_IP as i64, "每一次未受邀都寫一列");

        let err = register_start(State(st.clone()), test_session(), h(), Json(body()))
            .await
            .map_err(|e| e.0)
            .unwrap_err();
        assert!(err.to_string().contains("太頻繁"), "{err}");
        assert_eq!(audit_rows(&st.db), before, "被限流的請求不能再寫稽核");

        // join/invite：亂猜的權杖每次都寫一列 invite_link_bad
        let st = test_state();
        let h = || hdrs(&[("cf-connecting-ip", "203.0.113.8")]);
        let body = || InviteTokenReq { token: "not-a-real-token".into() };
        let mut last = None;
        for _ in 0..JOIN_LIMIT_PER_IP {
            last = Some(
                join_invite(State(st.clone()), test_session(), h(), Json(body()))
                    .await
                    .map_err(|e| e.0)
                    .unwrap_err(),
            );
        }
        assert!(last.unwrap().to_string().contains("無效或已經用過"), "前 N 次要真的跑到業務邏輯");
        let before = audit_rows(&st.db);
        assert_eq!(before, JOIN_LIMIT_PER_IP as i64);

        let err = join_invite(State(st.clone()), test_session(), h(), Json(body()))
            .await
            .map_err(|e| e.0)
            .unwrap_err();
        assert!(err.to_string().contains("太頻繁"), "{err}");
        assert_eq!(audit_rows(&st.db), before, "被限流的請求不能再寫稽核");
    }

    /// 上面那條攻擊鏈其實停在 `clear_auth_flows`，走不到 owner 檢查。
    /// 這裡直接餵它該擋的情境：`register_start` 之後 session 的身分被換掉，
    /// 而註冊狀態原封不動 —— 沒有這道檢查，憑證就會寫給換上來的那個人。
    #[tokio::test]
    async fn a_registration_is_refused_when_the_session_identity_changed() {
        use webauthn_authenticator_rs::{softpasskey::SoftPasskey, WebauthnAuthenticator};
        let st = test_state();
        let origin = Url::parse("http://localhost").unwrap();
        db::create_user_with_platforms(&st.db, "admin", "admin@x", "admin@x", "admin", Some("admin@x"), &[]).unwrap();
        db::create_user_with_platforms(&st.db, "mem", "mem@x", "mem@x", "member", Some("mem@x"), &[]).unwrap();

        let session = test_session();
        session.insert(S_USER, &"mem".to_string()).await.unwrap();
        session.insert(S_NAME, &"mem@x".to_string()).await.unwrap();

        // 1. member 正常開始「新增 Passkey」，目標是自己
        let ccr = register_start(
            State(st.clone()), session.clone(), hdrs(&[]),
            Json(RegisterStart { email: None, bootstrap_token: None, nickname: None }),
        )
        .await
        .map_err(|e| e.0)
        .unwrap()
        .0;

        // 2. 換人 —— 不經任何清除，只有「誰在這個 session 上」變了
        session.insert(S_USER, &"admin".to_string()).await.unwrap();

        // 3. 提交第 1 步的註冊回應
        let mut key = WebauthnAuthenticator::new(SoftPasskey::new(true));
        let cred = key.do_registration(origin, ccr).unwrap();
        let err = register_finish(State(st.clone()), session.clone(), hdrs(&[]), Json(cred))
            .await
            .expect_err("註冊目標與登入者不符就不該通過");

        assert!(err.0.to_string().contains("不符"), "要擋在 owner 檢查，拿到的是：{}", err.0);
        assert!(db::credentials_for(&st.db, "admin").unwrap().is_empty(), "admin 不得多出憑證");
        assert!(db::credentials_for(&st.db, "mem").unwrap().is_empty(), "member 也不該拿到");
    }

    /// 對稱檢查：註冊那一側也要清得乾淨。登入流程留下的挑戰若活過
    /// `register_start`，下一次 finish 就有另一條流程的目標可讀。
    #[tokio::test]
    async fn starting_a_registration_wipes_any_pending_login() {
        let st = test_state();
        db::create_user_with_platforms(&st.db, "u1", "mei@x.tw", "mei", "member", Some("mei@x.tw"), &[]).unwrap();

        let session = test_session();
        session.insert(S_USER, &"u1".to_string()).await.unwrap();
        session.insert(S_NAME, &"mei@x.tw".to_string()).await.unwrap();
        // `clear_auth_flows` 按鍵清除、不看型別，這兩把放什麼都會被清掉
        session.insert(S_AUTH, &serde_json::json!("stale")).await.unwrap();
        session.insert(S_DISC, &serde_json::json!("stale")).await.unwrap();
        session
            .insert(S_LOGIN_USER, &PendingReg {
                user_id: "admin".into(), username: "admin@x".into(), email: None, is_new: false,
                role: "admin".into(), bootstrap_token: None, nickname: None,
            })
            .await
            .unwrap();

        let _ = register_start(
            State(st.clone()), session.clone(), hdrs(&[]),
            Json(RegisterStart { email: None, bootstrap_token: None, nickname: None }),
        )
        .await
        .map_err(|e| e.0)
        .unwrap();

        assert!(session.get::<serde_json::Value>(S_AUTH).await.unwrap().is_none());
        assert!(session.get::<serde_json::Value>(S_DISC).await.unwrap().is_none());
        assert!(session.get::<PendingReg>(S_LOGIN_USER).await.unwrap().is_none());
        // 清完之後才輪到自己的狀態進場
        assert!(session.get::<PendingReg>(S_REG_USER).await.unwrap().is_some());
        // 登入身分刻意留著 —— 加備援金鑰本來就要在登入狀態下做
        assert_eq!(current_user(&session).await.map(|(id, _)| id).as_deref(), Some("u1"));
    }

    /// 可探索登入也是一條獨立的狀態機，開始時同樣要把註冊那側清空。
    #[tokio::test]
    async fn starting_a_discoverable_login_wipes_any_pending_registration() {
        let st = test_state();
        let session = test_session();
        let (_, reg_state) = st
            .webauthn
            .start_passkey_registration(Uuid::new_v4(), "a@x", "a@x", None)
            .unwrap();
        session.insert(S_REG, &reg_state).await.unwrap();
        session
            .insert(S_REG_USER, &PendingReg {
                user_id: "u1".into(), username: "a@x".into(), email: None, is_new: false,
                role: "member".into(), bootstrap_token: None, nickname: None,
            })
            .await
            .unwrap();

        let _ = login_any_start(State(st.clone()), session.clone()).await.map_err(|e| e.0).unwrap();

        assert!(session.get::<PasskeyRegistration>(S_REG).await.unwrap().is_none());
        assert!(session.get::<PendingReg>(S_REG_USER).await.unwrap().is_none());
        // 清完之後才輪到自己的挑戰進場
        assert!(session.get::<DiscoverableAuthentication>(S_DISC).await.unwrap().is_some());
    }

    /// 白名單的授權對象必須是**那一戶的 IPv4**，不是連線來源。
    ///
    /// 面板走 Cloudflare Tunnel，看到的是開面板那台裝置連到 Cloudflare 用的
    /// 位址。手機開著 IPv6 時那是一個 /128，而閘門是精準比對單一位址 ——
    /// 授權它等於只放行這一台裝置的這個位址：同一戶走 IPv4 的電視照樣被擋，
    /// 而 SLAAC 臨時位址輪替後連這台自己都會失效。所以前端在瀏覽器端問出
    /// 公網 IPv4 帶進 `?ip=`，這裡釘住後端會採信它。
    #[tokio::test]
    async fn the_claimed_ipv4_wins_over_the_ipv6_connection_address() {
        use tower_sessions::{MemoryStore, Session};

        let st = test_state();
        db::create_user_with_platforms(&st.db, "u1", "mei@x.tw", "mei", "member", Some("mei@x.tw"), &[]).unwrap();
        db::upsert_allow(&st.db, "4.3.2.1", None, Some("mei@x.tw"), db::now() + 86400, 1).unwrap();

        let session = Session::new(None, std::sync::Arc::new(MemoryStore::default()), None);
        session.insert(S_USER, "u1".to_string()).await.unwrap();
        session.insert(S_NAME, "mei@x.tw".to_string()).await.unwrap();

        // 連線走 IPv6，但前端問出的是這一戶的 IPv4
        let h = hdrs(&[("cf-connecting-ip", "2001:db8:5c07:5ec7::1")]);
        let q = |ip: Option<&str>| Query(StatusQuery { ip: ip.map(str::to_string) });

        let out = status(State(st.clone()), session.clone(), h.clone(), q(Some("4.3.2.1")))
            .await
            .map_err(|e| e.0)
            .unwrap()
            .0;
        assert_eq!(out.my_ip.as_deref(), Some("4.3.2.1"));
        assert!(out.my_ip_allowed, "帶進來的 IPv4 在白名單裡，就該說已授權");

        // 沒帶的話退回連線來源 —— 那個 /128 不在白名單裡
        let out = status(State(st.clone()), session.clone(), h.clone(), q(None)).await.map_err(|e| e.0).unwrap().0;
        assert_eq!(out.my_ip.as_deref(), Some("2001:db8:5c07:5ec7::1"));
        assert!(!out.my_ip_allowed);

        // 私有位址不採信 —— 家裡的 192.168.x 加進白名單不會有任何效果
        let out = status(State(st.clone()), session.clone(), h.clone(), q(Some("192.168.1.20")))
            .await
            .map_err(|e| e.0)
            .unwrap()
            .0;
        assert_eq!(out.my_ip.as_deref(), Some("2001:db8:5c07:5ec7::1"));

        // ⚠️ 未登入時一律不採信：這支端點不需要登入就打得到，採信了它就變成
        //    「某個 IP 在不在白名單裡」的探測器。
        let anon = Session::new(None, std::sync::Arc::new(MemoryStore::default()), None);
        let out = status(State(st), anon, h, q(Some("4.3.2.1"))).await.map_err(|e| e.0).unwrap().0;
        assert_eq!(out.my_ip.as_deref(), Some("2001:db8:5c07:5ec7::1"));
        assert!(!out.my_ip_allowed, "未登入不得靠 ?ip= 問出白名單內容");
    }

    /// ⚠️ 別人加過的 IP 不是「延長授權」，而是改寫別人的條目：標籤、到期
    /// 時間、天數都會被蓋掉，到期提醒也被重設。member 一律擋下，所有權爭議
    /// 交給 admin。
    #[tokio::test]
    async fn a_member_cannot_rewrite_another_members_allow_entry() {
        let st = test_state();
        db::create_user_with_platforms(&st.db, "ua", "a@x", "a", "member", Some("a@x"), &[]).unwrap();
        db::create_user_with_platforms(&st.db, "ub", "b@x", "b", "member", Some("b@x"), &[]).unwrap();
        db::upsert_allow(&st.db, "4.3.2.1", Some("老家"), Some("a@x"), db::now() + 30 * 86400, 30).unwrap();

        let session = test_session();
        session.insert(S_USER, "ub".to_string()).await.unwrap();
        session.insert(S_NAME, "b@x".to_string()).await.unwrap();

        let err = allow_add(
            State(st.clone()),
            session,
            hdrs(&[]),
            Json(AllowReq {
                ip: Some("4.3.2.1".into()),
                label: Some("hijack".into()),
                ttl_days: Some(1),
            }),
        )
        .await
        .map_err(|e| e.0)
        .expect_err("別人名下的 IP 不該讓 member 改寫");
        assert!(err.to_string().contains("不是你新增的"), "拿到的是：{err}");

        let e = db::list_allow(&st.db).unwrap().into_iter().find(|e| e.ip == "4.3.2.1").unwrap();
        assert_eq!(
            (e.added_by.as_deref(), e.label.as_deref(), e.ttl_days),
            (Some("a@x"), Some("老家"), 30),
            "被拒絕的請求不得留下任何痕跡"
        );
    }

    /// 額度只在「全新的 IP」時扣，但那條路要真的擋得住 —— 額度滿了就不能
    /// 再授權沒看過的網路。
    #[tokio::test]
    async fn a_full_quota_rejects_a_new_ip() {
        let mut cfg = Config::from_env().unwrap();
        cfg.max_per_user = 1;
        let st = state_with(cfg);
        db::create_user_with_platforms(&st.db, "ua", "a@x", "a", "member", Some("a@x"), &[]).unwrap();
        db::upsert_allow(&st.db, "4.3.2.1", None, Some("a@x"), db::now() + 86400, 1).unwrap();

        let session = test_session();
        session.insert(S_USER, "ua".to_string()).await.unwrap();
        session.insert(S_NAME, "a@x".to_string()).await.unwrap();

        let err = allow_add(
            State(st.clone()),
            session,
            hdrs(&[]),
            Json(AllowReq { ip: Some("1.2.3.4".into()), label: None, ttl_days: Some(1) }),
        )
        .await
        .map_err(|e| e.0)
        .expect_err("額度滿了就不該再收新的 IP");
        assert!(err.to_string().contains("額度已滿"), "拿到的是：{err}");
        assert_eq!(db::list_allow(&st.db).unwrap().len(), 1, "被擋下的請求不得留下條目");
    }

    /// 前端呼叫的每個端點都必須接受它實際送出的方法。
    ///
    /// 這類 bug 從畫面上看不出來：`api.js` 的 `req()` 是「有 body 就 POST，
    /// 沒有就 GET」，一個不帶 body 的 POST 會**靜靜送成 GET** 換回 405
    /// （這在 v6 真的發生過，見 git log 的「可探索登入送成了 GET」）。
    ///
    /// 只看方法對不對，不管授權 —— 未登入時回 400/401 都算通過。
    /// 對照 `web/dev/check-routes.sh`，那支打真的伺服器，這支不必。
    #[tokio::test]
    async fn every_endpoint_accepts_the_method_the_frontend_sends() {
        use tower::ServiceExt;

        // (方法, 路徑)。跟 api.js 的呼叫一一對應。
        let cases: &[(&str, &str)] = &[
            // 不帶 body —— api.js 必須明寫 method: 'POST'
            ("POST", "/api/recipients/1/verify"),
            ("POST", "/api/mailboxes"),
            ("DELETE", "/api/mailboxes/x@y.tw"),
            ("GET", "/api/push/key"),
            ("GET", "/api/push/subs"),
            ("POST", "/api/push/subs"),
            ("DELETE", "/api/push/subs/1"),
            ("POST", "/api/push/unsubscribe"),
            ("POST", "/api/push/check"),
            ("GET", "/api/me/notify"),
            ("POST", "/api/me/notify"),
            ("GET", "/api/me/forwarding"),
            ("POST", "/api/me/forwarding"),
            // ⚠️ 這支不帶 body —— api.js 必須明寫 method: 'POST'
            ("POST", "/api/me/forwarding/resend"),
            // 全文只從這支出去（清單只有摘要），跟同路徑的 DELETE 併在一條路由上
            ("GET", "/api/mail/1"),
        ];

        for (method, path) in cases {
            let req = axum::http::Request::builder()
                .method(*method)
                .uri(*path)
                .header("content-type", "application/json")
                .body(axum::body::Body::from("{}"))
                .unwrap();
            let res = routes(test_state()).oneshot(req).await.unwrap();
            assert_ne!(
                res.status(),
                StatusCode::METHOD_NOT_ALLOWED,
                "{method} {path} 回了 405 —— 路由表與前端對不上"
            );
        }
    }

    /// PWA 的三個檔案必須用對的 content-type 送出，否則會**靜默失效**：
    /// manifest 型別不對時 iOS 不把它當 web app（於是沒有推送），
    /// service worker 型別不對時瀏覽器直接拒絕註冊。
    #[test]
    fn pwa_assets_are_served_with_usable_types() {
        for (path, want) in [
            ("sw.js", "javascript"),
            ("manifest.webmanifest", "manifest"),
            ("icon-192.png", "image/png"),
        ] {
            let Some(f) = Assets::get(path) else {
                panic!("{path} 不在產物裡 —— 前端要先 build 過");
            };
            let got = f.metadata.mimetype().to_string();
            assert!(got.contains(want), "{path} 的 content-type 是 {got}，應含 {want}");
        }
    }

    /// service worker **不能被快取住**。舊的那支會繼續攔推送，
    /// 而使用者沒有任何辦法察覺自己收到的是舊邏輯。
    #[test]
    fn the_service_worker_is_never_cached() {
        assert_eq!(cache_control("sw.js"), "no-cache");
        assert_eq!(cache_control("manifest.webmanifest"), "no-cache");
    }

    /// 帶雜湊的產物可以永久快取，index.html 絕對不行 ——
    /// 它引用的正是那些雜湊檔名，快取住等於部署新版後永遠看到舊的。
    #[test]
    fn only_hashed_assets_are_cached_forever() {
        assert!(cache_control("assets/index-abc123.js").contains("immutable"));
        assert_eq!(cache_control("index.html"), "no-cache");
        assert_eq!(cache_control("favicon.ico"), "no-cache");
    }

    /// axum 對衝突的路由是在建構時 panic，不是在收到請求時。
    /// 沒有這條測試，`/api/mail/inbox` 撞上 `/api/mail/{id}` 這種事
    /// 要等到部署後啟動才會發現。
    #[test]
    fn router_builds_without_conflicts() {
        let _ = routes(test_state());
    }

    /// 寄件者宣告的日期只當顯示用，太舊或在未來都改用現在。
    /// 「太舊」以保留期為界 —— 留不了 14 天以上的信，就不該顯示得比那更舊。
    #[test]
    fn claimed_dates_are_clamped_to_a_sane_window() {
        let now = 1_800_000_000;
        let keep = 14;
        assert_eq!(clamp_received(None, now, keep), now);
        assert_eq!(clamp_received(Some(now - 60), now, keep), now - 60);
        assert_eq!(clamp_received(Some(now + 10 * 365 * 86400), now, keep), now);
        assert_eq!(clamp_received(Some(now - 400 * 86400), now, keep), now);
        assert_eq!(
            clamp_received(Some(now - 13 * 86400), now, keep),
            now - 13 * 86400,
            "還在保留期內的日期照原樣顯示"
        );
        assert_eq!(
            clamp_received(Some(now - 15 * 86400), now, keep),
            now,
            "比保留期還舊的信根本不可能在庫裡，那日期是假的"
        );
        assert_eq!(clamp_received(Some(now + 1800), now, keep), now + 1800, "時鐘誤差一小時內接受");
    }
}
