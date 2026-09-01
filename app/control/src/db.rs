//! SQLite 存取層。白名單的唯一真實來源，nft set 只是它的投影。

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

pub type Db = Arc<Mutex<Connection>>;

pub fn open(path: &str) -> Result<Db> {
    let conn = Connection::open(path).with_context(|| format!("開啟資料庫失敗: {path}"))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    migrate(&conn)?;
    Ok(Arc::new(Mutex::new(conn)))
}

/// 版本化遷移，用 `PRAGMA user_version` 記錄進度。
fn migrate(conn: &Connection) -> Result<()> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;

    if version < 1 {
        migrate_v1(conn)?;
        conn.pragma_update(None, "user_version", 1)?;
    }
    if version < 2 {
        migrate_v2(conn)?;
        conn.pragma_update(None, "user_version", 2)?;
    }
    if version < 3 {
        migrate_v3(conn)?;
        conn.pragma_update(None, "user_version", 3)?;
    }
    if version < 4 {
        // 保留原始 HTML 供面板在沙箱 iframe 內呈現
        let has: bool = conn
            .prepare("SELECT 1 FROM pragma_table_info('mails') WHERE name = 'html'")?
            .exists([])?;
        if !has {
            conn.execute_batch("ALTER TABLE mails ADD COLUMN html TEXT;")?;
        }
        conn.pragma_update(None, "user_version", 4)?;
    }
    if version < 5 {
        migrate_v5(conn)?;
        conn.pragma_update(None, "user_version", 5)?;
    }
    if version < 6 {
        migrate_v6(conn)?;
        conn.pragma_update(None, "user_version", 6)?;
    }
    if version < 7 {
        migrate_v7(conn)?;
        conn.pragma_update(None, "user_version", 7)?;
    }
    if version < 8 {
        migrate_v8(conn)?;
        conn.pragma_update(None, "user_version", 8)?;
    }
    if version < 9 {
        migrate_v9(conn)?;
        conn.pragma_update(None, "user_version", 9)?;
    }
    if version < 10 {
        migrate_v10(conn)?;
        conn.pragma_update(None, "user_version", 10)?;
    }
    if version < 11 {
        migrate_v11(conn)?;
        conn.pragma_update(None, "user_version", 11)?;
    }
    Ok(())
}

/// v11：補上 v10 那批欄位。
///
/// ⚠️ **不要修改已經套用過的 migration。** `expiry_notified_at` 與
/// `cf_present` 原本是後來補進 `migrate_v10` 的 —— 但已經跑過 v10 的資料庫
/// `user_version` 已經是 10，那個區塊永遠不會再執行，欄位因此從缺，
/// 直到某支 SELECT 撞上 `no such column` 才會發現。
///
/// 這裡整批重跑一次。`add_column` 本來就會先查 `pragma_table_info`，
/// 欄位已存在時是 no-op，所以對兩種資料庫都安全。
fn migrate_v11(conn: &Connection) -> Result<()> {
    add_column(conn, "users", "notify_codes", "INTEGER NOT NULL DEFAULT 1")?;
    add_column(conn, "users", "notify_expiry", "INTEGER NOT NULL DEFAULT 0")?;
    add_column(conn, "allowlist", "expiry_notified_at", "INTEGER")?;
    add_column(conn, "mail_recipients", "cf_present", "INTEGER")?;
    Ok(())
}

/// v10：推送通知訂閱與兩顆開關。
///
/// 訂閱是**每台裝置一筆**，`endpoint` 天生唯一，直接拿它當去重鍵。
///
/// ⚠️ 存明文而非雜湊：`p256dh` 與 `auth` 是加密酬載的材料，雜湊過就沒用了。
///    訂閱不是憑據，掉了只代表推不到那台裝置。
fn migrate_v10(conn: &Connection) -> Result<()> {
    // 設計 3a 的兩顆推桿。預設只開「新驗證碼」—— 預設就吵會讓人整組關掉。
    add_column(conn, "users", "notify_codes", "INTEGER NOT NULL DEFAULT 1")?;
    add_column(conn, "users", "notify_expiry", "INTEGER NOT NULL DEFAULT 0")?;

    // 「授權快到期」的去重標記。續期檢查每 10 分鐘跑一次，而提醒視窗有
    // 24 小時 —— 不記已經通知過，同一條白名單會被提醒 144 次。
    // 續期成功時清回 NULL，下一輪到期時才能再提醒一次。
    add_column(conn, "allowlist", "expiry_notified_at", "INTEGER")?;

    // Cloudflare 有沒有這個位址。NULL = 沒查過 / 0 = 查過但沒有 / 1 = 有。
    //
    // ⚠️ 少了這一欄，「Cloudflare 沒有這個位址」會顯示成無害的「未查詢」——
    //    但前者的轉發一定會退信。最危險的狀態長得跟最無害的一樣。
    add_column(conn, "mail_recipients", "cf_present", "INTEGER")?;

    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS push_subscriptions (
            id         INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id    TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            -- 推送服務給的網址（FCM／Apple），天然唯一
            endpoint   TEXT NOT NULL UNIQUE,
            -- RFC 8291 加密要用的裝置公鑰與共享密鑰，base64url
            p256dh     TEXT NOT NULL,
            auth       TEXT NOT NULL,
            label      TEXT,
            created_at INTEGER NOT NULL,
            last_ok_at INTEGER,
            -- 連續失敗次數。404／410 當場刪除，這個給的是「一直逾時」那種
            fail_count INTEGER NOT NULL DEFAULT 0
        );
        CREATE INDEX IF NOT EXISTS idx_push_user ON push_subscriptions(user_id);
        "#,
    )?;
    Ok(())
}

/// v9：邀請函連結。
///
/// 登記 Email 之後順帶寄一封邀請函，信裡的連結按下去就能直接建 Passkey ——
/// 對方不必再回頭輸入一次自己的位址、再等一組驗證碼。連結證明的事情跟
/// 驗證碼一樣（「這個信箱是你的」），因為它只寄得到那個信箱。
///
/// 存的是權杖的 HMAC 而非權杖本身：這張表洩漏不該等於可以拿走別人的邀請。
/// 也因此連結只在登記的當下拿得到一次，之後要重發只能重新登記（換一把新的）。
fn migrate_v9(conn: &Connection) -> Result<()> {
    add_column(conn, "invited_emails", "token_hash", "TEXT")?;
    // 查詢一律走這個雜湊，位址是查完才知道的
    conn.execute_batch(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_invited_token
         ON invited_emails(token_hash) WHERE token_hash IS NOT NULL;",
    )?;
    Ok(())
}

/// v8：給驗證碼關鍵字一組預設值。
///
/// v6 種進去的是空陣列，而空陣列在新規則下代表「只有抽得到碼的信才進
/// 驗證碼分頁」—— 那會讓 Netflix 的「暫時存取碼」永遠看不到，
/// 因為它的碼在連結後面而不在信裡。
///
/// **只在這個設定從沒被人改過時才動。** seed 寫入時 `updated_by` 是 NULL，
/// UI 存檔時會填上管理員 —— 靠這個分辨「預設值」與「有人刻意設成這樣」。
/// 有人特意清空是合法的設定，不該被下一次升級填回去。
fn migrate_v8(conn: &Connection) -> Result<()> {
    const DEFAULTS: &str = r#"["驗證碼","存取碼","verification code","access code"]"#;
    conn.execute(
        "UPDATE settings SET value = ?2, updated_at = ?3
         WHERE key = ?1 AND updated_by IS NULL AND value IN ('[]', '')",
        params![keys::CODE_KEYWORDS, DEFAULTS, now()],
    )?;
    Ok(())
}

/// v7：登記邀請 Email 時就決定平台授權。
///
/// v6 的流程是「登記 → 對方註冊 → admin 再回去開平台」，中間有段空窗：
/// 家人註冊完看到的是空的驗證碼分頁，還得回頭問「怎麼什麼都沒有」。
/// 把授權挪到登記當下決定，註冊完成的那一刻就直接可用。
fn migrate_v7(conn: &Connection) -> Result<()> {
    // JSON 陣列。NULL = v7 之前登記的，註冊後不會自動獲得任何平台
    // —— 跟遷移前的行為一致。
    add_column(conn, "invited_emails", "platforms", "TEXT")?;
    Ok(())
}

/// SQLite 沒有 `ADD COLUMN IF NOT EXISTS`，靠查 table_info 自己擋。
/// 遷移必須能在既有資料庫上重複執行 —— 見 `migration_is_idempotent` 測試。
fn add_column(conn: &Connection, table: &str, column: &str, decl: &str) -> Result<()> {
    let has: bool = conn
        .prepare(&format!(
            "SELECT 1 FROM pragma_table_info('{table}') WHERE name = ?1"
        ))?
        .exists(params![column])?;
    if !has {
        conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {decl};"))?;
    }
    Ok(())
}

/// v6：Email 身分、平台分權、自動續期、面板可改的設定。
///
/// 帳號識別從 username 改成 email，但 **username 不動** ——
/// 它是 WebAuthn 的 user handle，也是既有稽核紀錄裡 actor 欄位的值。
/// 改掉會讓已註冊的 passkey 對不上、讓歷史稽核失去指涉對象。
fn migrate_v6(conn: &Connection) -> Result<()> {
    add_column(conn, "users", "email", "TEXT")?;

    // 部分索引：既有帳號的 email 是 NULL，不該互相衝突
    conn.execute_batch(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_users_email
         ON users(email) WHERE email IS NOT NULL;",
    )?;

    // 白名單條目記住自己的續期天數 —— 自動續期時才知道要延多久。
    // 既有條目沿用舊的固定 7 天，跟遷移前的行為一致。
    add_column(conn, "allowlist", "ttl_days", "INTEGER NOT NULL DEFAULT 7")?;
    add_column(conn, "allowlist", "renewed_at", "INTEGER")?;

    add_column(conn, "mails", "platform", "TEXT")?;
    // 沒抽到驗證碼的原因（命中排除字／無匹配），管理收件匣要顯示
    add_column(conn, "mails", "skip_reason", "TEXT")?;

    // Cloudflare Email Routing 的驗證狀態快取。cf_verified_at 為 NULL
    // 代表「尚未驗證」，但只有在 cf_checked_at 有值時這個結論才成立。
    add_column(conn, "mail_recipients", "cf_verified_at", "INTEGER")?;
    add_column(conn, "mail_recipients", "cf_checked_at", "INTEGER")?;

    conn.execute_batch(
        r#"
        -- admin 登記的邀請 Email。刻意不過期：家人可能隔幾個月才想起來要註冊，
        -- 撤銷是 admin 的明確動作，不是時間到了自己消失。
        CREATE TABLE IF NOT EXISTS invited_emails (
            email      TEXT PRIMARY KEY,   -- 一律小寫
            invited_by TEXT,
            invited_at INTEGER NOT NULL,
            revoked_at INTEGER,
            used_at    INTEGER,
            used_by    TEXT                -- users.id
        );

        -- Email 一次性驗證碼。只證明「這個信箱是你的」，通過後才建 passkey。
        -- 存雜湊而非明碼：這張表洩漏不該等於可以冒用任何人的信箱。
        CREATE TABLE IF NOT EXISTS email_otp (
            email       TEXT PRIMARY KEY,
            code_hash   TEXT NOT NULL,
            expires_at  INTEGER NOT NULL,
            attempts    INTEGER NOT NULL DEFAULT 0,
            sent_at     INTEGER NOT NULL,  -- 重寄冷卻的基準
            verified_at INTEGER
        );

        -- 成員 × 平台的授權矩陣。
        -- ⚠️ 只作用於「誰看得到驗證碼、誰收得到轉發」。網路層（nft set 與
        --    smartdns 的 domain-set）依然是全有全無 —— 一個 IP 進了白名單，
        --    所有平台的網域都會解到 proxy。分權管的是帳號存取，不是流量歸屬。
        CREATE TABLE IF NOT EXISTS user_platforms (
            user_id    TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            platform   TEXT NOT NULL,      -- 對應 domain-set 的檔名，如 netflix
            granted_by TEXT,
            granted_at INTEGER NOT NULL,
            PRIMARY KEY (user_id, platform)
        );

        -- 面板可改的設定。原本這些只能靠環境變數，改一次要重啟容器；
        -- 搬進來之後 UI 存檔即生效。環境變數退居「首次啟動的種子值」。
        CREATE TABLE IF NOT EXISTS settings (
            key        TEXT PRIMARY KEY,
            value      TEXT NOT NULL,      -- 純量或 JSON 陣列，由讀取端決定
            updated_by TEXT,
            updated_at INTEGER NOT NULL
        );
        "#,
    )?;
    Ok(())
}

/// v5：轉發收件人 + 寄件者驗證結果。
///
/// 收件人清單存在這裡，Worker 向面板查詢「這封該轉給誰」。
/// 刻意不跟 users 表綁定 —— 多數家人尚未註冊 passkey。
fn migrate_v5(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS mail_recipients (
            id       INTEGER PRIMARY KEY AUTOINCREMENT,
            -- 哪個信箱收到的信要轉給這個人，例如 netflix@share.example.com
            mailbox  TEXT NOT NULL,
            address  TEXT NOT NULL,
            label    TEXT,
            -- 關掉而不是刪掉：之後要恢復轉發時不必重打一次位址
            enabled  INTEGER NOT NULL DEFAULT 1,
            added_by TEXT,
            added_at INTEGER NOT NULL,
            UNIQUE(mailbox, address)
        );
        CREATE INDEX IF NOT EXISTS idx_recipients_mailbox ON mail_recipients(mailbox);
        "#,
    )?;

    // 寄件者驗證結果留在信件上，面板才能把未通過的信標出來
    let has: bool = conn
        .prepare("SELECT 1 FROM pragma_table_info('mails') WHERE name = 'verified'")?
        .exists([])?;
    if !has {
        conn.execute_batch("ALTER TABLE mails ADD COLUMN verified INTEGER;")?;
    }
    Ok(())
}

/// v3：驗證碼信件。集中顯示以便協調「這次由誰驗證」。
fn migrate_v3(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS mails (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            -- 用信件本身的 Message-ID 去重，Worker 重送不會產生重複紀錄
            message_id  TEXT UNIQUE,
            received_at INTEGER NOT NULL,
            sender      TEXT,
            recipient   TEXT,
            subject     TEXT,
            code        TEXT,   -- 抽取出的驗證碼，抽不到則為 NULL
            body        TEXT,   -- 純文字內容（已去 HTML）
            links       TEXT    -- JSON 陣列，信中的連結
        );
        CREATE INDEX IF NOT EXISTS idx_mails_at ON mails(received_at DESC);
        "#,
    )?;
    Ok(())
}

/// v2：角色 + 邀請碼。讓家人各自建立帳號，稽核分得出是誰做的。
fn migrate_v2(conn: &Connection) -> Result<()> {
    // 既有帳號（第一個註冊的）預設為 admin
    let has_role: bool = conn
        .prepare("SELECT 1 FROM pragma_table_info('users') WHERE name = 'role'")?
        .exists([])?;
    if !has_role {
        conn.execute_batch(
            "ALTER TABLE users ADD COLUMN role TEXT NOT NULL DEFAULT 'admin';",
        )?;
    }

    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS invites (
            token      TEXT PRIMARY KEY,
            role       TEXT NOT NULL DEFAULT 'member',
            note       TEXT,
            created_by TEXT,
            created_at INTEGER NOT NULL,
            expires_at INTEGER NOT NULL,
            used_at    INTEGER,
            used_by    TEXT
        );
        "#,
    )?;
    Ok(())
}

fn migrate_v1(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS users (
            id           TEXT PRIMARY KEY,
            username     TEXT NOT NULL UNIQUE,
            display_name TEXT NOT NULL,
            created_at   INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS credentials (
            id           TEXT PRIMARY KEY,   -- credential id (base64url)
            user_id      TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            passkey      TEXT NOT NULL,      -- Passkey 的 JSON 序列化
            nickname     TEXT,
            created_at   INTEGER NOT NULL,
            last_used_at INTEGER
        );

        -- 白名單。expires_at 存絕對時間戳，不是相對 TTL，
        -- 這樣重開機後剩餘時間才是對的（nft 的 timeout 做不到這點）。
        CREATE TABLE IF NOT EXISTS allowlist (
            ip         TEXT PRIMARY KEY,
            label      TEXT,
            added_by   TEXT,
            added_at   INTEGER NOT NULL,
            expires_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS audit (
            id        INTEGER PRIMARY KEY AUTOINCREMENT,
            at        INTEGER NOT NULL,
            actor     TEXT,
            action    TEXT NOT NULL,
            detail    TEXT,
            client_ip TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_audit_at ON audit(at DESC);

        -- 首次註冊用的一次性碼。沒有它，面板一上線任何人都能註冊第一把 passkey。
        CREATE TABLE IF NOT EXISTS bootstrap (
            token      TEXT PRIMARY KEY,
            created_at INTEGER NOT NULL,
            used_at    INTEGER
        );
        "#,
    )?;
    Ok(())
}

/// 測試用：一顆已完成遷移的記憶體資料庫。放在這裡而不是各模組各寫一份，
/// 是為了讓「測試用的 schema」跟正式的遷移路徑保證同源。
#[cfg(test)]
pub fn test_db() -> Db {
    let conn = Connection::open_in_memory().unwrap();
    conn.pragma_update(None, "foreign_keys", "ON").unwrap();
    migrate(&conn).unwrap();
    Arc::new(Mutex::new(conn))
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ── 使用者與憑證 ──────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct User {
    pub id: String,
    pub username: String,
    pub display_name: String,
    /// "admin" 可管理邀請碼並移除任何人的白名單；"member" 只能管自己加的
    pub role: String,
    /// 面板一律以 email 稱呼使用者。v6 之前註冊的帳號為 None，
    /// 首次登入時會被要求補填。
    pub email: Option<String>,
}

impl User {
    pub fn is_admin(&self) -> bool {
        self.role == "admin"
    }

    /// 畫面上顯示用。舊帳號還沒補 email 之前退回 username。
    pub fn label(&self) -> &str {
        self.email.as_deref().unwrap_or(&self.username)
    }
}

pub fn user_count(db: &Db) -> Result<i64> {
    let conn = db.lock().unwrap();
    Ok(conn.query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))?)
}

const USER_COLS: &str = "id, username, display_name, role, email";

fn row_to_user(r: &rusqlite::Row) -> rusqlite::Result<User> {
    Ok(User {
        id: r.get(0)?,
        username: r.get(1)?,
        display_name: r.get(2)?,
        role: r.get(3)?,
        email: r.get(4)?,
    })
}

pub fn find_user(db: &Db, username: &str) -> Result<Option<User>> {
    let conn = db.lock().unwrap();
    Ok(conn
        .query_row(
            &format!("SELECT {USER_COLS} FROM users WHERE username = ?1"),
            params![username],
            row_to_user,
        )
        .optional()?)
}

/// 登入以 email 為入口。位址一律正規化後比對。
pub fn find_user_by_email(db: &Db, email: &str) -> Result<Option<User>> {
    let conn = db.lock().unwrap();
    Ok(conn
        .query_row(
            &format!("SELECT {USER_COLS} FROM users WHERE email = ?1"),
            params![norm(email)],
            row_to_user,
        )
        .optional()?)
}

pub fn get_user(db: &Db, id: &str) -> Result<Option<User>> {
    let conn = db.lock().unwrap();
    Ok(conn
        .query_row(
            &format!("SELECT {USER_COLS} FROM users WHERE id = ?1"),
            params![id],
            row_to_user,
        )
        .optional()?)
}

pub fn list_users(db: &Db) -> Result<Vec<User>> {
    let conn = db.lock().unwrap();
    let mut stmt = conn.prepare(&format!(
        "SELECT {USER_COLS} FROM users ORDER BY role, coalesce(email, username)"
    ))?;
    let rows = stmt.query_map([], row_to_user)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn set_user_email(db: &Db, id: &str, email: &str) -> Result<()> {
    let conn = db.lock().unwrap();
    conn.execute(
        "UPDATE users SET email = ?2 WHERE id = ?1",
        params![id, norm(email)],
    )?;
    Ok(())
}

/// 角色升降。呼叫端負責擋掉「拿掉最後一個 admin」——
/// 那是授權規則，不是資料完整性約束。
pub fn set_user_role(db: &Db, id: &str, role: &str) -> Result<usize> {
    let conn = db.lock().unwrap();
    Ok(conn.execute("UPDATE users SET role = ?2 WHERE id = ?1", params![id, role])?)
}

/// 移除一個帳號時一併帶走的東西。
#[derive(Debug, PartialEq, Eq)]
pub struct Removed {
    /// 他新增的白名單條目。呼叫端必須接著 `nft::sync`。
    pub entries: usize,
    /// 轉發到他信箱的登記。
    pub recipients: usize,
}

/// 移除一個帳號，連同他的存取能力。
///
/// 會被帶走的東西：
///   - `credentials`、`user_platforms`、`push_subscriptions` —— 外鍵 ON DELETE CASCADE
///   - **他新增的白名單條目** —— 手動刪。移除一個人卻留著他授權的網路，
///     等於沒有移除；那些網路上的裝置照樣能用。呼叫端必須接著 nft::sync。
///   - **轉發到他信箱的登記** —— 同一個道理，而且更嚴重：白名單有 TTL
///     會自己過期，轉發不會。留著等於那個人**永遠繼續收到驗證碼**，
///     而面板上完全看不出來他已經被移除了。
///   - 他那筆 `invited_emails` —— 位址回到乾淨狀態，之後可以重新登記。
///     不刪的話它會永遠卡在「已使用」，那個位址再也註冊不了。
///
/// 全部在同一個交易裡：刪一半的帳號比沒刪更糟。
///
/// ⚠️ Cloudflare 那邊的目的地位址**刻意不動**。它是帳戶層級的共用資源，
///    可能還有別的路由規則在用，而且刪掉已驗證的位址是不可逆的。
///    面板這邊不轉了就夠了。
///
/// 稽核紀錄刻意不動：那是歷史，人走了不代表做過的事沒發生過。
///
/// ⚠️ 「不能刪自己」與「不能刪掉最後一個 admin」是**授權規則**，
/// 由呼叫端負責 —— 這裡只做事，不判斷該不該做。
pub fn delete_user(db: &Db, user_id: &str, label: &str) -> Result<Removed> {
    let mut conn = db.lock().unwrap();
    let tx = conn.transaction()?;

    // 先把信箱撈出來 —— 帳號一刪就查不到了，而轉發是以位址登記的
    let email: Option<String> = tx
        .query_row("SELECT email FROM users WHERE id = ?1", params![user_id], |r| r.get(0))
        .optional()?
        .flatten();

    let entries = tx.execute("DELETE FROM allowlist WHERE added_by = ?1", params![label])?;
    let recipients = match &email {
        Some(e) => tx.execute("DELETE FROM mail_recipients WHERE address = ?1", params![norm(e)])?,
        None => 0,
    };
    tx.execute("DELETE FROM invited_emails WHERE used_by = ?1", params![user_id])?;
    tx.execute("DELETE FROM users WHERE id = ?1", params![user_id])?;

    tx.commit()?;
    Ok(Removed { entries, recipients })
}

pub fn admin_count(db: &Db) -> Result<i64> {
    let conn = db.lock().unwrap();
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM users WHERE role = 'admin'",
        [],
        |r| r.get(0),
    )?)
}

/// 建立帳號並授予平台，**順序封在這裡**。
///
/// ⚠️ `user_platforms.user_id` 有外鍵指向 `users`，授權一定要排在建立帳號
/// 之後。這個順序曾經在 `register_finish` 裡寫反、而且用 `let _ =` 吞掉錯誤，
/// 結果是登記時選好的平台**靜默地一個都沒授權**，家人註冊完看到的是空的
/// 驗證碼分頁。把兩件事綁在一支函式裡，順序就不可能再寫反。
///
/// 兩步在同一個交易裡：授權失敗時不留下一個沒有平台的帳號。
pub fn create_user_with_platforms(
    db: &Db,
    id: &str,
    username: &str,
    display_name: &str,
    role: &str,
    email: Option<&str>,
    platforms: &[String],
) -> Result<()> {
    let mut conn = db.lock().unwrap();
    let tx = conn.transaction()?;
    let t = now();

    tx.execute(
        "INSERT INTO users (id, username, display_name, role, email, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![id, username, display_name, role, email.map(norm), t],
    )?;
    for code in platforms {
        tx.execute(
            "INSERT OR IGNORE INTO user_platforms (user_id, platform, granted_by, granted_at)
             VALUES (?1,?2,?3,?4)",
            params![id, code, "邀請時指定", t],
        )?;
    }
    tx.commit()?;
    Ok(())
}

// ── 邀請碼（已淘汰）──────────────────────────────────
//
// v6 起改用 `invited_emails`：憑據綁信箱而不是一段可轉傳的連結。
// `invites` 表刻意留著不刪 —— 移除它要一次破壞性遷移，而它只是躺著佔幾 KB。

/// 一把 passkey 的中繼資料。**不含 `passkey` 欄位本身** ——
/// 那是憑證材料，前端不需要也不該拿到。
#[derive(Debug, Serialize)]
pub struct Credential {
    pub id: String,
    /// 使用者取的名字，例如「iPhone 15」。v6 之前註冊的都是 None。
    pub nickname: Option<String>,
    pub created_at: i64,
    /// 最後一次用它登入的時間。None = 註冊後從未用過 ——
    /// 那通常代表這把是備援，或那台裝置已經不在了。
    pub last_used_at: Option<i64>,
}

pub fn add_credential(
    db: &Db,
    cred_id: &str,
    user_id: &str,
    passkey_json: &str,
    nickname: Option<&str>,
) -> Result<()> {
    let conn = db.lock().unwrap();
    conn.execute(
        "INSERT INTO credentials (id, user_id, passkey, nickname, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![cred_id, user_id, passkey_json, nickname, now()],
    )?;
    Ok(())
}

pub fn list_credentials(db: &Db, user_id: &str) -> Result<Vec<Credential>> {
    let conn = db.lock().unwrap();
    let mut stmt = conn.prepare(
        "SELECT id, nickname, created_at, last_used_at FROM credentials
         WHERE user_id = ?1 ORDER BY created_at",
    )?;
    let rows = stmt.query_map(params![user_id], |r| {
        Ok(Credential {
            id: r.get(0)?,
            nickname: r.get(1)?,
            created_at: r.get(2)?,
            last_used_at: r.get(3)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// 移除一把 passkey。
///
/// WHERE 帶上 `user_id` 而不只是 `id`：credential id 是 base64url，
/// 猜不出來但也不是機密（它會出現在登入回應裡）。綁上擁有者之後，
/// 就算有人拿到別人的 id 也刪不掉。
///
/// ⚠️ 「不能刪掉最後一把」是**授權規則**，由呼叫端在同一個檢查裡負責 ——
/// 沒有密碼可以救，刪光了就永遠登不進來。
pub fn delete_credential(db: &Db, user_id: &str, cred_id: &str) -> Result<usize> {
    let conn = db.lock().unwrap();
    Ok(conn.execute(
        "DELETE FROM credentials WHERE user_id = ?1 AND id = ?2",
        params![user_id, cred_id],
    )?)
}

pub fn rename_credential(
    db: &Db,
    user_id: &str,
    cred_id: &str,
    nickname: Option<&str>,
) -> Result<usize> {
    let conn = db.lock().unwrap();
    Ok(conn.execute(
        "UPDATE credentials SET nickname = ?3 WHERE user_id = ?1 AND id = ?2",
        params![user_id, cred_id, nickname],
    )?)
}

/// 取回某使用者的所有 passkey（JSON 字串，由呼叫端反序列化）
pub fn credentials_for(db: &Db, user_id: &str) -> Result<Vec<String>> {
    let conn = db.lock().unwrap();
    let mut stmt = conn.prepare("SELECT passkey FROM credentials WHERE user_id = ?1")?;
    let rows = stmt.query_map(params![user_id], |r| r.get::<_, String>(0))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn credential_count(db: &Db, user_id: &str) -> Result<i64> {
    let conn = db.lock().unwrap();
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM credentials WHERE user_id = ?1",
        params![user_id],
        |r| r.get(0),
    )?)
}

pub fn touch_credential(db: &Db, cred_id: &str) -> Result<()> {
    let conn = db.lock().unwrap();
    conn.execute(
        "UPDATE credentials SET last_used_at = ?1 WHERE id = ?2",
        params![now(), cred_id],
    )?;
    Ok(())
}

// ── 白名單 ────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AllowEntry {
    pub ip: String,
    pub label: Option<String>,
    pub added_by: Option<String>,
    pub added_at: i64,
    pub expires_at: i64,
    /// 自動續期時要延多久。存在條目上而不是全域設定，
    /// 因為授權當下選的天數是使用者對「這個網路」的判斷。
    pub ttl_days: i64,
    pub renewed_at: Option<i64>,
}

const ALLOW_COLS: &str = "ip, label, added_by, added_at, expires_at, ttl_days, renewed_at";

fn row_to_allow(r: &rusqlite::Row) -> rusqlite::Result<AllowEntry> {
    Ok(AllowEntry {
        ip: r.get(0)?,
        label: r.get(1)?,
        added_by: r.get(2)?,
        added_at: r.get(3)?,
        expires_at: r.get(4)?,
        ttl_days: r.get(5)?,
        renewed_at: r.get(6)?,
    })
}

pub fn list_allow(db: &Db) -> Result<Vec<AllowEntry>> {
    let conn = db.lock().unwrap();
    let mut stmt =
        conn.prepare(&format!("SELECT {ALLOW_COLS} FROM allowlist ORDER BY added_at DESC"))?;
    let rows = stmt.query_map([], row_to_allow)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// 某人目前佔用的條目數。額度是 per-user 的 —— 全域上限已在 v6 拿掉，
/// 濫用防護改由「4 × 成員數」與 admin 的 Email 登記共同構成。
pub fn allow_count_by(db: &Db, added_by: &str) -> Result<i64> {
    let conn = db.lock().unwrap();
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM allowlist WHERE added_by = ?1",
        params![added_by],
        |r| r.get(0),
    )?)
}

/// 把白名單的擁有者標記從舊的稱呼改成新的。
///
/// `added_by` 存的是**顯示名稱**而不是 user_id —— 這是 v1 就留下的形狀，
/// 而額度與「只能移除自己加的」都靠它比對。舊帳號補填 email 之後稱呼會變，
/// 不一起改的話那個人的條目會突然變成「不是我的」，額度也會歸零。
///
/// 稽核紀錄刻意**不改**：那是歷史，當時的 actor 就是叫那個名字。
pub fn rename_owner(db: &Db, from: &str, to: &str) -> Result<usize> {
    let conn = db.lock().unwrap();
    Ok(conn.execute(
        "UPDATE allowlist SET added_by = ?2 WHERE added_by = ?1",
        params![from, to],
    )?)
}

/// 改名不動到期時間 —— 這是純標記操作，不該偷偷續命。
pub fn rename_allow(db: &Db, ip: &str, label: Option<&str>) -> Result<usize> {
    let conn = db.lock().unwrap();
    Ok(conn.execute(
        "UPDATE allowlist SET label = ?2 WHERE ip = ?1",
        params![ip, label],
    )?)
}

/// 自動續期：把到期時間往後推該條目自己的 ttl_days。
/// 記下 renewed_at 讓稽核看得出這是機器做的、不是人按的。
pub fn renew_allow(db: &Db, ip: &str) -> Result<Option<i64>> {
    let conn = db.lock().unwrap();
    let n = conn.execute(
        // 一併把到期提醒的標記清掉：續期後它不再快到期了，
        // 下一輪真的接近到期時應該可以再提醒一次
        "UPDATE allowlist SET expires_at = ?2 + ttl_days * 86400, renewed_at = ?2,
                expiry_notified_at = NULL
         WHERE ip = ?1",
        params![ip, now()],
    )?;
    if n != 1 {
        return Ok(None);
    }
    Ok(Some(conn.query_row(
        "SELECT expires_at FROM allowlist WHERE ip = ?1",
        params![ip],
        |r| r.get(0),
    )?))
}

/// 清掉已過期的條目，回傳被清掉的數量。
pub fn purge_expired(db: &Db) -> Result<usize> {
    let conn = db.lock().unwrap();
    Ok(conn.execute("DELETE FROM allowlist WHERE expires_at <= ?1", params![now()])?)
}

pub fn upsert_allow(
    db: &Db,
    ip: &str,
    label: Option<&str>,
    added_by: Option<&str>,
    expires_at: i64,
    ttl_days: i64,
) -> Result<()> {
    let conn = db.lock().unwrap();
    conn.execute(
        // 手動重新授權跟自動續期一樣要清掉到期提醒的標記（同 renew_allow）——
        // 而且這正是最常見的路徑：收到「快到期」通知的人就是來按這顆的。
        // 不清的話那條白名單這輩子只會被提醒一次，之後每次到期都是靜悄悄的。
        "INSERT INTO allowlist (ip, label, added_by, added_at, expires_at, ttl_days)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(ip) DO UPDATE SET
             label       = coalesce(excluded.label, allowlist.label),
             expires_at  = excluded.expires_at,
             ttl_days    = excluded.ttl_days,
             expiry_notified_at = NULL",
        params![ip, label, added_by, now(), expires_at, ttl_days],
    )?;
    Ok(())
}

pub fn remove_allow(db: &Db, ip: &str) -> Result<usize> {
    let conn = db.lock().unwrap();
    Ok(conn.execute("DELETE FROM allowlist WHERE ip = ?1", params![ip])?)
}

// ── 稽核 ──────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct AuditRow {
    /// 這一列的身分。同一秒、同一個動作、同一段說明的兩列是真的會出現的
    /// （兩個人同時做同一件事），所以前端不能拿欄位拼 key，只能用它。
    pub id: i64,
    pub at: i64,
    pub actor: Option<String>,
    pub action: String,
    pub detail: Option<String>,
    pub client_ip: Option<String>,
}

pub fn audit(db: &Db, actor: Option<&str>, action: &str, detail: Option<&str>, ip: Option<&str>) {
    if let Ok(conn) = db.lock() {
        let _ = conn.execute(
            "INSERT INTO audit (at, actor, action, detail, client_ip) VALUES (?1,?2,?3,?4,?5)",
            params![now(), actor, action, detail, ip],
        );
    }
}

pub fn recent_audit(db: &Db, limit: i64) -> Result<Vec<AuditRow>> {
    let conn = db.lock().unwrap();
    let mut stmt = conn.prepare(
        // 同秒的兩列靠 id 決定先後，否則順序由 SQLite 自由發揮
        "SELECT id, at, actor, action, detail, client_ip FROM audit
         ORDER BY at DESC, id DESC LIMIT ?1",
    )?;
    let rows = stmt.query_map(params![limit], |r| {
        Ok(AuditRow {
            id: r.get(0)?,
            at: r.get(1)?,
            actor: r.get(2)?,
            action: r.get(3)?,
            detail: r.get(4)?,
            client_ip: r.get(5)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

// ── 驗證碼信件 ────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct Mail {
    pub id: i64,
    pub received_at: i64,
    pub sender: Option<String>,
    /// 收到這封信的信箱。多平台共用面板時，這是唯一能分辨
    /// 「這封是寄到哪個信箱」的線索（設計 1b 顯示的就是它）。
    pub recipient: Option<String>,
    pub subject: Option<String>,
    pub code: Option<String>,
    pub body: Option<String>,
    pub html: Option<String>,
    pub links: Vec<String>,
    /// 寄件者是否通過 DKIM/DMARC 驗證。None = 舊資料，當時還沒有這個欄位。
    pub verified: Option<bool>,
    /// 平台代號（對應 domain-set 檔名）。決定誰看得到這封信的驗證碼。
    /// None = 認不出是哪個平台，只會出現在管理收件匣。
    pub platform: Option<String>,
    /// 沒抽到驗證碼的原因。有值代表這封信不進驗證碼分頁。
    pub skip_reason: Option<String>,
    /// 信裡「要按的那個連結」，頁尾樣板已排除。
    /// 抽不到碼的信靠它把使用者送到平台的取碼頁。
    pub primary_link: Option<String>,
}

/// 回傳 true 表示是新信件；false 表示 Message-ID 已存在（Worker 重送）。
#[allow(clippy::too_many_arguments)]
pub fn insert_mail(
    db: &Db,
    message_id: Option<&str>,
    received_at: i64,
    sender: Option<&str>,
    recipient: Option<&str>,
    subject: Option<&str>,
    code: Option<&str>,
    body: Option<&str>,
    html: Option<&str>,
    links: &[String],
    verified: bool,
    platform: Option<&str>,
    skip_reason: Option<&str>,
) -> Result<bool> {
    let conn = db.lock().unwrap();
    let n = conn.execute(
        "INSERT OR IGNORE INTO mails
         (message_id, received_at, sender, recipient, subject, code, body, html, links,
          verified, platform, skip_reason)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
        params![
            message_id,
            received_at,
            sender,
            recipient,
            subject,
            code,
            body,
            html,
            serde_json::to_string(links).unwrap_or_else(|_| "[]".into()),
            verified as i64,
            platform,
            skip_reason
        ],
    )?;
    Ok(n == 1)
}

pub fn recent_mails(db: &Db, limit: i64) -> Result<Vec<Mail>> {
    let conn = db.lock().unwrap();
    let mut stmt = conn.prepare(
        "SELECT id, received_at, sender, subject, code, body, links, html, verified,
                platform, skip_reason, recipient
         FROM mails ORDER BY received_at DESC LIMIT ?1",
    )?;
    let rows = stmt.query_map(params![limit], |r| {
        let links_json: String = r.get(6).unwrap_or_else(|_| "[]".into());
        let links: Vec<String> = serde_json::from_str(&links_json).unwrap_or_default();
        Ok(Mail {
            id: r.get(0)?,
            received_at: r.get(1)?,
            sender: r.get(2)?,
            subject: r.get(3)?,
            code: r.get(4)?,
            body: r.get(5)?,
            html: r.get(7).unwrap_or(None),
            primary_link: crate::mail::primary_link(&links),
            links,
            verified: r.get::<_, Option<i64>>(8).unwrap_or(None).map(|v| v != 0),
            platform: r.get(9).unwrap_or(None),
            skip_reason: r.get(10).unwrap_or(None),
            recipient: r.get(11).unwrap_or(None),
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// 「全部刪除」。回傳刪掉的筆數。
pub fn delete_all_mails(db: &Db) -> Result<usize> {
    let conn = db.lock().unwrap();
    Ok(conn.execute("DELETE FROM mails", [])?)
}

/// 還沒歸屬平台的信件，連同它們的收件信箱。
///
/// v6 之前收到的信 `platform` 都是 NULL，而驗證碼分頁是靠平台過濾的 ——
/// 不回填的話那些信對**所有人**都會消失。啟動時跑一次補上。
pub fn mails_missing_platform(db: &Db) -> Result<Vec<(i64, String)>> {
    let conn = db.lock().unwrap();
    let mut stmt = conn.prepare(
        "SELECT id, recipient FROM mails WHERE platform IS NULL AND recipient IS NOT NULL",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn update_mail_platform(db: &Db, id: i64, platform: Option<&str>) -> Result<()> {
    let conn = db.lock().unwrap();
    conn.execute(
        "UPDATE mails SET platform = ?2 WHERE id = ?1",
        params![id, platform],
    )?;
    Ok(())
}

// ── 轉發收件人 ────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub struct Recipient {
    pub id: i64,
    pub mailbox: String,
    pub address: String,
    pub label: Option<String>,
    pub enabled: bool,
    pub added_by: Option<String>,
    pub added_at: i64,
    /// Cloudflare Email Routing 回報的驗證時間。None = 尚未驗證，
    /// 但這個結論只有在 cf_checked_at 有值時才成立 —— 否則只是還沒查過。
    pub cf_verified_at: Option<i64>,
    pub cf_checked_at: Option<i64>,
    /// Cloudflare 有沒有這個位址。None = 還沒查過。
    /// `Some(false)` 是**最需要處理**的狀態：轉發一定失敗。
    pub cf_present: Option<bool>,
}

/// 位址與信箱一律轉小寫存放，路由查詢靠字串比對。
fn norm(s: &str) -> String {
    s.trim().to_lowercase()
}

pub fn list_recipients(db: &Db) -> Result<Vec<Recipient>> {
    let conn = db.lock().unwrap();
    let mut stmt = conn.prepare(
        "SELECT id, mailbox, address, label, enabled, added_by, added_at,
                cf_verified_at, cf_checked_at, cf_present
         FROM mail_recipients ORDER BY mailbox, address",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(Recipient {
            id: r.get(0)?,
            mailbox: r.get(1)?,
            address: r.get(2)?,
            label: r.get(3)?,
            enabled: r.get::<_, i64>(4)? != 0,
            added_by: r.get(5)?,
            added_at: r.get(6)?,
            cf_verified_at: r.get(7).unwrap_or(None),
            cf_checked_at: r.get(8).unwrap_or(None),
            cf_present: r.get::<_, Option<i64>>(9).unwrap_or(None).map(|v| v != 0),
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// 某個位址名下的所有轉發登記（跨 mailbox）。
///
/// 用位址比對而不是加 `user_id` 外鍵 —— 這張表刻意不跟 `users` 綁定
/// （見 v5 的註解：多數家人還沒註冊 passkey）。
pub fn recipients_for_address(db: &Db, address: &str) -> Result<Vec<Recipient>> {
    Ok(list_recipients(db)?
        .into_iter()
        .filter(|r| r.address == norm(address))
        .collect())
}

/// 一次切換某個位址名下所有登記的啟用狀態。回異動的筆數。
/// 永久刪掉某個信箱底下的所有轉發登記。給「信箱一開始就設錯」用 ——
/// 設錯的信箱不該只是停用，它會繼續看起來像個有效的轉發目標。
pub fn delete_recipients_for_mailbox(db: &Db, mailbox: &str) -> Result<usize> {
    let conn = db.lock().unwrap();
    Ok(conn.execute(
        "DELETE FROM mail_recipients WHERE mailbox = ?1",
        params![norm(mailbox)],
    )?)
}

pub fn set_recipients_enabled_for_address(db: &Db, address: &str, enabled: bool) -> Result<usize> {
    let conn = db.lock().unwrap();
    Ok(conn.execute(
        "UPDATE mail_recipients SET enabled = ?2 WHERE address = ?1",
        params![norm(address), enabled as i64],
    )?)
}

/// 用 Cloudflare 回來的**完整**清單覆寫所有人的狀態。
///
/// ⚠️ 一定要整份覆寫。逐筆 `UPDATE ... WHERE address = ?` 會讓 Cloudflare
/// 沒回傳的位址永遠停在「未查詢」—— 而那種位址的轉發一定會退信。
///
/// 先全部標成「不在 Cloudflare」再補回查到的，兩步同一個交易。
pub fn sync_cf_status(db: &Db, found: &[(String, Option<i64>)]) -> Result<()> {
    let mut conn = db.lock().unwrap();
    let tx = conn.transaction()?;
    let t = now();

    tx.execute(
        "UPDATE mail_recipients SET cf_checked_at = ?1, cf_present = 0, cf_verified_at = NULL",
        params![t],
    )?;
    for (address, verified_at) in found {
        // 同一個位址可能登記在多個 mailbox 底下，狀態是共用的 —— 比對位址而非 id
        tx.execute(
            "UPDATE mail_recipients SET cf_present = 1, cf_verified_at = ?2 WHERE address = ?1",
            params![norm(address), verified_at],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// 某信箱目前啟用中的轉發位址。這是 Worker 路由決策的唯一依據。
pub fn enabled_recipients_for(db: &Db, mailbox: &str) -> Result<Vec<String>> {
    let conn = db.lock().unwrap();
    let mut stmt = conn.prepare(
        "SELECT address FROM mail_recipients
         WHERE mailbox = ?1 AND enabled = 1 ORDER BY address",
    )?;
    let rows = stmt.query_map(params![norm(mailbox)], |r| r.get::<_, String>(0))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn add_recipient(
    db: &Db,
    mailbox: &str,
    address: &str,
    label: Option<&str>,
    added_by: &str,
) -> Result<()> {
    let conn = db.lock().unwrap();
    conn.execute(
        "INSERT INTO mail_recipients (mailbox, address, label, added_by, added_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(mailbox, address) DO UPDATE SET enabled = 1, label = excluded.label",
        params![norm(mailbox), norm(address), label, added_by, now()],
    )?;
    Ok(())
}

pub fn set_recipient_enabled(db: &Db, id: i64, enabled: bool) -> Result<usize> {
    let conn = db.lock().unwrap();
    Ok(conn.execute(
        "UPDATE mail_recipients SET enabled = ?2 WHERE id = ?1",
        params![id, enabled as i64],
    )?)
}

pub fn delete_recipient(db: &Db, id: i64) -> Result<usize> {
    let conn = db.lock().unwrap();
    Ok(conn.execute("DELETE FROM mail_recipients WHERE id = ?1", params![id])?)
}

/// 清除逾期信件，保留天數由呼叫端決定。
pub fn purge_old_mails(db: &Db, keep_days: i64) -> Result<usize> {
    let conn = db.lock().unwrap();
    Ok(conn.execute(
        "DELETE FROM mails WHERE received_at < ?1",
        params![now() - keep_days * 86400],
    )?)
}

/// 取出所有信件的主旨與內文供重新抽取驗證碼。
/// 每次啟動重跑一遍，讓規則改動自動套用到既有信件。
pub fn mails_for_reextract(db: &Db) -> Result<Vec<(i64, String, Option<String>)>> {
    let conn = db.lock().unwrap();
    let mut stmt = conn.prepare("SELECT id, coalesce(subject,'')||char(10)||coalesce(body,''), code FROM mails")?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn update_mail_code(db: &Db, id: i64, code: Option<&str>) -> Result<()> {
    let conn = db.lock().unwrap();
    conn.execute("UPDATE mails SET code = ?1 WHERE id = ?2", params![code, id])?;
    Ok(())
}

pub fn delete_mail(db: &Db, id: i64) -> Result<usize> {
    let conn = db.lock().unwrap();
    Ok(conn.execute("DELETE FROM mails WHERE id = ?1", params![id])?)
}

// ── 面板設定 ──────────────────────────────────────────

/// 設定的鍵。集中成常數，避免字串在 handler 裡各寫各的。
pub mod keys {
    /// "off" | "observe" | "enforce" —— 只管**顯示**，不管轉發。
    /// 轉發由 [`FORWARD_ENFORCE`] 決定，兩者刻意分開：面板上想看到可疑的信
    /// （才查得出問題），不代表要把它轉給家人。
    pub const SENDER_MODE: &str = "sender_verify_mode";
    /// JSON 陣列：可信的寄件品牌網域（比對 DKIM header.d）
    pub const SENDER_DOMAINS: &str = "sender_domains";
    /// JSON 陣列：命中任一才算驗證碼信
    pub const CODE_KEYWORDS: &str = "code_keywords";
    /// JSON 陣列：命中任一一律排除
    pub const CODE_EXCLUDES: &str = "code_excludes";
    /// `"1"` / `"0"`：未通過寄件者驗證的信要不要**轉發**。
    ///
    /// 跟 [`SENDER_MODE`] 是兩件事：那個管面板顯示，這個決定
    /// `/api/mail/ingest` 回給 Worker 的 `forward_to` 名單。
    pub const FORWARD_ENFORCE: &str = "forward_enforce_sender";
    /// JSON 物件 `{ 平台代號: [寄件者位址或網域, …] }`。
    ///
    /// 收件信箱的 local part 本來就能推出平台，但那要求每個平台各有一個
    /// 信箱。用同一個 catch-all 收全部時那個線索就沒了 —— 這份對應是補那個洞。
    pub const PLATFORM_SENDERS: &str = "platform_senders";
    /// 轉發信箱的網域，如 `share.example.com`。
    ///
    /// 面板要自己組出 `netflix@share.example.com` 這種 mailbox（登記邀請時
    /// 順帶新增轉發），所以它不能只活在前端。環境變數當種子值，
    /// 之後以 DB 為準 —— 跟其他面板可改設定一致。
    pub const MAIL_DOMAIN: &str = "mail_domain";
    /// JSON 物件 `{ 平台代號: 收件信箱 }`，例如
    /// `{"disneyplus": "disney@share.example.com"}`。
    ///
    /// ⚠️ **不能用 `代號@網域` 推**。那個約定對 Netflix 剛好成立
    /// （`netflix.list` → `netflix@`），對 Disney+ 就是錯的
    /// （`disneyplus.list` 但信箱是 `disney@`）。推錯的後果是新增到一個
    /// 根本收不到信的信箱，而且完全沒有徵兆。對應必須是 admin 明說的。
    pub const PLATFORM_MAILBOXES: &str = "platform_mailboxes";
    /// VAPID 金鑰對，首次推送時產生。私鑰用來簽 JWT，公鑰要給前端訂閱時用。
    ///
    /// 跟 HMAC 那幾把一樣存在 `settings`：純內部密鑰，沒有理由要求部署的人
    /// 自己產生保管。**換掉等於所有既有訂閱作廢**，所以只在缺的時候產生。
    pub const VAPID_PRIVATE: &str = "vapid_private_key";
    pub const VAPID_PUBLIC: &str = "vapid_public_key";
}

pub fn get_setting(db: &Db, key: &str) -> Result<Option<String>> {
    let conn = db.lock().unwrap();
    Ok(conn
        .query_row(
            "SELECT value FROM settings WHERE key = ?1",
            params![key],
            |r| r.get(0),
        )
        .optional()?)
}

pub fn set_setting(db: &Db, key: &str, value: &str, by: Option<&str>) -> Result<()> {
    let conn = db.lock().unwrap();
    conn.execute(
        "INSERT INTO settings (key, value, updated_by, updated_at) VALUES (?1,?2,?3,?4)
         ON CONFLICT(key) DO UPDATE SET
             value=excluded.value, updated_by=excluded.updated_by, updated_at=excluded.updated_at",
        params![key, value, by, now()],
    )?;
    Ok(())
}

/// 只在鍵尚不存在時寫入。啟動時用環境變數種入預設值，
/// 之後 UI 改過的值就不會被下一次重啟蓋掉。
pub fn seed_setting(db: &Db, key: &str, value: &str) -> Result<()> {
    let conn = db.lock().unwrap();
    conn.execute(
        "INSERT OR IGNORE INTO settings (key, value, updated_at) VALUES (?1,?2,?3)",
        params![key, value, now()],
    )?;
    Ok(())
}

/// 取出一把存在 `settings` 裡的 HMAC 金鑰，第一次呼叫時產生。
///
/// 用 `settings` 而不是環境變數：這些是純內部的密鑰，沒有任何理由要求
/// 部署的人去產生並保管。存在 DB 裡也代表它跟資料庫同壽 —— 重建資料庫時
/// 在途的驗證碼與邀請連結一起失效，這正是我們要的。
///
/// 每種用途一把（`key` 不同），互相不能拿來偽造對方的憑據。
pub fn hmac_key(db: &Db, key: &str) -> Result<String> {
    if let Some(k) = get_setting(db, key)? {
        return Ok(k);
    }
    // 兩個 v4 UUID = 256 bit 的 CSPRNG 輸出
    let fresh = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    seed_setting(db, key, &fresh)?;
    // 重讀而不是直接回傳：併發時 seed 可能沒寫進去，要拿實際生效的那把
    get_setting(db, key)?.with_context(|| format!("無法建立金鑰 {key}"))
}

/// 讀 JSON 陣列型的設定。解析失敗當成空陣列 ——
/// 設定壞掉不該讓收信整條掛掉。
pub fn get_setting_list(db: &Db, key: &str) -> Vec<String> {
    get_setting(db, key)
        .ok()
        .flatten()
        .and_then(|v| serde_json::from_str::<Vec<String>>(&v).ok())
        .unwrap_or_default()
}

pub fn set_setting_list(db: &Db, key: &str, values: &[String], by: Option<&str>) -> Result<()> {
    let json = serde_json::to_string(values)?;
    set_setting(db, key, &json, by)
}

/// 以環境變數種入清單型設定的初值。同 [`seed_setting`]，只在鍵不存在時寫入。
pub fn set_setting_list_if_absent(db: &Db, key: &str, values: &[String]) -> Result<()> {
    seed_setting(db, key, &serde_json::to_string(values)?)
}

/// 讀「鍵 → 清單」型的設定。解析失敗當成空的 —— 同 [`get_setting_list`]，
/// 設定壞掉不該讓收信整條掛掉。
/// 讀「鍵 → 單一字串」型的設定。解析失敗當成空的。
pub fn get_setting_str_map(db: &Db, key: &str) -> BTreeMap<String, String> {
    get_setting(db, key)
        .ok()
        .flatten()
        .and_then(|v| serde_json::from_str(&v).ok())
        .unwrap_or_default()
}

pub fn set_setting_str_map(
    db: &Db,
    key: &str,
    map: &BTreeMap<String, String>,
    by: Option<&str>,
) -> Result<()> {
    set_setting(db, key, &serde_json::to_string(map)?, by)
}

pub fn get_setting_map(db: &Db, key: &str) -> BTreeMap<String, Vec<String>> {
    get_setting(db, key)
        .ok()
        .flatten()
        .and_then(|v| serde_json::from_str(&v).ok())
        .unwrap_or_default()
}

pub fn set_setting_map(
    db: &Db,
    key: &str,
    map: &BTreeMap<String, Vec<String>>,
    by: Option<&str>,
) -> Result<()> {
    set_setting(db, key, &serde_json::to_string(map)?, by)
}

/// 已收到、但認不出平台的寄件位址，附出現次數。
///
/// 設定頁用它把「請憑記憶輸入位址」變成「從你實際收過的挑」。
/// 沒有這個，管理員得先去收件匣一封封看才知道要填什麼。
pub fn unmatched_senders(db: &Db) -> Result<Vec<(String, i64)>> {
    let conn = db.lock().unwrap();
    let mut stmt = conn.prepare(
        "SELECT sender, COUNT(*) FROM mails
         WHERE platform IS NULL AND sender IS NOT NULL AND sender <> ''
         GROUP BY sender ORDER BY COUNT(*) DESC, sender LIMIT 20",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// 沒有平台歸屬的信件，連同寄件者與收件信箱 —— 設定改動後重新判定用。
pub fn mails_without_platform(db: &Db) -> Result<Vec<(i64, Option<String>, Option<String>)>> {
    let conn = db.lock().unwrap();
    let mut stmt =
        conn.prepare("SELECT id, sender, recipient FROM mails WHERE platform IS NULL")?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

// ── 邀請 Email ────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct InvitedEmail {
    pub email: String,
    /// 註冊完成時要自動授予的平台。空的 = 註冊後還是什麼都看不到，
    /// admin 得再去成員管理開。
    pub platforms: Vec<String>,
    pub invited_by: Option<String>,
    pub invited_at: i64,
    pub revoked_at: Option<i64>,
    pub used_at: Option<i64>,
    pub used_by: Option<String>,
}

/// 重新登記一個曾被撤銷的位址 = 解除撤銷，而不是報「已存在」。
/// 重新登記也會更新平台授權 —— 那是「改這筆登記」最自然的操作方式。
///
/// 連同舊的邀請連結一起作廢。要發新的連結請接著呼叫 [`set_invite_token`] ——
/// 舊連結留著等於「改過的登記」還能被舊信裡的按鈕帶回去用。
pub fn invite_email(db: &Db, email: &str, by: &str, platforms: &[String]) -> Result<()> {
    let conn = db.lock().unwrap();
    conn.execute(
        "INSERT INTO invited_emails (email, invited_by, invited_at, platforms)
         VALUES (?1,?2,?3,?4)
         ON CONFLICT(email) DO UPDATE SET
             revoked_at = NULL, invited_by = excluded.invited_by,
             invited_at = excluded.invited_at, platforms = excluded.platforms,
             token_hash = NULL",
        params![norm(email), by, now(), serde_json::to_string(platforms)?],
    )?;
    Ok(())
}

/// **還在等對方註冊**的登記。已撤銷與已註冊的都不列出來。
///
/// 這張清單回答的是「還有誰沒進來」。兩種已結束的狀態都沒有回答它：
///
///   已撤銷 —— 那筆之後沒有任何功能：連結已死、註冊擋著、UI 給不出動作
///   已註冊 —— 那個人就在上面的成員清單裡，連平台授權都顯示得更準確
///
/// 兩者留著只會讓清單無限累積死資料，而且**沒有任何地方清得掉**。
/// 要查歷史看稽核（`invite_email_registered` / `invite_email_revoked`）——
/// 依這個專案自己的原則，**稽核才是歷史，這張表是現況**。
///
/// 保留列而不是硬刪：`used_at` 是「這個位址已經換過帳號」的閘門，刪了
/// 就擋不住第二次；而重新登記走 `ON CONFLICT DO UPDATE SET revoked_at =
/// NULL`，撤銷過的那筆會原地復活並重新出現。移除成員時 `delete_user`
/// 會把那列一併刪掉，位址因此回到可以重新登記的乾淨狀態。
pub fn list_invited_emails(db: &Db) -> Result<Vec<InvitedEmail>> {
    let conn = db.lock().unwrap();
    let mut stmt = conn.prepare(
        "SELECT email, invited_by, invited_at, revoked_at, used_at, used_by, platforms
         FROM invited_emails
         WHERE revoked_at IS NULL AND used_at IS NULL
         ORDER BY invited_at DESC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(InvitedEmail {
            email: r.get(0)?,
            invited_by: r.get(1)?,
            invited_at: r.get(2)?,
            revoked_at: r.get(3)?,
            used_at: r.get(4)?,
            used_by: r.get(5)?,
            platforms: r
                .get::<_, Option<String>>(6)
                .ok()
                .flatten()
                .and_then(|v| serde_json::from_str(&v).ok())
                .unwrap_or_default(),
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// 這個位址現在可以拿去註冊嗎？撤銷過或已用過的都不行。
pub fn is_email_invited(db: &Db, email: &str) -> Result<bool> {
    let conn = db.lock().unwrap();
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM invited_emails
         WHERE email = ?1 AND revoked_at IS NULL AND used_at IS NULL",
        params![norm(email)],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// 標記為已使用，並回傳這筆登記要授予的平台。
///
/// WHERE 帶上 used_at IS NULL，讓檢查與標記是同一個原子操作 ——
/// 否則兩個併發請求可以用同一個登記位址各建一個帳號。
///
/// 回 None = 這個位址不能用（沒登記過、已撤銷、或已經被用掉）。
pub fn consume_invited_email(db: &Db, email: &str, user_id: &str) -> Result<Option<Vec<String>>> {
    let conn = db.lock().unwrap();
    let email = norm(email);
    let n = conn.execute(
        "UPDATE invited_emails SET used_at = ?2, used_by = ?3
         WHERE email = ?1 AND revoked_at IS NULL AND used_at IS NULL",
        params![email, now(), user_id],
    )?;
    if n != 1 {
        return Ok(None);
    }
    let json: Option<String> = conn.query_row(
        "SELECT platforms FROM invited_emails WHERE email = ?1",
        params![email],
        |r| r.get(0),
    )?;
    Ok(Some(
        json.and_then(|v| serde_json::from_str(&v).ok()).unwrap_or_default(),
    ))
}

/// 只撤銷還沒用掉的。已註冊的位址要移除的是帳號，不是這筆登記紀錄。
///
/// 撤銷同時清掉連結權杖。查詢本來就會濾掉已撤銷的，清掉是為了讓
/// 「撤銷之後這張表裡不再有任何能換帳號的東西」這句話字面上成立。
pub fn revoke_invited_email(db: &Db, email: &str) -> Result<usize> {
    let conn = db.lock().unwrap();
    Ok(conn.execute(
        "UPDATE invited_emails SET revoked_at = ?2, token_hash = NULL
         WHERE email = ?1 AND used_at IS NULL",
        params![norm(email), now()],
    )?)
}

/// 掛一把新的邀請連結權杖上去（存雜湊）。
///
/// 只掛在還能用的登記上：已撤銷或已註冊的位址不該冒出一條可用連結。
/// 回 false = 沒有這樣的登記，呼叫端不該把連結寄出去。
pub fn set_invite_token(db: &Db, email: &str, token_hash: &str) -> Result<bool> {
    let conn = db.lock().unwrap();
    let n = conn.execute(
        "UPDATE invited_emails SET token_hash = ?2
         WHERE email = ?1 AND revoked_at IS NULL AND used_at IS NULL",
        params![norm(email), token_hash],
    )?;
    Ok(n == 1)
}

/// 用連結權杖的雜湊找回那筆登記。撤銷過或已用掉的都找不到。
pub fn invited_email_by_token(db: &Db, token_hash: &str) -> Result<Option<InvitedEmail>> {
    let conn = db.lock().unwrap();
    Ok(conn
        .query_row(
            "SELECT email, invited_by, invited_at, revoked_at, used_at, used_by, platforms
             FROM invited_emails
             WHERE token_hash = ?1 AND revoked_at IS NULL AND used_at IS NULL",
            params![token_hash],
            |r| {
                Ok(InvitedEmail {
                    email: r.get(0)?,
                    invited_by: r.get(1)?,
                    invited_at: r.get(2)?,
                    revoked_at: r.get(3)?,
                    used_at: r.get(4)?,
                    used_by: r.get(5)?,
                    platforms: r
                        .get::<_, Option<String>>(6)
                        .ok()
                        .flatten()
                        .and_then(|v| serde_json::from_str(&v).ok())
                        .unwrap_or_default(),
                })
            },
        )
        .optional()?)
}

// ── Email 一次性驗證碼 ────────────────────────────────

/// 同一個信箱同時只會有一組有效的碼 —— 重寄直接覆蓋，
/// 舊碼當場失效，避免「攻擊者觸發重寄、舊碼還能用」的窗口。
pub fn put_otp(db: &Db, email: &str, code_hash: &str, ttl_secs: i64) -> Result<()> {
    let conn = db.lock().unwrap();
    let t = now();
    conn.execute(
        "INSERT INTO email_otp (email, code_hash, expires_at, attempts, sent_at)
         VALUES (?1,?2,?3,0,?4)
         ON CONFLICT(email) DO UPDATE SET
             code_hash=excluded.code_hash, expires_at=excluded.expires_at,
             attempts=0, sent_at=excluded.sent_at, verified_at=NULL",
        params![norm(email), code_hash, t + ttl_secs, t],
    )?;
    Ok(())
}

/// 距離可以重寄還剩幾秒。0 代表現在就能重寄。
pub fn otp_cooldown(db: &Db, email: &str, cooldown_secs: i64) -> Result<i64> {
    let conn = db.lock().unwrap();
    let sent: Option<i64> = conn
        .query_row(
            "SELECT sent_at FROM email_otp WHERE email = ?1",
            params![norm(email)],
            |r| r.get(0),
        )
        .optional()?;
    Ok(sent.map_or(0, |s| (s + cooldown_secs - now()).max(0)))
}

pub enum OtpCheck {
    Ok,
    Wrong,
    Expired,
    TooManyAttempts,
}

/// 驗證並在成功時標記通過。失敗一律累加 attempts ——
/// 六位數只有一百萬種，沒有次數上限就等於沒有保護。
pub fn check_otp(db: &Db, email: &str, code_hash: &str, max_attempts: i64) -> Result<OtpCheck> {
    let conn = db.lock().unwrap();
    let email = norm(email);
    let row: Option<(String, i64, i64)> = conn
        .query_row(
            "SELECT code_hash, expires_at, attempts FROM email_otp WHERE email = ?1",
            params![email],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;

    let Some((stored, expires_at, attempts)) = row else {
        return Ok(OtpCheck::Expired);
    };
    if attempts >= max_attempts {
        return Ok(OtpCheck::TooManyAttempts);
    }
    if expires_at <= now() {
        return Ok(OtpCheck::Expired);
    }
    if stored != code_hash {
        conn.execute(
            "UPDATE email_otp SET attempts = attempts + 1 WHERE email = ?1",
            params![email],
        )?;
        return Ok(OtpCheck::Wrong);
    }
    conn.execute(
        "UPDATE email_otp SET verified_at = ?2 WHERE email = ?1",
        params![email, now()],
    )?;
    Ok(OtpCheck::Ok)
}

/// 這個信箱剛剛通過 OTP 了嗎？註冊 passkey 前的最後一道確認。
/// 給一個短的有效窗口，避免「幾天前驗過」還能拿來建帳號。
pub fn otp_recently_verified(db: &Db, email: &str, window_secs: i64) -> Result<bool> {
    let conn = db.lock().unwrap();
    let v: Option<i64> = conn
        .query_row(
            "SELECT verified_at FROM email_otp WHERE email = ?1",
            params![norm(email)],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    Ok(v.is_some_and(|t| now() - t <= window_secs))
}

/// 不經驗證碼直接把信箱標成「剛剛證實過」。給邀請連結用 ——
/// 連結只寄得到那個信箱，收得到它跟收得到一組碼證明的是同一件事。
///
/// 刻意寫進 `email_otp` 而不是另開一張表：註冊那關讀的就是這裡，
/// 兩條路徑共用同一個「最近驗證過」的時窗，不會出現第二套過期規則。
/// `code_hash` 放一個不可能等於任何 HMAC 十六進位輸出的值，`expires_at`
/// 直接給 0 —— 這一筆只代表「驗過了」，不是一組能拿去輸入的碼。
///
/// `sent_at` 新建時給 0、既有的不動：那欄是重寄冷卻的基準，而這裡根本沒寄
/// 任何東西。給 `now()` 的話，走完連結又想退回「用 Email 加入」的人會被
/// 擋在「請等 60 秒」後面，而他等的是一封從來沒寄出的信。
pub fn mark_email_verified(db: &Db, email: &str) -> Result<()> {
    let conn = db.lock().unwrap();
    conn.execute(
        "INSERT INTO email_otp (email, code_hash, expires_at, attempts, sent_at, verified_at)
         VALUES (?1,'-',0,0,0,?2)
         ON CONFLICT(email) DO UPDATE SET
             code_hash='-', expires_at=0, attempts=0, verified_at=excluded.verified_at",
        params![norm(email), now()],
    )?;
    Ok(())
}

pub fn clear_otp(db: &Db, email: &str) -> Result<()> {
    let conn = db.lock().unwrap();
    conn.execute("DELETE FROM email_otp WHERE email = ?1", params![norm(email)])?;
    Ok(())
}

// ── 平台分權 ──────────────────────────────────────────

pub fn grant_platform(db: &Db, user_id: &str, platform: &str, by: &str) -> Result<()> {
    let conn = db.lock().unwrap();
    conn.execute(
        "INSERT OR IGNORE INTO user_platforms (user_id, platform, granted_by, granted_at)
         VALUES (?1,?2,?3,?4)",
        params![user_id, platform, by, now()],
    )?;
    Ok(())
}

pub fn revoke_platform(db: &Db, user_id: &str, platform: &str) -> Result<usize> {
    let conn = db.lock().unwrap();
    Ok(conn.execute(
        "DELETE FROM user_platforms WHERE user_id = ?1 AND platform = ?2",
        params![user_id, platform],
    )?)
}

pub fn platforms_for(db: &Db, user_id: &str) -> Result<Vec<String>> {
    let conn = db.lock().unwrap();
    let mut stmt =
        conn.prepare("SELECT platform FROM user_platforms WHERE user_id = ?1 ORDER BY platform")?;
    let rows = stmt.query_map(params![user_id], |r| r.get::<_, String>(0))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// 成員管理頁要一次列出所有人的授權，不能每人打一次查詢。
pub fn all_platform_grants(db: &Db) -> Result<Vec<(String, String)>> {
    let conn = db.lock().unwrap();
    let mut stmt = conn.prepare("SELECT user_id, platform FROM user_platforms")?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// 認領一則「授權快到期」提醒。回 true 代表這一輪由你負責通知。
///
/// 用 UPDATE 的回傳列數當閘門而不先讀再寫 —— 天生冪等，重跑不會重複提醒。
pub fn claim_expiry_notice(db: &Db, ip: &str) -> Result<bool> {
    let conn = db.lock().unwrap();
    Ok(conn.execute(
        "UPDATE allowlist SET expiry_notified_at = ?2
         WHERE ip = ?1 AND expiry_notified_at IS NULL",
        params![ip, now()],
    )? > 0)
}

// ── 推送通知 ──────────────────────────────────────────

/// 一台裝置的推送訂閱。
#[derive(Debug, Clone, Serialize)]
pub struct PushSub {
    pub id: i64,
    #[serde(skip)]
    pub user_id: String,
    /// 推送服務的網址。**不外流到前端** —— 它等於「可以推播到這台裝置」
    /// 的能力，列表只需要看得出是哪台。
    #[serde(skip)]
    pub endpoint: String,
    #[serde(skip)]
    pub p256dh: String,
    #[serde(skip)]
    pub auth: String,
    pub label: Option<String>,
    pub created_at: i64,
    pub last_ok_at: Option<i64>,
    pub fail_count: i64,
}

const PUSH_COLS: &str =
    "id, user_id, endpoint, p256dh, auth, label, created_at, last_ok_at, fail_count";

fn row_to_push(r: &rusqlite::Row) -> rusqlite::Result<PushSub> {
    Ok(PushSub {
        id: r.get(0)?,
        user_id: r.get(1)?,
        endpoint: r.get(2)?,
        p256dh: r.get(3)?,
        auth: r.get(4)?,
        label: r.get(5)?,
        created_at: r.get(6)?,
        last_ok_at: r.get(7)?,
        fail_count: r.get(8)?,
    })
}

/// 新增或更新一筆訂閱。
///
/// endpoint 衝突時整筆蓋掉並把 `fail_count` 歸零 —— 那台裝置又活著了。
/// `user_id` 也一起更新：同一台裝置可能換人登入。
pub fn add_push_sub(
    db: &Db,
    user_id: &str,
    endpoint: &str,
    p256dh: &str,
    auth: &str,
    label: Option<&str>,
) -> Result<()> {
    let conn = db.lock().unwrap();
    conn.execute(
        "INSERT INTO push_subscriptions
             (user_id, endpoint, p256dh, auth, label, created_at)
         VALUES (?1,?2,?3,?4,?5,?6)
         ON CONFLICT(endpoint) DO UPDATE SET
             user_id = excluded.user_id, p256dh = excluded.p256dh,
             auth = excluded.auth, label = excluded.label, fail_count = 0",
        params![user_id, endpoint.trim(), p256dh, auth, label, now()],
    )?;
    Ok(())
}

pub fn list_push_subs(db: &Db, user_id: &str) -> Result<Vec<PushSub>> {
    let conn = db.lock().unwrap();
    let mut stmt = conn.prepare(&format!(
        "SELECT {PUSH_COLS} FROM push_subscriptions WHERE user_id = ?1 ORDER BY created_at"
    ))?;
    let rows = stmt.query_map(params![user_id], row_to_push)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// 這個 endpoint 還在不在。
///
/// 給裝置自己對帳用：瀏覽器端的訂閱是本機物件，別台裝置在設定裡把它撤掉
/// 之後這邊完全沒有感覺 —— 面板會一直顯示「已開啟」，實際上推不到了。
pub fn push_sub_exists(db: &Db, endpoint: &str, user_id: &str) -> Result<bool> {
    let conn = db.lock().unwrap();
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM push_subscriptions WHERE endpoint = ?1 AND user_id = ?2",
        params![endpoint.trim(), user_id],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// 撤銷一筆訂閱。WHERE 帶上 `user_id` 而不只是 `id` ——
/// 一律只能操作自己的，跟 passkey 那邊同一個原則。
pub fn delete_push_sub(db: &Db, id: i64, user_id: &str) -> Result<usize> {
    let conn = db.lock().unwrap();
    Ok(conn.execute(
        "DELETE FROM push_subscriptions WHERE id = ?1 AND user_id = ?2",
        params![id, user_id],
    )?)
}

/// 使用者在自己裝置上按下「關閉通知」時，前端帶著 endpoint 來退訂。
///
/// endpoint 對**擁有它的那台裝置**不是秘密（是它自己建的），但仍然帶上
/// `user_id` 比對 —— 猜到別人的 endpoint 不該就能把對方的通知關掉。
pub fn delete_push_sub_for_user(db: &Db, endpoint: &str, user_id: &str) -> Result<usize> {
    let conn = db.lock().unwrap();
    Ok(conn.execute(
        "DELETE FROM push_subscriptions WHERE endpoint = ?1 AND user_id = ?2",
        params![endpoint.trim(), user_id],
    )?)
}

/// 推送服務回 404／410 時當場清掉。那是它在說「這個訂閱不存在了」——
/// 留著只會每次都失敗，而且沒有任何辦法自己好起來。
///
/// 這支**不帶 user_id**：呼叫端是推送迴圈，它拿到的 endpoint 來自 DB
/// 自己，而且此時要清的正是「這個 endpoint 不管屬於誰都已經死了」。
pub fn delete_push_sub_by_endpoint(db: &Db, endpoint: &str) -> Result<usize> {
    let conn = db.lock().unwrap();
    Ok(conn.execute(
        "DELETE FROM push_subscriptions WHERE endpoint = ?1",
        params![endpoint.trim()],
    )?)
}

/// 該收到某個平台新驗證碼通知的所有訂閱。
///
/// 平台過濾走 `user_platforms`，跟驗證碼分頁同一條規則（見 `mail_list`）——
/// 分兩份寫遲早會歪。admin 一樣要被授權，沒有特例。
pub fn push_subs_for_platform(db: &Db, platform: &str) -> Result<Vec<PushSub>> {
    let conn = db.lock().unwrap();
    let mut stmt = conn.prepare(&format!(
        "SELECT {} FROM push_subscriptions s
         JOIN users u ON u.id = s.user_id
         JOIN user_platforms p ON p.user_id = s.user_id
         WHERE p.platform = ?1 AND u.notify_codes = 1",
        PUSH_COLS
            .split(", ")
            .map(|c| format!("s.{c}"))
            .collect::<Vec<_>>()
            .join(", ")
    ))?;
    let rows = stmt.query_map(params![platform], row_to_push)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// 某個人所有開著「授權快到期」的訂閱。
pub fn push_subs_for_expiry(db: &Db, user_id: &str) -> Result<Vec<PushSub>> {
    let conn = db.lock().unwrap();
    let mut stmt = conn.prepare(&format!(
        "SELECT {} FROM push_subscriptions s
         JOIN users u ON u.id = s.user_id
         WHERE s.user_id = ?1 AND u.notify_expiry = 1",
        PUSH_COLS
            .split(", ")
            .map(|c| format!("s.{c}"))
            .collect::<Vec<_>>()
            .join(", ")
    ))?;
    let rows = stmt.query_map(params![user_id], row_to_push)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn mark_push_ok(db: &Db, id: i64) -> Result<()> {
    let conn = db.lock().unwrap();
    conn.execute(
        "UPDATE push_subscriptions SET last_ok_at = ?2, fail_count = 0 WHERE id = ?1",
        params![id, now()],
    )?;
    Ok(())
}

pub fn bump_push_fail(db: &Db, id: i64) -> Result<()> {
    let conn = db.lock().unwrap();
    conn.execute(
        "UPDATE push_subscriptions SET fail_count = fail_count + 1 WHERE id = ?1",
        params![id],
    )?;
    Ok(())
}

/// 設計 3a 的兩顆推桿：(新驗證碼, 授權快到期)。
pub fn notify_prefs(db: &Db, user_id: &str) -> Result<(bool, bool)> {
    let conn = db.lock().unwrap();
    Ok(conn.query_row(
        "SELECT notify_codes, notify_expiry FROM users WHERE id = ?1",
        params![user_id],
        |r| Ok((r.get::<_, i64>(0)? != 0, r.get::<_, i64>(1)? != 0)),
    )?)
}

pub fn set_notify_prefs(db: &Db, user_id: &str, codes: bool, expiry: bool) -> Result<()> {
    let conn = db.lock().unwrap();
    conn.execute(
        "UPDATE users SET notify_codes = ?2, notify_expiry = ?3 WHERE id = ?1",
        params![user_id, codes as i64, expiry as i64],
    )?;
    Ok(())
}

// ── Bootstrap 一次性碼 ────────────────────────────────

pub fn create_bootstrap(db: &Db, token: &str) -> Result<()> {
    let conn = db.lock().unwrap();
    conn.execute(
        "INSERT INTO bootstrap (token, created_at) VALUES (?1, ?2)",
        params![token, now()],
    )?;
    Ok(())
}

/// 只檢查不消耗，實際消耗在註冊流程的 finish 階段。
pub fn peek_bootstrap(db: &Db, token: &str) -> Result<bool> {
    let conn = db.lock().unwrap();
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM bootstrap WHERE token = ?1 AND used_at IS NULL",
        params![token],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// 檢查並消耗一次性碼。回傳 true 表示驗證通過且已標記為使用。
pub fn consume_bootstrap(db: &Db, token: &str) -> Result<bool> {
    let conn = db.lock().unwrap();
    let n = conn.execute(
        "UPDATE bootstrap SET used_at = ?1 WHERE token = ?2 AND used_at IS NULL",
        params![now(), token],
    )?;
    Ok(n == 1)
}

pub fn has_unused_bootstrap(db: &Db) -> Result<bool> {
    let conn = db.lock().unwrap();
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM bootstrap WHERE used_at IS NULL",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem() -> Db {
        test_db()
    }

    /// 位址大小寫須正規化，路由查詢靠字串比對。
    #[test]
    fn recipient_lookup_is_case_insensitive() {
        let db = mem();
        add_recipient(&db, "Netflix@Share.Example.com", "  Someone@Example.COM ", None, "me").unwrap();
        let got = enabled_recipients_for(&db, "netflix@share.example.com").unwrap();
        assert_eq!(got, vec!["someone@example.com"]);
    }

    /// 停用後不應再出現在路由結果中。
    #[test]
    fn disabled_recipients_are_not_routed() {
        let db = mem();
        add_recipient(&db, "netflix@x.tw", "a@e.com", None, "me").unwrap();
        add_recipient(&db, "netflix@x.tw", "b@e.com", None, "me").unwrap();
        let id = list_recipients(&db).unwrap().iter().find(|r| r.address == "a@e.com").unwrap().id;

        assert_eq!(set_recipient_enabled(&db, id, false).unwrap(), 1);
        assert_eq!(enabled_recipients_for(&db, "netflix@x.tw").unwrap(), vec!["b@e.com"]);

        // 停用不等於刪除，之後可恢復
        assert_eq!(list_recipients(&db).unwrap().len(), 2);
        set_recipient_enabled(&db, id, true).unwrap();
        assert_eq!(enabled_recipients_for(&db, "netflix@x.tw").unwrap().len(), 2);
    }

    /// 不同信箱的收件人不得互相外溢。
    #[test]
    fn mailboxes_are_isolated() {
        let db = mem();
        add_recipient(&db, "netflix@x.tw", "a@e.com", None, "me").unwrap();
        add_recipient(&db, "disney@x.tw", "b@e.com", None, "me").unwrap();
        assert_eq!(enabled_recipients_for(&db, "netflix@x.tw").unwrap(), vec!["a@e.com"]);
        assert_eq!(enabled_recipients_for(&db, "disney@x.tw").unwrap(), vec!["b@e.com"]);
        // 未知信箱不回傳任何人
        assert!(enabled_recipients_for(&db, "unknown@x.tw").unwrap().is_empty());
    }

    /// 重複新增同一人 = 恢復啟用並更新備註，不產生第二筆。
    #[test]
    fn re_adding_reenables() {
        let db = mem();
        add_recipient(&db, "netflix@x.tw", "a@e.com", Some("舊"), "me").unwrap();
        let id = list_recipients(&db).unwrap()[0].id;
        set_recipient_enabled(&db, id, false).unwrap();

        add_recipient(&db, "netflix@x.tw", "a@e.com", Some("新"), "me").unwrap();
        let all = list_recipients(&db).unwrap();
        assert_eq!(all.len(), 1, "不該產生重複紀錄");
        assert!(all[0].enabled);
        assert_eq!(all[0].label.as_deref(), Some("新"));
    }

    /// 造一顆 v6 之前的資料庫，塞進線上那顆會有的東西。
    /// 升級路徑必須在**有資料**的情況下驗證，空庫跑得過不代表什麼。
    fn v5_db_with_data() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        migrate_v1(&conn).unwrap();
        migrate_v2(&conn).unwrap();
        migrate_v3(&conn).unwrap();
        conn.execute_batch("ALTER TABLE mails ADD COLUMN html TEXT;").unwrap();
        migrate_v5(&conn).unwrap();
        conn.pragma_update(None, "user_version", 5).unwrap();

        conn.execute_batch(
            "INSERT INTO users (id, username, display_name, role, created_at)
                 VALUES ('u1', 'alex', 'alex', 'admin', 100);
             INSERT INTO credentials (id, user_id, passkey, created_at)
                 VALUES ('c1', 'u1', '{}', 100);
             INSERT INTO allowlist (ip, label, added_by, added_at, expires_at)
                 VALUES ('1.2.3.4', '家裡', 'alex', 100, 999999999999);
             INSERT INTO mails (message_id, received_at, subject, code)
                 VALUES ('m1', 100, '登入驗證碼', '3849');
             INSERT INTO mail_recipients (mailbox, address, added_at)
                 VALUES ('netflix@x.tw', 'a@e.com', 100);",
        )
        .unwrap();
        conn
    }

    /// v5 → v6 不得動到任何既有資料。這條路徑會跑在線上那顆
    /// 有帳號、有 passkey、有白名單的資料庫上，失敗是不可逆的。
    #[test]
    fn v6_upgrade_preserves_existing_data() {
        let conn = v5_db_with_data();
        migrate(&conn).unwrap();

        let db = Arc::new(Mutex::new(conn));

        let u = get_user(&db, "u1").unwrap().expect("帳號不該消失");
        assert_eq!(u.username, "alex", "username 是 WebAuthn user handle，不能被改寫");
        assert_eq!(u.email, None, "舊帳號沒有 email，應為 NULL 而不是空字串");
        assert_eq!(u.label(), "alex", "還沒補 email 時顯示名退回 username");
        assert_eq!(credential_count(&db, "u1").unwrap(), 1, "passkey 不該掉");

        let entries = list_allow(&db).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].label.as_deref(), Some("家裡"));
        assert_eq!(entries[0].ttl_days, 7, "既有條目沿用舊的固定 7 天");
        assert_eq!(entries[0].renewed_at, None);

        let mails = recent_mails(&db, 10).unwrap();
        assert_eq!(mails.len(), 1);
        assert_eq!(mails[0].code.as_deref(), Some("3849"));
        assert_eq!(mails[0].platform, None, "舊信件沒有平台歸屬");

        let r = &list_recipients(&db).unwrap()[0];
        assert_eq!(r.cf_checked_at, None, "還沒查過 Cloudflare");
    }

    /// 多個舊帳號的 email 都是 NULL，唯一索引不該把它們當成衝突。
    #[test]
    fn null_emails_do_not_collide() {
        let db = mem();
        create_user_with_platforms(&db, "a", "alice", "alice", "admin", None, &[]).unwrap();
        create_user_with_platforms(&db, "b", "bob", "bob", "member", None, &[]).unwrap();
        assert_eq!(list_users(&db).unwrap().len(), 2);
    }

    /// 額度是 per-user，各人互不影響。
    #[test]
    fn quota_is_counted_per_user() {
        let db = mem();
        upsert_allow(&db, "1.1.1.1", None, Some("alice"), 999, 7).unwrap();
        upsert_allow(&db, "2.2.2.2", None, Some("alice"), 999, 7).unwrap();
        upsert_allow(&db, "3.3.3.3", None, Some("bob"), 999, 7).unwrap();
        assert_eq!(allow_count_by(&db, "alice").unwrap(), 2);
        assert_eq!(allow_count_by(&db, "bob").unwrap(), 1);
    }

    /// 「延長授權」不帶名稱時，不該把既有的名稱洗掉。
    #[test]
    fn renewing_keeps_existing_label() {
        let db = mem();
        upsert_allow(&db, "1.1.1.1", Some("咖啡廳"), Some("alice"), 100, 7).unwrap();
        upsert_allow(&db, "1.1.1.1", None, Some("alice"), 200, 30).unwrap();
        let e = &list_allow(&db).unwrap()[0];
        assert_eq!(e.label.as_deref(), Some("咖啡廳"));
        assert_eq!(e.expires_at, 200);
        assert_eq!(e.ttl_days, 30, "改天數要生效");
    }

    /// 自動續期要依條目自己的 ttl_days，不是全域預設。
    #[test]
    fn renewal_uses_entry_own_ttl() {
        let db = mem();
        upsert_allow(&db, "1.1.1.1", None, Some("alice"), 100, 30).unwrap();
        let new_exp = renew_allow(&db, "1.1.1.1").unwrap().unwrap();
        assert_eq!(new_exp, now() + 30 * 86400);
        assert!(list_allow(&db).unwrap()[0].renewed_at.is_some(), "要看得出是機器續的");
    }

    /// 同一個登記位址不能被兩個人拿去各建一個帳號。
    #[test]
    fn invited_email_is_single_use() {
        let db = mem();
        invite_email(&db, "Mei@Example.com", "admin", &[]).unwrap();
        assert!(is_email_invited(&db, "mei@example.com").unwrap());

        assert!(consume_invited_email(&db, "mei@example.com", "u1").unwrap().is_some());
        assert!(
            consume_invited_email(&db, "mei@example.com", "u2").unwrap().is_none(),
            "第二次必須失敗"
        );
        assert!(!is_email_invited(&db, "mei@example.com").unwrap());
    }

    /// 撤銷後不能註冊；重新登記等於解除撤銷，不必換一個位址。
    #[test]
    fn revoking_blocks_then_reinviting_restores() {
        let db = mem();
        invite_email(&db, "mei@example.com", "admin", &[]).unwrap();
        revoke_invited_email(&db, "mei@example.com").unwrap();
        assert!(!is_email_invited(&db, "mei@example.com").unwrap());

        invite_email(&db, "mei@example.com", "admin", &[]).unwrap();
        assert!(is_email_invited(&db, "mei@example.com").unwrap());
    }

    /// 已註冊的位址不該被「撤銷」偷偷解除使用標記。
    #[test]
    fn revoking_does_not_free_a_used_invite() {
        let db = mem();
        invite_email(&db, "mei@example.com", "admin", &[]).unwrap();
        consume_invited_email(&db, "mei@example.com", "u1").unwrap();
        assert_eq!(revoke_invited_email(&db, "mei@example.com").unwrap(), 0);
        assert!(!is_email_invited(&db, "mei@example.com").unwrap());
    }

    /// 連結權杖的生命週期跟著登記走：註冊完成就換不到東西了。
    #[test]
    fn invite_token_dies_with_the_invite() {
        let db = mem();
        invite_email(&db, "Mei@Example.com", "admin", &["netflix".into()]).unwrap();
        assert!(set_invite_token(&db, "mei@example.com", "h1").unwrap());

        let row = invited_email_by_token(&db, "h1").unwrap().expect("查得到");
        assert_eq!(row.email, "mei@example.com");
        assert_eq!(row.platforms, vec!["netflix"], "連結要帶得出登記時選的平台");

        consume_invited_email(&db, "mei@example.com", "u1").unwrap();
        assert!(
            invited_email_by_token(&db, "h1").unwrap().is_none(),
            "註冊完成後同一條連結不能再換一個帳號"
        );
    }

    /// 撤銷 = 連結當場失效，而且雜湊也不留在表裡。
    #[test]
    fn revoking_kills_the_link() {
        let db = mem();
        invite_email(&db, "mei@example.com", "admin", &[]).unwrap();
        set_invite_token(&db, "mei@example.com", "h1").unwrap();
        revoke_invited_email(&db, "mei@example.com").unwrap();

        assert!(invited_email_by_token(&db, "h1").unwrap().is_none());
        // 撤銷過的登記不該還能被掛上新連結
        assert!(!set_invite_token(&db, "mei@example.com", "h2").unwrap());
    }

    /// 重新登記 = 換一條連結。舊信裡的按鈕不能繞過剛改好的設定。
    #[test]
    fn reinviting_invalidates_the_old_link() {
        let db = mem();
        invite_email(&db, "mei@example.com", "admin", &["netflix".into()]).unwrap();
        set_invite_token(&db, "mei@example.com", "old").unwrap();

        invite_email(&db, "mei@example.com", "admin", &["disneyplus".into()]).unwrap();
        assert!(invited_email_by_token(&db, "old").unwrap().is_none());

        set_invite_token(&db, "mei@example.com", "new").unwrap();
        let row = invited_email_by_token(&db, "new").unwrap().unwrap();
        assert_eq!(row.platforms, vec!["disneyplus"], "拿到的要是改過的那份");
    }

    /// 邀請連結按下去 = 信箱已證實，接上的是註冊那關讀的同一個時窗。
    #[test]
    fn marking_verified_opens_the_registration_window() {
        let db = mem();
        assert!(!otp_recently_verified(&db, "mei@example.com", 900).unwrap());
        mark_email_verified(&db, "Mei@Example.com").unwrap();
        assert!(otp_recently_verified(&db, "mei@example.com", 900).unwrap());
    }

    /// 但那一筆不是一組能拿去輸入的碼 —— 它連「還沒過期」都不是。
    #[test]
    fn marking_verified_does_not_hand_out_a_usable_code() {
        let db = mem();
        mark_email_verified(&db, "mei@example.com").unwrap();
        assert!(matches!(
            check_otp(&db, "mei@example.com", "-", 5).unwrap(),
            OtpCheck::Expired
        ));
    }

    /// 走完連結還想退回「用 Email 加入」的人，不該被一段從沒寄出的信的
    /// 冷卻擋住。
    #[test]
    fn marking_verified_does_not_start_a_resend_cooldown() {
        let db = mem();
        mark_email_verified(&db, "mei@example.com").unwrap();
        assert_eq!(otp_cooldown(&db, "mei@example.com", 60).unwrap(), 0);
    }

    /// 反過來，真的寄過碼的那筆冷卻要留著 —— 邀請連結不是繞過它的路。
    #[test]
    fn marking_verified_keeps_an_existing_cooldown() {
        let db = mem();
        put_otp(&db, "mei@example.com", "h", 600).unwrap();
        mark_email_verified(&db, "mei@example.com").unwrap();
        assert!(otp_cooldown(&db, "mei@example.com", 60).unwrap() > 0);
    }

    /// 寄過碼的信箱後來改走連結，舊碼要跟著失效 ——
    /// 否則那組還在信箱裡的碼會多活一個時窗。
    #[test]
    fn marking_verified_replaces_a_pending_code() {
        let db = mem();
        let h = crate::otp::hash(&db, "mei@example.com", "123456").unwrap();
        put_otp(&db, "mei@example.com", &h, 600).unwrap();
        mark_email_verified(&db, "mei@example.com").unwrap();
        assert!(matches!(check_otp(&db, "mei@example.com", &h, 5).unwrap(), OtpCheck::Expired));
    }

    /// 六位數只有一百萬種，沒有次數上限等於沒有保護。
    #[test]
    fn otp_locks_out_after_too_many_attempts() {
        let db = mem();
        put_otp(&db, "mei@example.com", "hash-good", 600).unwrap();

        for _ in 0..3 {
            assert!(matches!(
                check_otp(&db, "mei@example.com", "hash-bad", 3).unwrap(),
                OtpCheck::Wrong
            ));
        }
        // 用完次數之後，連正確的碼也不放行
        assert!(matches!(
            check_otp(&db, "mei@example.com", "hash-good", 3).unwrap(),
            OtpCheck::TooManyAttempts
        ));
    }

    /// 重寄必須讓舊碼當場失效，否則會多開一個可用窗口。
    #[test]
    fn resending_invalidates_the_previous_code() {
        let db = mem();
        put_otp(&db, "mei@example.com", "hash-old", 600).unwrap();
        put_otp(&db, "mei@example.com", "hash-new", 600).unwrap();

        assert!(matches!(
            check_otp(&db, "mei@example.com", "hash-old", 3).unwrap(),
            OtpCheck::Wrong
        ));
        assert!(matches!(
            check_otp(&db, "mei@example.com", "hash-new", 3).unwrap(),
            OtpCheck::Ok
        ));
        assert!(otp_recently_verified(&db, "mei@example.com", 600).unwrap());
    }

    #[test]
    fn expired_otp_is_rejected() {
        let db = mem();
        put_otp(&db, "mei@example.com", "h", -1).unwrap();
        assert!(matches!(
            check_otp(&db, "mei@example.com", "h", 3).unwrap(),
            OtpCheck::Expired
        ));
    }

    /// 通過驗證的時效很短 —— 幾天前驗過的信箱不該還能拿來建帳號。
    #[test]
    fn stale_verification_does_not_count() {
        let db = mem();
        put_otp(&db, "mei@example.com", "h", 600).unwrap();
        check_otp(&db, "mei@example.com", "h", 3).unwrap();
        assert!(!otp_recently_verified(&db, "mei@example.com", -1).unwrap());
    }

    /// UI 改過的設定不該被下一次重啟的環境變數種子蓋掉。
    #[test]
    fn seeding_never_overwrites_a_configured_value() {
        let db = mem();
        seed_setting(&db, keys::SENDER_MODE, "observe").unwrap();
        set_setting(&db, keys::SENDER_MODE, "enforce", Some("alex")).unwrap();
        seed_setting(&db, keys::SENDER_MODE, "observe").unwrap();
        assert_eq!(get_setting(&db, keys::SENDER_MODE).unwrap().as_deref(), Some("enforce"));
    }

    /// 設定壞掉只該讓該項失效，不該讓收信整條掛掉。
    #[test]
    fn malformed_setting_list_reads_as_empty() {
        let db = mem();
        set_setting(&db, keys::CODE_KEYWORDS, "這不是 JSON", None).unwrap();
        assert!(get_setting_list(&db, keys::CODE_KEYWORDS).is_empty());
    }

    /// 刪掉使用者要一併帶走他的平台授權，不能留下孤兒。
    #[test]
    fn platform_grants_follow_the_user() {
        let db = mem();
        create_user_with_platforms(&db, "u1", "alex", "alex", "admin", Some("ALEX@x.tw"), &[]).unwrap();
        grant_platform(&db, "u1", "netflix", "admin").unwrap();
        grant_platform(&db, "u1", "disneyplus", "admin").unwrap();
        assert_eq!(platforms_for(&db, "u1").unwrap(), vec!["disneyplus", "netflix"]);

        assert_eq!(revoke_platform(&db, "u1", "disneyplus").unwrap(), 1);
        assert_eq!(platforms_for(&db, "u1").unwrap(), vec!["netflix"]);

        db.lock().unwrap().execute("DELETE FROM users WHERE id='u1'", []).unwrap();
        assert!(platforms_for(&db, "u1").unwrap().is_empty(), "ON DELETE CASCADE 沒生效");
    }

    /// Email 一律正規化：大小寫不同不該變成兩個人。
    #[test]
    fn email_lookup_is_case_insensitive() {
        let db = mem();
        create_user_with_platforms(&db, "u1", "alex", "alex", "admin", Some("  ALEX@Example.COM "), &[]).unwrap();
        let u = find_user_by_email(&db, "alex@example.com").unwrap().unwrap();
        assert_eq!(u.id, "u1");
        assert_eq!(u.email.as_deref(), Some("alex@example.com"));
        assert_eq!(u.label(), "alex@example.com", "有 email 就以 email 稱呼");
    }

    /// 補填 email 之後，這個人既有的白名單條目不能變成「不是我的」。
    /// added_by 存的是稱呼不是 id，稱呼變了要一起搬。
    #[test]
    fn backfilling_email_moves_ownership() {
        let db = mem();
        create_user_with_platforms(&db, "u1", "alex", "alex", "admin", None, &[]).unwrap();
        upsert_allow(&db, "1.1.1.1", Some("家裡"), Some("alex"), 999, 7).unwrap();
        upsert_allow(&db, "2.2.2.2", None, Some("someone-else"), 999, 7).unwrap();
        assert_eq!(allow_count_by(&db, "alex").unwrap(), 1);

        set_user_email(&db, "u1", "alex@example.com").unwrap();
        assert_eq!(rename_owner(&db, "alex", "alex@example.com").unwrap(), 1);

        assert_eq!(allow_count_by(&db, "alex@example.com").unwrap(), 1);
        assert_eq!(allow_count_by(&db, "alex").unwrap(), 0);
        // 別人的條目不能被順手搬走
        assert_eq!(allow_count_by(&db, "someone-else").unwrap(), 1);
    }

    /// credential id 會出現在登入回應裡，不是機密。刪除必須綁上擁有者，
    /// 否則拿到別人的 id 就能把他的登入手段刪掉。
    #[test]
    fn credentials_can_only_be_touched_by_their_owner() {
        let db = mem();
        create_user_with_platforms(&db, "u1", "a", "a", "admin", None, &[]).unwrap();
        create_user_with_platforms(&db, "u2", "b", "b", "member", None, &[]).unwrap();
        add_credential(&db, "c1", "u1", "{}", Some("iPhone")).unwrap();
        add_credential(&db, "c2", "u1", "{}", None).unwrap();

        // u2 拿著 u1 的 credential id 也刪不掉、改不動
        assert_eq!(delete_credential(&db, "u2", "c1").unwrap(), 0);
        assert_eq!(rename_credential(&db, "u2", "c1", Some("壞了")).unwrap(), 0);
        assert_eq!(credential_count(&db, "u1").unwrap(), 2);

        assert_eq!(delete_credential(&db, "u1", "c1").unwrap(), 1);
        assert_eq!(credential_count(&db, "u1").unwrap(), 1);
    }

    /// 列出時不能把憑證材料一起送出去 —— 前端只需要辨認裝置的資訊。
    #[test]
    fn listing_credentials_omits_the_key_material() {
        let db = mem();
        create_user_with_platforms(&db, "u1", "a", "a", "admin", None, &[]).unwrap();
        add_credential(&db, "c1", "u1", "{\"secret\":\"不該外流\"}", Some("iPhone")).unwrap();

        let list = list_credentials(&db, "u1").unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].nickname.as_deref(), Some("iPhone"));
        assert_eq!(list[0].last_used_at, None, "剛註冊還沒用過");
        let json = serde_json::to_string(&list).unwrap();
        assert!(!json.contains("不該外流"), "序列化不得帶出 passkey 欄位");
    }

    /// v6 之前註冊的沒有暱稱，之後可以補上；清空則回到 None。
    #[test]
    fn nickname_can_be_set_and_cleared() {
        let db = mem();
        create_user_with_platforms(&db, "u1", "a", "a", "admin", None, &[]).unwrap();
        add_credential(&db, "c1", "u1", "{}", None).unwrap();
        assert_eq!(list_credentials(&db, "u1").unwrap()[0].nickname, None);

        rename_credential(&db, "u1", "c1", Some("備援金鑰")).unwrap();
        assert_eq!(list_credentials(&db, "u1").unwrap()[0].nickname.as_deref(), Some("備援金鑰"));

        rename_credential(&db, "u1", "c1", None).unwrap();
        assert_eq!(list_credentials(&db, "u1").unwrap()[0].nickname, None);
    }

    /// 登入會更新 last_used_at，帳號頁靠它分辨「哪一把還在用」。
    #[test]
    fn using_a_credential_records_the_time() {
        let db = mem();
        create_user_with_platforms(&db, "u1", "a", "a", "admin", None, &[]).unwrap();
        add_credential(&db, "c1", "u1", "{}", None).unwrap();
        touch_credential(&db, "c1").unwrap();
        assert!(list_credentials(&db, "u1").unwrap()[0].last_used_at.is_some());
    }

    /// 登記時選的平台要原封不動傳到註冊完成的那一刻。
    #[test]
    fn invite_carries_its_platform_grants() {
        let db = mem();
        invite_email(&db, "mei@example.com", "admin", &["netflix".into(), "disneyplus".into()])
            .unwrap();

        let row = &list_invited_emails(&db).unwrap()[0];
        assert_eq!(row.platforms, vec!["netflix", "disneyplus"]);

        let got = consume_invited_email(&db, "mei@example.com", "u1").unwrap().unwrap();
        assert_eq!(got, vec!["netflix", "disneyplus"]);
    }

    /// 重新登記等於改這筆登記 —— 平台也要跟著換，不能只解除撤銷。
    #[test]
    fn re_inviting_replaces_the_platform_grants() {
        let db = mem();
        invite_email(&db, "mei@example.com", "admin", &["netflix".into()]).unwrap();
        invite_email(&db, "mei@example.com", "admin", &["disneyplus".into()]).unwrap();
        assert_eq!(list_invited_emails(&db).unwrap()[0].platforms, vec!["disneyplus"]);
    }

    /// v7 之前登記的沒有這一欄，要讀成空的而不是炸掉。
    #[test]
    fn invites_from_before_v7_have_no_platforms() {
        let db = mem();
        {
            let conn = db.lock().unwrap();
            conn.execute(
                "INSERT INTO invited_emails (email, invited_by, invited_at)
                 VALUES ('old@x.tw', 'admin', 100)",
                [],
            )
            .unwrap();
        }
        assert!(list_invited_emails(&db).unwrap()[0].platforms.is_empty());
        assert_eq!(
            consume_invited_email(&db, "old@x.tw", "u1").unwrap(),
            Some(vec![]),
            "沒有平台不等於不能註冊"
        );
    }

    /// 移除一個人必須連他授權的網路一起帶走 —— 留著等於沒有移除。
    #[test]
    fn removing_a_user_takes_their_networks_with_them() {
        let db = mem();
        create_user_with_platforms(&db, "u1", "mei@x.tw", "mei", "member", Some("mei@x.tw"), &[]).unwrap();
        create_user_with_platforms(&db, "u2", "other@x.tw", "other", "admin", Some("other@x.tw"), &[]).unwrap();
        add_credential(&db, "c1", "u1", "{}", None).unwrap();
        grant_platform(&db, "u1", "netflix", "admin").unwrap();
        upsert_allow(&db, "1.1.1.1", None, Some("mei@x.tw"), 999, 7).unwrap();
        upsert_allow(&db, "2.2.2.2", None, Some("mei@x.tw"), 999, 7).unwrap();
        upsert_allow(&db, "3.3.3.3", None, Some("other@x.tw"), 999, 7).unwrap();
        invite_email(&db, "mei@x.tw", "admin", &["netflix".into()]).unwrap();
        consume_invited_email(&db, "mei@x.tw", "u1").unwrap();

        assert_eq!(delete_user(&db, "u1", "mei@x.tw").unwrap().entries, 2, "他的兩筆白名單");

        assert!(get_user(&db, "u1").unwrap().is_none());
        assert_eq!(credential_count(&db, "u1").unwrap(), 0, "passkey 要 CASCADE 掉");
        assert!(platforms_for(&db, "u1").unwrap().is_empty(), "平台授權要 CASCADE 掉");
        assert_eq!(allow_count_by(&db, "mei@x.tw").unwrap(), 0);

        // 別人的東西不能被順手帶走
        assert_eq!(allow_count_by(&db, "other@x.tw").unwrap(), 1);
        assert!(get_user(&db, "u2").unwrap().is_some());
    }

    /// 移除帳號後，那個位址要能重新登記 —— 否則它會永遠卡在「已使用」。
    #[test]
    fn removing_a_user_frees_their_invited_address() {
        let db = mem();
        create_user_with_platforms(&db, "u1", "mei@x.tw", "mei", "member", Some("mei@x.tw"), &[]).unwrap();
        invite_email(&db, "mei@x.tw", "admin", &[]).unwrap();
        consume_invited_email(&db, "mei@x.tw", "u1").unwrap();
        assert!(!is_email_invited(&db, "mei@x.tw").unwrap());

        delete_user(&db, "u1", "mei@x.tw").unwrap();
        invite_email(&db, "mei@x.tw", "admin", &["netflix".into()]).unwrap();
        assert!(is_email_invited(&db, "mei@x.tw").unwrap(), "應該可以重新登記");
    }

    /// 設錯的信箱要能整組永久刪掉 —— 停用留得住位址，但設錯的信箱本身
    /// 不該留著，它只會在畫面上繼續看起來像個有效的轉發目標。
    #[test]
    fn a_wrong_mailbox_can_be_purged_without_touching_the_others() {
        let db = mem();
        add_recipient(&db, "disneyplus@share.example.com", "a@e.com", None, "admin").unwrap();
        add_recipient(&db, "disneyplus@share.example.com", "b@e.com", None, "admin").unwrap();
        add_recipient(&db, "disney@share.example.com", "a@e.com", None, "admin").unwrap();

        assert_eq!(delete_recipients_for_mailbox(&db, "disneyplus@share.example.com").unwrap(), 2);

        assert!(enabled_recipients_for(&db, "disneyplus@share.example.com").unwrap().is_empty());
        assert_eq!(
            enabled_recipients_for(&db, "disney@share.example.com").unwrap(),
            vec!["a@e.com"],
            "對的那個信箱不該被波及"
        );
    }

    /// 平台 → 信箱的對應要存得住、讀得回來。
    #[test]
    fn the_platform_mailbox_map_round_trips() {
        let db = mem();
        assert!(get_setting_str_map(&db, keys::PLATFORM_MAILBOXES).is_empty());

        let m = BTreeMap::from([
            ("disneyplus".to_string(), "disney@share.example.com".to_string()),
            ("netflix".to_string(), "netflix@share.example.com".to_string()),
        ]);
        set_setting_str_map(&db, keys::PLATFORM_MAILBOXES, &m, Some("admin")).unwrap();
        assert_eq!(get_setting_str_map(&db, keys::PLATFORM_MAILBOXES), m);
    }

    /// 移除一個帳號到底帶走了什麼 —— 逐張表確認，不靠註解。
    ///
    /// 這張清單是「移除成員」這個動作的完整定義。少刪任何一項，
    /// 那個人就還留著某種形式的存取或收件能力。
    #[test]
    fn removing_a_member_clears_every_trace_except_the_audit_log() {
        let db = mem();
        create_user_with_platforms(
            &db, "u1", "mei", "mei", "member", Some("mei@x.tw"),
            &["netflix".into(), "disneyplus".into()],
        )
        .unwrap();

        // 他名下的每一種資料各放一筆
        add_credential(&db, "cred1", "u1", "passkey-json", Some("iPhone")).unwrap();
        add_push_sub(&db, "u1", "https://push.example/aaa", "pub", "auth", None).unwrap();
        upsert_allow(&db, "1.2.3.4", None, Some("mei@x.tw"), now() + 86400, 7).unwrap();
        add_recipient(&db, "netflix@share.example.com", "mei@x.tw", None, "admin").unwrap();
        invite_email(&db, "mei@x.tw", "admin", &[]).unwrap();
        consume_invited_email(&db, "mei@x.tw", "u1").unwrap();
        audit(&db, Some("mei@x.tw"), "allow_add", Some("1.2.3.4"), None);

        // 另一個人的同類資料，用來確認刪除有界線
        create_user_with_platforms(&db, "u2", "ann@x.tw", "ann", "member", Some("ann@x.tw"), &[]).unwrap();
        add_credential(&db, "cred2", "u2", "passkey-json", None).unwrap();
        upsert_allow(&db, "5.6.7.8", None, Some("ann@x.tw"), now() + 86400, 7).unwrap();
        add_recipient(&db, "netflix@share.example.com", "ann@x.tw", None, "admin").unwrap();

        let removed = delete_user(&db, "u1", "mei@x.tw").unwrap();
        assert_eq!(removed.entries, 1);
        assert_eq!(removed.recipients, 1);

        // ── 必須消失的 ──
        assert!(get_user(&db, "u1").unwrap().is_none(), "users");
        assert!(list_credentials(&db, "u1").unwrap().is_empty(), "credentials（Passkey）");
        assert!(platforms_for(&db, "u1").unwrap().is_empty(), "user_platforms");
        assert!(list_push_subs(&db, "u1").unwrap().is_empty(), "push_subscriptions");
        assert!(
            !list_allow(&db).unwrap().iter().any(|e| e.ip == "1.2.3.4"),
            "allowlist"
        );
        assert!(
            recipients_for_address(&db, "mei@x.tw").unwrap().is_empty(),
            "mail_recipients"
        );
        // invited_emails 那列刪掉後，位址回到可以重新登記的狀態
        invite_email(&db, "mei@x.tw", "admin", &[]).unwrap();
        assert!(
            consume_invited_email(&db, "mei@x.tw", "u3").unwrap().is_some(),
            "位址必須能重新登記並註冊 —— 不刪的話會永遠卡在「已使用」"
        );

        // ── 必須留著的 ──
        assert_eq!(recent_audit(&db, 10).unwrap().len(), 1, "稽核是歷史，不該跟著人走");

        // ── 不該波及別人的 ──
        assert!(get_user(&db, "u2").unwrap().is_some());
        assert_eq!(list_credentials(&db, "u2").unwrap().len(), 1);
        assert!(list_allow(&db).unwrap().iter().any(|e| e.ip == "5.6.7.8"));
        assert_eq!(
            enabled_recipients_for(&db, "netflix@share.example.com").unwrap(),
            vec!["ann@x.tw"]
        );
    }

    /// ⚠️ 移除成員必須連轉發一起收掉。白名單有 TTL 會自己過期，
    /// 轉發不會 —— 留著等於那個人**永遠繼續收到驗證碼**，
    /// 而面板上完全看不出來他已經被移除了。
    #[test]
    fn removing_a_member_stops_forwarding_to_them() {
        let db = mem();
        create_user_with_platforms(&db, "u1", "mei@x.tw", "mei", "member", Some("mei@x.tw"), &[]).unwrap();
        add_recipient(&db, "netflix@share.example.com", "mei@x.tw", None, "admin").unwrap();
        add_recipient(&db, "disneyplus@share.example.com", "mei@x.tw", None, "admin").unwrap();
        add_recipient(&db, "netflix@share.example.com", "ann@x.tw", None, "admin").unwrap();

        let removed = delete_user(&db, "u1", "mei@x.tw").unwrap();
        assert_eq!(removed.recipients, 2, "名下兩個 mailbox 都要收掉");

        // 最要緊的是 Worker 拿到的那份名單真的少了他
        assert_eq!(
            enabled_recipients_for(&db, "netflix@share.example.com").unwrap(),
            vec!["ann@x.tw"],
            "被移除的人不該還在轉發名單上"
        );
        assert!(enabled_recipients_for(&db, "disneyplus@share.example.com").unwrap().is_empty());
    }

    /// 沒有 email 的舊帳號（v6 之前註冊的）也要刪得掉，不能因為
    /// 撈不到位址就整個炸掉。
    #[test]
    fn removing_a_member_without_an_email_still_works() {
        let db = mem();
        create_user_with_platforms(&db, "u1", "legacy", "legacy", "member", None, &[]).unwrap();
        let removed = delete_user(&db, "u1", "legacy").unwrap();
        assert_eq!(removed.recipients, 0);
        assert!(get_user(&db, "u1").unwrap().is_none());
    }

    /// 推送訂閱靠外鍵 CASCADE 帶走 —— 不然還會繼續推驗證碼到他手機上。
    #[test]
    fn removing_a_member_stops_pushing_to_them() {
        let db = mem();
        user_with_push(&db, "u1", "https://push.example/aaa");
        grant_platform(&db, "u1", "netflix", "admin").unwrap();
        assert_eq!(push_subs_for_platform(&db, "netflix").unwrap().len(), 1);

        delete_user(&db, "u1", "u1@x.tw").unwrap();
        assert!(push_subs_for_platform(&db, "netflix").unwrap().is_empty());
    }

    /// 稽核是歷史 —— 人走了不代表做過的事沒發生過。
    #[test]
    fn removing_a_user_keeps_the_audit_trail() {
        let db = mem();
        create_user_with_platforms(&db, "u1", "mei@x.tw", "mei", "member", Some("mei@x.tw"), &[]).unwrap();
        audit(&db, Some("mei@x.tw"), "allow_add", Some("1.1.1.1"), None);
        delete_user(&db, "u1", "mei@x.tw").unwrap();
        assert_eq!(recent_audit(&db, 10).unwrap().len(), 1);
    }

    // ── 推送通知 ──────────────────────────────────────

    fn user_with_push(db: &Db, id: &str, endpoint: &str) {
        create_user_with_platforms(
            db, id, &format!("{id}@x.tw"), id, "member", Some(&format!("{id}@x.tw")), &[],
        )
        .unwrap();
        add_push_sub(db, id, endpoint, "pub", "auth", Some("iPhone")).unwrap();
    }

    /// 同一台裝置重新訂閱不該累積第二筆 —— endpoint 是去重鍵。
    #[test]
    fn resubscribing_the_same_device_replaces_the_old_row() {
        let db = mem();
        user_with_push(&db, "u1", "https://push.example/aaa");
        add_push_sub(&db, "u1", "https://push.example/aaa", "pub2", "auth2", Some("iPad")).unwrap();

        let subs = list_push_subs(&db, "u1").unwrap();
        assert_eq!(subs.len(), 1, "同一個 endpoint 只該有一筆");
        assert_eq!(subs[0].p256dh, "pub2", "金鑰材料要更新到最新的");
        assert_eq!(subs[0].label.as_deref(), Some("iPad"));
    }

    /// 兩個人在同一秒做同一件事會留下兩列一模一樣的稽核 —— 只有 id 分得出來，
    /// 而面板的清單就是拿它當 key（拿欄位拼會撞，整塊畫不出來）。
    #[test]
    fn simultaneous_identical_events_are_still_distinguishable() {
        let db = mem();
        audit(&db, Some("a@x.tw"), "login", Some("以 passkey 登入"), None);
        audit(&db, Some("b@x.tw"), "login", Some("以 passkey 登入"), None);

        let rows = recent_audit(&db, 10).unwrap();
        assert_eq!(rows.len(), 2);
        assert_ne!(rows[0].id, rows[1].id);
        assert!(rows[0].id > rows[1].id, "同秒的兩列要照 id 由新到舊，順序不能隨機");
    }

    /// 別台裝置撤掉之後，這台要問得出來「我已經不在名單裡了」。
    #[test]
    fn a_revoked_device_can_tell_that_it_is_gone() {
        let db = mem();
        user_with_push(&db, "u1", "https://push.example/aaa");
        user_with_push(&db, "u2", "https://push.example/bbb");
        assert!(push_sub_exists(&db, "https://push.example/aaa", "u1").unwrap());
        // 別人的 endpoint 不算數，猜到了也問不出東西
        assert!(!push_sub_exists(&db, "https://push.example/bbb", "u1").unwrap());

        let id = list_push_subs(&db, "u1").unwrap()[0].id;
        delete_push_sub(&db, id, "u1").unwrap();
        assert!(!push_sub_exists(&db, "https://push.example/aaa", "u1").unwrap());
    }

    /// 裝置回來了就該從乾淨的狀態開始，不帶著上次的失敗計數。
    #[test]
    fn resubscribing_clears_the_failure_count() {
        let db = mem();
        user_with_push(&db, "u1", "https://push.example/aaa");
        let id = list_push_subs(&db, "u1").unwrap()[0].id;
        bump_push_fail(&db, id).unwrap();
        bump_push_fail(&db, id).unwrap();
        assert_eq!(list_push_subs(&db, "u1").unwrap()[0].fail_count, 2);

        add_push_sub(&db, "u1", "https://push.example/aaa", "pub", "auth", None).unwrap();
        assert_eq!(list_push_subs(&db, "u1").unwrap()[0].fail_count, 0);
    }

    /// 推送對象與驗證碼分頁同一條規則：沒有這個平台就收不到通知。
    #[test]
    fn push_targets_follow_platform_grants() {
        let db = mem();
        user_with_push(&db, "u1", "https://push.example/aaa");
        user_with_push(&db, "u2", "https://push.example/bbb");
        grant_platform(&db, "u1", "netflix", "admin").unwrap();
        grant_platform(&db, "u2", "disneyplus", "admin").unwrap();

        let targets = push_subs_for_platform(&db, "netflix").unwrap();
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].user_id, "u1");
    }

    /// admin 沒有特例 —— 沒被授權那個平台就不該收到那個平台的碼。
    #[test]
    fn admins_are_not_exempt_from_platform_filtering() {
        let db = mem();
        create_user_with_platforms(&db, "a1", "boss@x.tw", "boss", "admin", Some("boss@x.tw"), &[]).unwrap();
        add_push_sub(&db, "a1", "https://push.example/aaa", "pub", "auth", None).unwrap();

        assert!(push_subs_for_platform(&db, "netflix").unwrap().is_empty());
    }

    /// 關掉「新驗證碼」就不該再出現在名單裡。
    #[test]
    fn turning_codes_off_removes_you_from_the_targets() {
        let db = mem();
        user_with_push(&db, "u1", "https://push.example/aaa");
        grant_platform(&db, "u1", "netflix", "admin").unwrap();
        assert_eq!(push_subs_for_platform(&db, "netflix").unwrap().len(), 1);

        set_notify_prefs(&db, "u1", false, true).unwrap();
        assert!(push_subs_for_platform(&db, "netflix").unwrap().is_empty());
    }

    /// 設計 3a：兩顆推桿預設只開「新驗證碼」。
    #[test]
    fn new_users_get_codes_on_and_expiry_off() {
        let db = mem();
        create_user_with_platforms(&db, "u1", "mei@x.tw", "mei", "member", Some("mei@x.tw"), &[]).unwrap();
        assert_eq!(notify_prefs(&db, "u1").unwrap(), (true, false));
    }

    /// 「授權快到期」預設關著，所以要先開才收得到。
    #[test]
    fn expiry_targets_require_the_second_toggle() {
        let db = mem();
        user_with_push(&db, "u1", "https://push.example/aaa");
        assert!(push_subs_for_expiry(&db, "u1").unwrap().is_empty());

        set_notify_prefs(&db, "u1", true, true).unwrap();
        assert_eq!(push_subs_for_expiry(&db, "u1").unwrap().len(), 1);
    }

    /// 續期檢查 10 分鐘一輪、提醒視窗 24 小時 —— 不去重會提醒 144 次。
    #[test]
    fn an_expiry_notice_is_claimed_only_once() {
        let db = mem();
        upsert_allow(&db, "1.2.3.4", Some("家裡"), Some("mei"), now() + 3600, 7).unwrap();

        assert!(claim_expiry_notice(&db, "1.2.3.4").unwrap());
        assert!(!claim_expiry_notice(&db, "1.2.3.4").unwrap(), "同一輪不該提醒第二次");
    }

    /// 續期之後標記要清回去，下一輪真的快到期時才提醒得了。
    ///
    /// ⚠️ 手動重新授權跟自動續期都算 —— **手動那條才是常見路徑**：
    /// 收到通知的人就是來按那顆的。漏掉它的話那條白名單只會被提醒一次，
    /// 之後每次到期都靜悄悄，而且從畫面上完全看不出來。
    #[test]
    fn renewing_lets_the_next_expiry_be_announced_again() {
        for renew in [
            // 自動續期
            (|db: &Db| { renew_allow(db, "1.2.3.4").unwrap(); }) as fn(&Db),
            // 使用者自己再按一次「授權這個網路」
            |db: &Db| {
                upsert_allow(db, "1.2.3.4", None, Some("mei"), now() + 7 * 86400, 7).unwrap()
            },
        ] {
            let db = mem();
            upsert_allow(&db, "1.2.3.4", Some("家裡"), Some("mei"), now() + 3600, 7).unwrap();
            assert!(claim_expiry_notice(&db, "1.2.3.4").unwrap());

            renew(&db);
            assert!(claim_expiry_notice(&db, "1.2.3.4").unwrap(), "續期後該能再提醒一次");
        }
    }

    /// 一律只能撤銷自己的 —— 跟 passkey 同一個原則。
    #[test]
    fn you_cannot_revoke_someone_elses_subscription() {
        let db = mem();
        user_with_push(&db, "u1", "https://push.example/aaa");
        user_with_push(&db, "u2", "https://push.example/bbb");
        let victim = list_push_subs(&db, "u2").unwrap()[0].id;

        assert_eq!(delete_push_sub(&db, victim, "u1").unwrap(), 0, "不該動得到別人的");
        assert_eq!(list_push_subs(&db, "u2").unwrap().len(), 1);
        assert_eq!(delete_push_sub(&db, victim, "u2").unwrap(), 1);
    }

    /// 推送服務回 404／410 時靠 endpoint 清除，那時手上沒有 id。
    #[test]
    fn dead_endpoints_are_removable_without_an_id() {
        let db = mem();
        user_with_push(&db, "u1", "https://push.example/aaa");
        assert_eq!(delete_push_sub_by_endpoint(&db, " https://push.example/aaa ").unwrap(), 1);
        assert!(list_push_subs(&db, "u1").unwrap().is_empty());
    }

    /// 移除成員時訂閱要跟著走（外鍵 CASCADE），否則推送會找不到人。
    #[test]
    fn removing_a_user_takes_their_subscriptions() {
        let db = mem();
        user_with_push(&db, "u1", "https://push.example/aaa");
        delete_user(&db, "u1", "u1@x.tw").unwrap();
        assert!(list_push_subs(&db, "u1").unwrap().is_empty());
    }

    /// 註冊完成的那一刻，登記時選好的平台就要生效 —— 不然家人看到的是
    /// 空的驗證碼分頁，還得回頭問「怎麼什麼都沒有」。
    #[test]
    fn a_new_member_gets_the_platforms_chosen_at_invite_time() {
        let db = mem();
        create_user_with_platforms(
            &db, "u1", "mei@x.tw", "mei", "member", Some("mei@x.tw"),
            &["netflix".into(), "disneyplus".into()],
        )
        .unwrap();

        assert_eq!(platforms_for(&db, "u1").unwrap(), vec!["disneyplus", "netflix"]);
        assert!(get_user(&db, "u1").unwrap().is_some());
    }

    /// 沒選平台就是沒選，不該憑空多出來。
    #[test]
    fn no_platforms_chosen_means_none_granted() {
        let db = mem();
        create_user_with_platforms(&db, "u1", "mei@x.tw", "mei", "member", None, &[]).unwrap();
        assert!(platforms_for(&db, "u1").unwrap().is_empty());
    }

    /// ⚠️ 授權**必須在帳號建立之後**才寫得進去：`user_platforms.user_id`
    /// 有外鍵指向 `users`，順序反了會被 FK 擋下。
    ///
    /// 這正是 register_finish 曾經踩到的坑 —— 授權迴圈排在 `create_user`
    /// 之前，而且用 `let _ =` 吞掉了錯誤，結果是登記時選好的平台
    /// **靜默地一個都沒授權**，家人註冊完看到空的驗證碼分頁。
    #[test]
    fn granting_before_the_user_exists_is_rejected() {
        let db = mem();
        assert!(
            grant_platform(&db, "ghost", "netflix", "admin").is_err(),
            "帳號還不存在時不該寫得進去 —— 靜默失敗比報錯更糟"
        );

        create_user_with_platforms(&db, "u1", "mei@x.tw", "mei", "member", Some("mei@x.tw"), &[]).unwrap();
        grant_platform(&db, "u1", "netflix", "admin").unwrap();
        assert_eq!(platforms_for(&db, "u1").unwrap(), vec!["netflix"]);
    }

    /// 撤銷之後就不該再出現在清單上 —— 那筆已經沒有任何功能，
    /// UI 上也給不出任何動作，留著只會無限累積死資料。
    #[test]
    fn revoked_invites_leave_the_list() {
        let db = mem();
        invite_email(&db, "mei@x.tw", "admin", &["netflix".into()]).unwrap();
        invite_email(&db, "ann@x.tw", "admin", &[]).unwrap();
        assert_eq!(list_invited_emails(&db).unwrap().len(), 2);

        revoke_invited_email(&db, "mei@x.tw").unwrap();
        let left = list_invited_emails(&db).unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].email, "ann@x.tw");
    }

    /// 註冊完就不該再出現 —— 那個人已經在成員清單裡了，
    /// 這份清單回答的是「還有誰沒進來」。
    #[test]
    fn registered_invites_leave_the_list_too() {
        let db = mem();
        invite_email(&db, "mei@x.tw", "admin", &["netflix".into()]).unwrap();
        invite_email(&db, "ann@x.tw", "admin", &[]).unwrap();

        create_user_with_platforms(&db, "u1", "mei@x.tw", "mei", "member", Some("mei@x.tw"), &[]).unwrap();
        consume_invited_email(&db, "mei@x.tw", "u1").unwrap();

        let left = list_invited_emails(&db).unwrap();
        assert_eq!(left.len(), 1, "已註冊的不該還留在待處理清單上");
        assert_eq!(left[0].email, "ann@x.tw");
    }

    /// 但那一列必須留在 DB 裡 —— 它是「這個位址已經換過帳號」的閘門。
    #[test]
    fn a_used_invite_still_blocks_a_second_registration() {
        let db = mem();
        invite_email(&db, "mei@x.tw", "admin", &[]).unwrap();
        create_user_with_platforms(&db, "u1", "mei@x.tw", "mei", "member", Some("mei@x.tw"), &[]).unwrap();
        consume_invited_email(&db, "mei@x.tw", "u1").unwrap();

        assert!(list_invited_emails(&db).unwrap().is_empty());
        assert!(
            consume_invited_email(&db, "mei@x.tw", "u2").unwrap().is_none(),
            "從清單上消失不等於可以再註冊一次"
        );
    }

    /// 但重新登記要能讓它原地復活 —— 撤銷不是永久黑名單。
    #[test]
    fn re_registering_brings_a_revoked_invite_back() {
        let db = mem();
        invite_email(&db, "mei@x.tw", "admin", &[]).unwrap();
        revoke_invited_email(&db, "mei@x.tw").unwrap();
        assert!(list_invited_emails(&db).unwrap().is_empty());

        invite_email(&db, "mei@x.tw", "admin", &["netflix".into()]).unwrap();
        let back = list_invited_emails(&db).unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].platforms, vec!["netflix"]);
    }

    /// Cloudflare 沒回傳的位址代表它根本沒有那個目的地 —— 轉發一定失敗。
    /// 舊的逐筆 UPDATE 會讓那筆停在「未查詢」，跟「面板沒設 token」
    /// 長得一模一樣，是最危險的一種混淆。
    #[test]
    fn addresses_missing_from_cloudflare_are_marked_absent() {
        let db = mem();
        add_recipient(&db, "netflix@x.tw", "known@e.com", None, "admin").unwrap();
        add_recipient(&db, "netflix@x.tw", "ghost@e.com", None, "admin").unwrap();

        sync_cf_status(&db, &[("known@e.com".into(), Some(1_700_000_000))]).unwrap();

        let rows = list_recipients(&db).unwrap();
        let known = rows.iter().find(|r| r.address == "known@e.com").unwrap();
        let ghost = rows.iter().find(|r| r.address == "ghost@e.com").unwrap();

        assert_eq!(known.cf_present, Some(true));
        assert_eq!(known.cf_verified_at, Some(1_700_000_000));

        assert_eq!(ghost.cf_present, Some(false), "Cloudflare 沒有它，必須看得出來");
        assert!(ghost.cf_checked_at.is_some(), "查過了就不能還顯示成「未查詢」");
        assert_eq!(ghost.cf_verified_at, None);
    }

    /// 位址在 Cloudflare 但還沒驗證 —— 跟「根本不存在」是不同的狀態，
    /// 給使用者的說法也不一樣（一個是他沒點信，一個是信從來沒寄出去）。
    #[test]
    fn present_but_unverified_is_its_own_state() {
        let db = mem();
        add_recipient(&db, "netflix@x.tw", "waiting@e.com", None, "admin").unwrap();
        sync_cf_status(&db, &[("waiting@e.com".into(), None)]).unwrap();

        let r = &list_recipients(&db).unwrap()[0];
        assert_eq!(r.cf_present, Some(true));
        assert_eq!(r.cf_verified_at, None);
    }

    /// 驗證狀態一撤就要跟著撤 —— 位址從 Cloudflare 刪掉之後，
    /// 面板不能還顯示著兩年前那個「已驗證」。
    #[test]
    fn a_removed_destination_loses_its_verified_badge() {
        let db = mem();
        add_recipient(&db, "netflix@x.tw", "gone@e.com", None, "admin").unwrap();
        sync_cf_status(&db, &[("gone@e.com".into(), Some(1_700_000_000))]).unwrap();
        assert!(list_recipients(&db).unwrap()[0].cf_verified_at.is_some());

        sync_cf_status(&db, &[]).unwrap();
        let r = &list_recipients(&db).unwrap()[0];
        assert_eq!(r.cf_verified_at, None);
        assert_eq!(r.cf_present, Some(false));
    }

    /// 遷移可在既有資料上重複執行。
    #[test]
    fn migration_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        migrate(&conn).unwrap();
        let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, 11);
    }

    /// ⚠️ 已經跑過 v10 的資料庫必須也拿得到後來補的那幾個欄位。
    ///
    /// 這是真的發生過的事故：`cf_present` 被後補進 `migrate_v10`，而線上的
    /// `user_version` 已經是 10，那個區塊不再執行 —— 面板一開「轉發收件人」
    /// 就 `no such column: cf_present`。教訓是**已套用的 migration 不能改**，
    /// 要補就開新版本。
    #[test]
    fn a_database_already_at_v10_still_gets_the_late_columns() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        migrate(&conn).unwrap();

        // 模擬「舊的 v10」：把後補的欄位拿掉，版本留在 10
        for (table, col) in [
            ("mail_recipients", "cf_present"),
            ("allowlist", "expiry_notified_at"),
        ] {
            conn.execute_batch(&format!("ALTER TABLE {table} DROP COLUMN {col};"))
                .unwrap();
        }
        conn.pragma_update(None, "user_version", 10).unwrap();

        migrate(&conn).unwrap();

        for (table, col) in [
            ("mail_recipients", "cf_present"),
            ("allowlist", "expiry_notified_at"),
        ] {
            let has: bool = conn
                .prepare(&format!("SELECT 1 FROM pragma_table_info('{table}') WHERE name = ?1"))
                .unwrap()
                .exists(params![col])
                .unwrap();
            assert!(has, "{table}.{col} 沒有被補上");
        }
    }

    /// verified 是 v5 新增欄位，舊信件為 NULL，不可讀成 false。
    #[test]
    fn old_mails_have_unknown_verification() {
        let db = mem();
        {
            let conn = db.lock().unwrap();
            conn.execute(
                "INSERT INTO mails (message_id, received_at, subject) VALUES ('old', 1, '舊信')",
                [],
            )
            .unwrap();
        }
        insert_mail(
            &db, Some("new"), 2, None, None, Some("新信"), None, None, None, &[], false,
            None, None,
        )
        .unwrap();

        let mails = recent_mails(&db, 10).unwrap();
        let old = mails.iter().find(|m| m.subject.as_deref() == Some("舊信")).unwrap();
        let new = mails.iter().find(|m| m.subject.as_deref() == Some("新信")).unwrap();
        assert_eq!(old.verified, None, "舊資料應為未知，而不是未通過");
        assert_eq!(new.verified, Some(false));
    }
}
