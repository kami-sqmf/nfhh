# 安全性修正實作計畫（Codex Security 掃描 f3c6583）

> **對於代理工作者：** 必要子技能：使用 superpowers:subagent-driven-development（推薦）或 superpowers:executing-plans 逐任務實作此計畫。步驟使用核取方塊（`- [ ]`）語法進行追蹤。

**目標：** 修掉 Codex Security 對修訂 `f3c6583` 掃出的 17 項發現（1 high、8 medium、8 low），每一項都有對應的回歸測試。其中 #4（偽造 `Authentication-Results`）的程式修正只能**降低**風險，要靠部署後的 canary 才能宣告關閉（見任務 7）。

**架構：** 後端修正集中在 `app/control/src`（Rust／Axum／SQLite），以「分離狀態機、把授權放進 SQL、為所有外部輸入設上限」三個原則處理；Email Worker 改成具型別的拒收／不可用分類；部署層用 systemd drop-in 讓 Docker 依賴防火牆；開發截圖流程移除 host network 與 root。文件最後統一更新。

**技術棧：** Rust edition 2024、axum 0.8、tower-sessions 0.15、rusqlite（bundled，含 JSON1）、webauthn-rs 0.5、Svelte 5、Bun、Cloudflare Email Workers、systemd、nftables。

---

## 背景與範圍

掃描報告在 `f3c65834e940f8ffda66325116327aa80b710561_20260901T174729Z_w17dljca/`（未進版控）。基準：`cargo test` 於 `app/control` 目前 181 個測試全部通過。工作分支 `security-fix` 已從 `master` 建出並推上 origin。

| # | 嚴重度 | 發現 | 任務 |
|---|---|---|---|
| 1 | high | 登入流程覆寫註冊狀態，member 可把 Passkey 掛到 admin | 任務 1 |
| 2 | medium | Email 驗證結果未綁定 session，可被搶先建立 Passkey | 任務 2 |
| 16 | low | 舊帳號可未驗證即搶占他人 Email | 任務 3（整段移除 v6 遷移路徑） |
| 10 | low | member 可改寫他人白名單的 TTL 與標籤 | 任務 4 |
| 11 | low | member 可枚舉硬刪任何郵件 | 任務 5 |
| 13 | low | Unicode HTML 讓解析 panic | 任務 6 |
| 4 | medium | 偽造 `Authentication-Results` 取得已認證 | 任務 7（程式修正 + 部署後 canary；殘餘風險見該任務） |
| 12 | low | 面板改的可信網域不生效 | 任務 8 |
| 5 | medium | Worker 把所有失敗折疊成 fail-open 轉發 | 任務 9 |
| 7 | medium | 偽造未來 `Date` 讓郵件永不清除並擠出清單 | 任務 10 |
| 8 | medium | 無界輸入撐大無保留期的稽核表 | 任務 11 |
| 9 | medium | 推播訂閱無配額、扇出無上限 | 任務 12 |
| 6 | medium | 郵件清單每 20 秒重傳全文 | 任務 13 |
| 17 | low | 未驗證郵件的任意連結被包成品牌按鈕 | 任務 14 |
| 3 | medium | 防火牆載入失敗仍啟動 Docker 資料平面 | 任務 15 |
| 14、15 | low | 開發截圖容器：浮動 root 映像、CDP 暴露全介面 | 任務 16 |
| — | — | 文件：DECISIONS、CONTROL、SETUP、README、.env.example | 任務 17 |

## 修訂紀錄（v2，依外部審查）

審查指出的問題逐項對照程式碼與環境驗證後的處置：

| 審查項目 | 處置 |
|---|---|
| `./nfhh apply` 不會重建 control | 屬實（`nfhh:128` 只 `exec scripts/apply-config.sh`）。新增「部署與驗收」一節，改用 `docker compose up -d --build control` 加 smoke |
| 任務 4 仍允許 member 延長他人條目、owner 查詢與更新有競態 | 採納。改成只有擁有者或 admin 能改寫，判斷放進同一句 `ON CONFLICT … DO UPDATE … WHERE` |
| 任務 10 的 migration 把偽造的 `received_at` 原樣搬進 `ingested_at` | 採納。回填改用 `min(received_at, unixepoch())`，加 v11→v12 惡意資料 fixture |
| 任務 12 用他人 endpoint 繞配額、task 數無上限、金鑰未驗曲線 | 採納。接手他人 endpoint 一律計入配額；扇出改為「滿 8 個就等一個做完」；`p256::PublicKey::from_sec1_bytes` 驗點、先擋字串長度 |
| 任務 14 手寫 URL parser 被 `https://evil.example\@netflix.com/x` 繞過 | 屬實（已用 `url 2.5.8` 實測：host 是 `evil.example`）。改用 `url::Url`，拒絕帶帳密的 URL |
| 任務 14 應另建品牌連結網域清單 | **不採納**。domain-set 清單本來就是「平台自己的控制平面網域」（見 `netflix.list` 檔頭），另建清單只會漂移；改在 DECISIONS 寫明清單只能放平台持有的網域 |
| 任務 7 只是部署假設，authserv-id 不是秘密 | 屬實。Cloudflare 沒有文件保證表頭順序，而且有回報指出 Worker 收到的信可能根本沒有 `Authentication-Results`（[workerd#6740](https://github.com/cloudflare/workerd/issues/6740)）。程式修正保留（嚴格優於現狀），另加部署後 canary 與殘餘風險紀錄；Worker 端無法正規化 —— 它拿不到任何獨立的驗證結果 |
| 任務 5／13 list、get、delete 授權規則不一致 | 採納。三處共用 `MailScope` 判斷式；刪除改為「讀出、判斷、刪除」在同一把鎖內 |
| 任務 13 應在 ingest 時持久化 `actionable` | **不採納**。關鍵字設定改了要立刻生效（現有行為），持久化就得在每次改設定時重算；清單查詢讀 body 的成本有界（60 列 × 20k 字元、本機 SQLite），真正的放大在網路端，已由摘要 DTO 解掉 |
| 任務 15 `start` 對 `RemainAfterExit=yes` 的 unit 無效、驗證會停掉正式資料平面 | 屬實。改用 `restart`，驗證限維護時段、要求 console 進入方式、加 `trap` 復原 |
| 任務 16 `Config.User` 空白不代表非 root、`--rm` 不保證清除 | 屬實（本機映像 `Config.User` 是 `chrome`）。改以容器內 `id -u` 驗證，改成有 `trap` 的 `dev/shoot.sh`，digest 直接釘本機已驗證的值 |
| 任務 1 缺完整重播攻擊鏈的測試、清理錯誤被吞、成功後沒清 `S_REG_USER` | 採納。加 `webauthn-authenticator-rs 0.5.5`（softpasskey）重播整條鏈；`clear_auth_flows` 回 `Result`；finish 成功後清兩把鍵 |
| 任務 3 的 `email IS NULL = 0` 要變成部署 hard gate、要備份與復原演練 | 採納，見「部署與驗收」 |
| 任務 9 未設定 `PANEL_ENDPOINT`／`PANEL_SECRET` 不該 fail-open | **不採納**。那是部署狀態不是攻擊面（攻擊者改不了 Worker 的環境變數），而且「Worker 先於面板部署」時它是唯一的轉發路徑（檔頭註解的既有設計）。改成獨立的 `unconfigured` 分類並大聲記錄；另補上「後端先、Worker 後」的部署順序 |
| 新環境變數沒進 `docker-compose.yml` | 屬實。任務 7、11 各自補上 |
| `cargo test -- a b` 無效 | 屬實。全部改成 `cargo test -- a b`（`--` 之後 libtest 接受多個 filter，已實測） |
| 驗收要有 fmt、clippy、`compose config`、正式 build、fixture、canary、smoke | 採納，見「最終驗收清單」 |
| 不可逆清理要有預覽、備份與 rollback runbook | 採納，見「部署與驗收」 |

## 共通約定

- 所有指令在 repo 根目錄 `/home/kamisqmf/nfhh` 執行；Rust 測試一律 `cd app/control && cargo test`。
- 測試名稱沿用專案風格：以句子描述行為（`a_member_cannot_…`），註解寫「為什麼」。
- 每個任務結尾 commit 一次。訊息用繁中，格式「主題：一句話說明」，跟現有 git log 一致。
- 中英文與數字之間補空格、中文語境用全形標點（專案排版規範）。
- 不改已經套用過的 migration（`db.rs` 檔頭的警告）。新欄位一律走新的 `migrate_v12`。

### 測試輔助（任務 1 建立，之後各任務共用）

`app/control/src/main.rs` 的 `mod tests` 內：

```rust
/// 不經 HTTP 層直接餵給 handler 的 session。每個測試自己開一個，
/// 跨 session 的攻擊情境就用兩個。
fn test_session() -> Session {
    Session::new(None, Arc::new(MemoryStore::default()), None)
}
```

---

## 階段一：身分與授權

### 任務 1：分離登入與註冊的 session 狀態（發現 #1，high）

**檔案：**
- 修改：`app/control/Cargo.toml`（dev-dependency）
- 修改：`app/control/src/main.rs:169-173`（session 鍵）、`:390-484`（`register_start`）、`:488-566`（`register_finish`）、`:600-606`（`login_any_start`）、`:637-700`（`login_start`／`login_finish`）
- 測試：`app/control/src/main.rs` 的 `mod tests`

- [ ] **步驟 1：加入軟體認證器作為測試相依**

`Cargo.toml` 的 `[dev-dependencies]` 加：

```toml
# 測試用的軟體 Passkey：能真的產生 WebAuthn 註冊／登入回應，才能整條重播攻擊鏈
webauthn-authenticator-rs = { version = "0.5.5", features = ["softpasskey"] }
```

執行：`cd app/control && cargo fetch`
預期：成功（crates.io 上有 0.5.5，與 `webauthn-rs 0.5.5` 同版；API 為 `WebauthnAuthenticator::new(SoftPasskey::new(true))`、`do_registration(origin, ccr)`、`do_authentication(origin, rcr)`）。

- [ ] **步驟 2：撰寫失敗的測試**

在 `mod tests` 加入 `test_session()`（見共通約定）與：

```rust
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
    login_start(State(st.clone()), session.clone(), Json(EmailReq { email: "admin@x".into() }))
        .await
        .map_err(|e| e.0)
        .unwrap();
    // 3. 提交第 1 步的註冊回應
    let cred = attacker_key.do_registration(origin, ccr).unwrap();
    let res = register_finish(State(st.clone()), session.clone(), hdrs(&[]), Json(cred)).await;

    assert!(res.is_err(), "跨流程的 finish 必須失敗");
    assert_eq!(db::credentials_for(&st.db, "admin").unwrap().len(), 1, "admin 不得多出憑證");
    assert!(db::credentials_for(&st.db, "mem").unwrap().is_empty(), "也不該偷偷寫給 member 自己");
}
```

- [ ] **步驟 3：執行測試以確認它們失敗**

執行：`cd app/control && cargo test -- registration_may_only_land starting_a_login_wipes cannot_attach_a_passkey`
預期：前兩個編譯錯誤 `cannot find function check_registration_owner`；把那兩個暫時註解掉再跑，第三個 FAIL 於 `跨流程的 finish 必須失敗`（現況下攻擊成功，admin 多出一把憑證）。

- [ ] **步驟 4：撰寫實作**

在 `main.rs:173`（`S_AUTH` 之後）新增鍵與輔助：

```rust
/// 信箱＋passkey 登入的目標。跟註冊的 `S_REG_USER` 分開 ——
/// 共用同一把鍵曾讓「啟動登入」覆寫「註冊目標」，member 的新 Passkey
/// 就被寫進 admin 的資料列。
const S_LOGIN_USER: &str = "login_user";

/// 任一認證流程開始時，先把其他流程留下的狀態全部清掉。
/// 登入與註冊各自是獨立的狀態機，殘留的鍵會讓 finish 讀到另一條流程的目標。
/// 清不掉就整個請求失敗 —— 帶著殘留狀態繼續，正是這個弱點的成因。
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
```

`register_start`：在 `let ip = client_ip(&headers);` 之後加 `clear_auth_flows(&session).await?;`（`current_user` 讀的是 `S_USER`，不受影響）。

`register_finish`：在讀出 `p` 之後、`finish_passkey_registration` 之前加：

```rust
    let logged_in = current_user(&session).await;
    check_registration_owner(logged_in.as_ref().map(|(id, _)| id.as_str()), &p)?;
```

並把成功路徑的 `let _ = session.remove::<PasskeyRegistration>(S_REG).await;` 改為：

```rust
    // 兩把鍵都清，而且清不掉要報錯：殘留的目標是下一次攻擊的材料
    session.remove::<PasskeyRegistration>(S_REG).await?;
    session.remove::<PendingReg>(S_REG_USER).await?;
```

`login_any_start`：函式第一行加 `clear_auth_flows(&session).await?;`。

`login_start`：函式第一行加 `clear_auth_flows(&session).await?;`；把 `session.insert(S_REG_USER, &PendingReg { … })` 改成 `session.insert(S_LOGIN_USER, …)`。

`login_finish`：`session.get(S_REG_USER)` 改成 `session.get(S_LOGIN_USER)`；`let _ = session.remove::<PasskeyAuthentication>(S_AUTH).await;` 改為兩行 `session.remove::<PasskeyAuthentication>(S_AUTH).await?;`、`session.remove::<PendingReg>(S_LOGIN_USER).await?;`。

- [ ] **步驟 5：執行測試以確認它們通過**

執行：`cd app/control && cargo test`
預期：全部通過（181 + 3）。

- [ ] **步驟 6：Commit**

```bash
git add app/control/Cargo.toml app/control/Cargo.lock app/control/src/main.rs
git commit -m "登入與註冊改用各自的 session 鍵，finish 前檢查註冊目標等於登入者；軟體 Passkey 重播攻擊鏈"
```

### 任務 2：把 Email 驗證結果綁定到完成驗證的 session（發現 #2，medium）

**檔案：**
- 修改：`app/control/src/main.rs:294-353`（`join_verify`、`join_invite`）、`:443-452`（`register_start` 的 OTP 路徑）、`:536-540`（`register_finish` 清除）
- 測試：`app/control/src/main.rs` 的 `mod tests`（含既有的 `invite_link_opens_the_same_gate_as_a_code`）

- [ ] **步驟 1：撰寫失敗的測試**

```rust
/// 驗證碼／邀請連結證明的是「這個瀏覽器的人擁有信箱」，證明不能被
/// 另一個瀏覽器拿去用 —— 否則攻擊者只要等真正持有人驗證完就能搶先建帳號。
#[tokio::test]
async fn email_proof_is_bound_to_the_session_that_earned_it() {
    let st = test_state();
    db::invite_email(&st.db, "mei@example.com", "admin", &[]).unwrap();
    let token = invite::generate();
    db::set_invite_token(&st.db, "mei@example.com", &invite::hash(&st.db, &token).unwrap()).unwrap();

    let victim = test_session();
    join_invite(State(st.clone()), victim.clone(), hdrs(&[]), Json(InviteTokenReq { token }))
        .await
        .map_err(|e| e.0)
        .unwrap();

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
    assert!(err.to_string().contains("完成信箱驗證"), "拿到的訊息是：{err}");
    start(victim).await.expect("驗證過的那個 session 要能拿到 challenge");
}
```

- [ ] **步驟 2：執行測試以確認它失敗**

執行：`cd app/control && cargo test -- email_proof_is_bound`
預期：編譯錯誤，`join_invite` 參數數量不符。

- [ ] **步驟 3：撰寫實作**

`main.rs:173` 附近新增鍵：

```rust
/// 這個 session 剛證明過擁有哪個信箱。`register_start` 除了查全域的
/// `email_otp.verified_at`，還要求證明是**同一個瀏覽器**做的。
const S_EMAIL_PROOF: &str = "email_proof";
```

`join_verify` 簽章改為 `(State(st), session: Session, headers: HeaderMap, Json(req))`；在 `OtpCheck::Ok` 分支的 `db::audit` 之前加 `session.insert(S_EMAIL_PROOF, &email).await?;`。

`join_invite` 同樣加 `session: Session`（排在 `State` 之後）；在 `db::mark_email_verified` 之後加 `session.insert(S_EMAIL_PROOF, &row.email).await?;`。

`register_start` 的 OTP 路徑，把 `if !db::otp_recently_verified(…)` 那段改成：

```rust
            let proven: Option<String> = session.get(S_EMAIL_PROOF).await?;
            if proven.as_deref() != Some(email.as_str())
                || !db::otp_recently_verified(&st.db, &email, otp::VERIFIED_WINDOW_SECS)?
            {
                return Err(AppError(anyhow::anyhow!(
                    "請先在這個瀏覽器完成信箱驗證，或重新寄送一組驗證碼"
                )));
            }
```

`register_finish` 在 `db::clear_otp` 那段之後加 `let _ = session.remove::<String>(S_EMAIL_PROOF).await;`。

既有測試 `invite_link_opens_the_same_gate_as_a_code`（約 `main.rs:3262`）的呼叫改成 `join_invite(State(st), test_session(), hdrs(&[]), Json(…))`。用 `grep -n 'join_verify(\|join_invite(' app/control/src/main.rs` 確認沒有其他呼叫點。

- [ ] **步驟 4：執行測試以確認它通過**

執行：`cd app/control && cargo test`
預期：全部通過。

- [ ] **步驟 5：Commit**

```bash
git add app/control/src/main.rs
git commit -m "信箱驗證證明綁定 session：register_start 只接受同一瀏覽器完成的驗證"
```

### 任務 3：移除 v6 遷移期的舊版身分路徑（發現 #16，low）

所有帳號都已是 v6 之後的新版 Passkey 帳號（`users.email` 皆有值），補填 Email
與以 `username` 登入的相容路徑不再有使用者。與其替它補驗證，直接拿掉：
沒有端點就沒有這個弱點。`users.username` 欄位**保留** —— 它是 WebAuthn 的
user handle，也是歷史稽核的 actor 值（見 `db.rs` migrate_v6 的註解）。

**檔案：**
- 修改：`app/control/src/main.rs:643-648`（`login_start` 的 username 退路）、`:760-761`／`:837-840`（`Status.needs_email`）、`:2193-2245`（刪除 `set_my_email`）、`:2863`（刪除路由）、`main()` 啟動檢查
- 修改：`app/control/src/db.rs:471-480`（刪除 `find_user`）、`:514-521`（刪除 `set_user_email`）、`:786-797`（刪除 `rename_owner`）、`:2355-2372`（刪除測試 `backfilling_email_moves_ownership`）、新增 `users_without_email`
- 刪除：`app/control/web/src/screens/NeedEmail.svelte`
- 修改：`app/control/web/src/App.svelte:7,42-44`、`lib/api.js:32`、`screens/Login.svelte:109-114`
- 修改：`docs/CONTROL.md:216`、`:528`
- 測試：`app/control/src/main.rs` 的 `mod tests`

- [ ] **步驟 1：撰寫失敗的測試**

```rust
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

/// 部署前的假設是「沒有任何帳號缺 email」。啟動時要把違反假設的帳號點名。
#[test]
fn users_without_email_are_counted() {
    let db = db::test_db();
    db::create_user_with_platforms(&db, "a", "a", "a", "member", Some("a@x"), &[]).unwrap();
    assert_eq!(db::users_without_email(&db).unwrap(), 0);
    db::create_user_with_platforms(&db, "b", "b", "b", "member", None, &[]).unwrap();
    assert_eq!(db::users_without_email(&db).unwrap(), 1);
}
```

- [ ] **步驟 2：執行測試以確認它們失敗**

執行：`cd app/control && cargo test -- no_longer_falls_back users_without_email`
預期：第一個 FAIL（訊息含「沒有已註冊的 passkey」而非「查無此帳號」）；第二個編譯錯誤。

- [ ] **步驟 3：撰寫實作（後端）**

`login_start`：把「先查 email；查不到再退回 username」那段改成

```rust
    let user = db::find_user_by_email(&st.db, &ident)?.context("查無此帳號")?;
```

刪除 `db::find_user`（`db.rs:471-480`）。

`Status`：刪除 `needs_email` 欄位、`status()` 內計算它的那段與 `Ok(Json(Status { … needs_email, … }))` 的那一行。

刪除整個 `// ── 補填 Email（v6 遷移用）` 區塊（`set_my_email` 與其註解），路由表刪除 `.route("/api/me/email", post(set_my_email))`。刪除 `db::set_user_email`、`db::rename_owner` 與測試 `backfilling_email_moves_ownership`。`db.rs:786` 附近說明「舊帳號補填 email 之後稱呼會變」的註解一併刪除。

`db.rs` 新增：

```rust
/// 遷移期已結束，不該再有沒有 email 的帳號。啟動時點名，方便發現漏網的。
pub fn users_without_email(db: &Db) -> Result<i64> {
    let conn = db.lock().unwrap();
    Ok(conn.query_row("SELECT count(*) FROM users WHERE email IS NULL", [], |r| r.get(0))?)
}
```

`main()` 在 `nft::preflight()?; let db = db::open(&cfg.db_path)?;` 之後加：

```rust
    // v6 遷移路徑（補填 Email、username 登入）已移除。還有這種帳號的話它
    // 登不進來，只能由 admin 刪掉後重新邀請。
    match db::users_without_email(&db) {
        Ok(0) => {}
        Ok(n) => tracing::error!("{n} 個帳號沒有 email，無法登入；請刪除後重新邀請"),
        Err(e) => tracing::warn!("檢查缺 email 的帳號失敗: {e:#}"),
    }
```

既有測試 `every_endpoint_accepts_the_method_the_frontend_sends` 若列有 `/api/me/email`，把那一項刪掉（用 `grep -n 'me/email' app/control/src/main.rs` 確認）。

- [ ] **步驟 4：撰寫實作（前端與文件）**

- 刪除 `app/control/web/src/screens/NeedEmail.svelte`。
- `App.svelte`：刪除 `import NeedEmail …` 與 `{:else if app.status.needs_email} <Msg /> <NeedEmail />` 分支。
- `api.js`：刪除 `setMyEmail` 那一行。
- `Login.svelte`：刪除「v6 之前的帳號還沒有信箱」的註解與其下那段 `<p>`。
- `docs/CONTROL.md:216` 那段改為：

```
`added_by` 存的是**顯示名稱**而不是 user_id（v1 就留下的形狀）。所有帳號的稱呼都是 email、註冊後不變，所以比對得上；v6 遷移期的 `rename_owner` 已隨補填流程一起移除。
```

- `docs/CONTROL.md:528` 刪除 `/api/me/email` 那一列。

- [ ] **步驟 5：執行測試並建置前端**

執行：`cd app/control && cargo test && cd web && bun run build`
預期：全部通過（含移除一個 db 測試）；build 無錯誤。

- [ ] **步驟 6：Commit**

```bash
git add app/control/src app/control/web/src docs/CONTROL.md
git commit -m "移除 v6 遷移路徑：補填 Email 端點、username 登入退路與 NeedEmail 畫面"
```

### 任務 4：既有白名單條目只有擁有者或 admin 能改寫（發現 #10，low）

原報告提到「可另設只允許單調延長的共享操作」，審查指出那仍讓 member 永久維持
他人的授權並重設提醒狀態，而且 owner 查詢與更新分成兩步有競態。這裡採更簡單
的規則：別人的 IP 一律拒絕，所有權爭議交給 admin；判斷放進同一句 SQL。

**檔案：**
- 修改：`app/control/src/db.rs:835-855`（新增 `upsert_allow_owned`；`upsert_allow` 保留給 `nft::import_legacy` 與既有測試）
- 修改：`app/control/src/main.rs:1017-1046`（`allow_add`）
- 測試：`app/control/src/db.rs` 的 `mod tests`（`:1962` 起）

- [ ] **步驟 1：撰寫失敗的測試**

```rust
/// 既有條目只有擁有者或 admin 能改寫；別人的 IP 一律拒絕，而且判斷放在
/// 同一句 SQL 裡，沒有「先查 owner 再寫」的競態。
#[test]
fn only_the_owner_or_an_admin_can_rewrite_an_entry() {
    let db = test_db();
    let t30 = now() + 30 * 86400;
    let find = |db: &Db| list_allow(db).unwrap().into_iter().find(|e| e.ip == "1.2.3.4").unwrap();
    assert!(upsert_allow_owned(&db, "1.2.3.4", Some("老家"), "a@x", t30, 30, false).unwrap());

    // 別人：拒絕，什麼都不變
    assert!(!upsert_allow_owned(&db, "1.2.3.4", Some("改名"), "b@x", now() + 86400, 1, false).unwrap());
    let e = find(&db);
    assert_eq!(
        (e.expires_at, e.ttl_days, e.label.as_deref(), e.added_by.as_deref()),
        (t30, 30, Some("老家"), Some("a@x"))
    );

    // 擁有者：可改
    assert!(upsert_allow_owned(&db, "1.2.3.4", None, "a@x", t30 + 86400, 7, false).unwrap());
    let e = find(&db);
    assert_eq!((e.expires_at, e.ttl_days, e.label.as_deref()), (t30 + 86400, 7, Some("老家")));

    // admin：可改，但 owner 不變
    assert!(upsert_allow_owned(&db, "1.2.3.4", Some("管理員改"), "root@x", t30, 30, true).unwrap());
    let e = find(&db);
    assert_eq!((e.label.as_deref(), e.added_by.as_deref()), (Some("管理員改"), Some("a@x")));
}
```

- [ ] **步驟 2：執行測試以確認它失敗**

執行：`cd app/control && cargo test -- only_the_owner_or_an_admin`
預期：編譯錯誤 `cannot find function upsert_allow_owned`。

- [ ] **步驟 3：撰寫實作**

`db.rs` 在 `upsert_allow` 之後新增：

```rust
/// 新增或改寫一條白名單。既有條目只有擁有者（或 admin）能改寫；別人的 IP
/// 直接拒絕，回 false 且什麼都不動。
///
/// 檢查與寫入是同一句 SQL：`DO UPDATE … WHERE` 不成立時 changes() 是 0，
/// 沒有「先查 owner 再 UPDATE」中間被人插隊的空隙。admin 改寫時 `added_by`
/// 保持原擁有者 —— 那是「誰的網路」，不是「誰最後按了按鈕」。
pub fn upsert_allow_owned(
    db: &Db,
    ip: &str,
    label: Option<&str>,
    owner: &str,
    expires_at: i64,
    ttl_days: i64,
    is_admin: bool,
) -> Result<bool> {
    let conn = db.lock().unwrap();
    let n = conn.execute(
        "INSERT INTO allowlist (ip, label, added_by, added_at, expires_at, ttl_days)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(ip) DO UPDATE SET
             label       = coalesce(excluded.label, allowlist.label),
             expires_at  = excluded.expires_at,
             ttl_days    = excluded.ttl_days,
             expiry_notified_at = NULL
         WHERE allowlist.added_by = excluded.added_by OR ?7",
        params![ip, label, owner, now(), expires_at, ttl_days, is_admin],
    )?;
    Ok(n == 1)
}
```

`allow_add` 從 `db::purge_expired(&st.db)?;` 之後改成：

```rust
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
    if !db::upsert_allow_owned(&st.db, &ip_str, req.label.as_deref(), &username, expires_at, ttl_days, me.is_admin())? {
        return Err(AppError(anyhow::anyhow!(
            "{ip_str} 已由其他成員授權，只有本人或管理員能修改"
        )));
    }
```

函式開頭的 `let (_, username) = require_user(…)` 改成 `let (uid, username) = require_user(&st, &session).await?;`；後面的 `nft::sync` 與 `db::audit(…, "allow_add", …)` 不變。

- [ ] **步驟 4：執行測試以確認它通過**

執行：`cd app/control && cargo test`
預期：全部通過。

- [ ] **步驟 5：Commit**

```bash
git add app/control/src/db.rs app/control/src/main.rs
git commit -m "白名單：既有條目只有擁有者或 admin 能改寫，判斷與寫入合為同一句 SQL"
```

### 任務 5：清單、單封讀取、刪除共用同一條可見性規則（發現 #11，low）

**檔案：**
- 修改：`app/control/src/db.rs:976-1002`（抽 `row_to_mail`、新增 `get_mail`）、`:1203-1206`（`delete_mail` 改 `delete_mail_if`）
- 修改：`app/control/src/main.rs:1390-1449`（新增 `MailScope`、`mode_allows`；改 `mail_delete`）
- 測試：`app/control/src/main.rs` 的 `mod tests`

- [ ] **步驟 1：撰寫失敗的測試**

```rust
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
    assert_eq!(db::recent_mails(&st.db, 10).unwrap().len(), 3);
}
```

- [ ] **步驟 2：執行測試以確認它失敗**

執行：`cd app/control && cargo test -- members_can_only_delete_mail`
預期：FAIL，`deleted` 是 4。

- [ ] **步驟 3：撰寫實作（db）**

`db.rs`：把 `recent_mails` 的 `query_map` 閉包抽成獨立函式並共用欄位清單：

```rust
const MAIL_COLS: &str = "id, received_at, sender, subject, code, body, links, html, verified,
                         platform, skip_reason, recipient";

fn row_to_mail(r: &rusqlite::Row) -> rusqlite::Result<Mail> {
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
}

pub fn recent_mails(db: &Db, limit: i64) -> Result<Vec<Mail>> {
    let conn = db.lock().unwrap();
    let mut stmt = conn.prepare(&format!(
        "SELECT {MAIL_COLS} FROM mails ORDER BY received_at DESC LIMIT ?1"
    ))?;
    let rows = stmt.query_map(params![limit], row_to_mail)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn get_mail(db: &Db, id: i64) -> Result<Option<Mail>> {
    let conn = db.lock().unwrap();
    Ok(conn
        .query_row(&format!("SELECT {MAIL_COLS} FROM mails WHERE id = ?1"), params![id], row_to_mail)
        .optional()?)
}

/// 讀出、判斷、刪除都在同一把鎖內：判斷用的是刪除當下那一列，不會被中間
/// 插進來的寫入改變。`pred` 回 false 與列不存在都回 0 —— 不給枚舉 id 的人
/// 一個存在性 oracle。
pub fn delete_mail_if(db: &Db, id: i64, pred: impl Fn(&Mail) -> bool) -> Result<usize> {
    let conn = db.lock().unwrap();
    let row = conn
        .query_row(&format!("SELECT {MAIL_COLS} FROM mails WHERE id = ?1"), params![id], row_to_mail)
        .optional()?;
    let Some(m) = row else { return Ok(0) };
    if !pred(&m) {
        return Ok(0);
    }
    Ok(conn.execute("DELETE FROM mails WHERE id = ?1", params![id])?)
}
```

刪掉原本的 `delete_mail`；用 `grep -rn 'delete_mail(' app/control/src` 確認沒有其他呼叫點。

- [ ] **步驟 4：撰寫實作（handler）**

`main.rs` 在 `mail_list` 之前加：

```rust
/// `sender_verify_mode` 對一封信的裁決。
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
            // 有碼、或命中關鍵字（Netflix 的「暫時存取碼」碼在按鈕後面）
            && mail::is_actionable(subject, body, has_code, &self.keywords, &self.excludes)
    }

    fn allows_mail(&self, m: &db::Mail) -> bool {
        self.allows(m.platform.as_deref(), m.verified, m.subject.as_deref(), m.body.as_deref(), m.code.is_some())
    }
}
```

`mail_delete`：

```rust
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
```

`mail_list` 的三個 `.filter(…)` 改成一個：先 `let scope = MailScope::load(&st, &uid)?;`，再 `.filter(|m| scope.allows(m.platform.as_deref(), m.verified, m.subject.as_deref(), m.body.as_deref(), m.code.is_some()))`（原本讀 `granted`／`mode`／`keywords`／`excludes` 的四行刪掉；任務 13 會再把這支換成摘要查詢）。

- [ ] **步驟 5：執行測試以確認它通過**

執行：`cd app/control && cargo test`
預期：全部通過。

- [ ] **步驟 6：Commit**

```bash
git add app/control/src/db.rs app/control/src/main.rs
git commit -m "郵件可見性抽成 MailScope，刪除改為同一把鎖內讀出、判斷、刪除"
```

---

## 階段二：郵件 ingest 的可信度

### 任務 6：HTML 解析對任意 Unicode 不得 panic（發現 #13，low）

**檔案：**
- 修改：`app/control/src/mail.rs:275-283`（`decode_entities`）、`:318-322`（`html_to_text`）
- 測試：`app/control/src/mail.rs` 的 `mod tests`（`:434` 起）

- [ ] **步驟 1：撰寫失敗的測試**

```rust
/// `İ`（U+0130）小寫化後變成兩個 code point、byte 數改變 —— 以前拿原字串
/// 的 byte 位移去切小寫字串，會落在字元中間而 panic。
#[test]
fn html_with_length_changing_lowercase_does_not_panic() {
    let t = html_to_text("İ<script>x</script><p>ok</p>");
    assert!(t.contains("ok"));
    assert!(!t.contains("x"), "script 內容要整段丟掉");
}

/// 實體視窗以前固定切 12 bytes，第 12 個 byte 落在多位元字元中間就 panic。
#[test]
fn entity_window_never_splits_a_multibyte_char() {
    assert_eq!(decode_entities("&a日日日日;"), "&a日日日日;");
    assert_eq!(decode_entities("&amp;日"), "&日");
}

/// 整個解析器對任意 Unicode 都不得 unwind。
#[test]
fn parser_survives_hostile_unicode_html() {
    for s in ["İ<", "<İ>", "&İİİİ;", "<p>İ</p>", "&#x1F600;İ<br", "ﬀ<style>İ</style>ﬀ", "<İ"] {
        let _ = html_to_text(s);
        let _ = decode_entities(s);
    }
}
```

- [ ] **步驟 2：執行測試以確認它失敗**

執行：`cd app/control && cargo test -- does_not_panic never_splits survives_hostile`
預期：至少兩個 FAIL，訊息含 `byte index … is not a char boundary`。

- [ ] **步驟 3：撰寫實作**

`html_to_text`：把 `let lower = html.to_lowercase();` 改成

```rust
    // 只做 ASCII 小寫：byte 長度與字元邊界跟原字串完全相同，下面用原字串的
    // `i` 去切 `lower` 才安全。要比對的標籤名本來就是 ASCII。
    let lower = html.to_ascii_lowercase();
```

`decode_entities`：把 `let Some(semi) = tail[..tail.len().min(12)].find(';') else {` 改成

```rust
        // 實體最長十來個字元。上限算「字元」不算 byte，才不會切在多位元字元中間
        let window_end = tail.char_indices().nth(12).map(|(i, _)| i).unwrap_or(tail.len());
        let Some(semi) = tail[..window_end].find(';') else {
```

- [ ] **步驟 4：執行測試以確認它通過**

執行：`cd app/control && cargo test`
預期：全部通過。

- [ ] **步驟 5：Commit**

```bash
git add app/control/src/mail.rs
git commit -m "郵件 HTML 解析：改 ASCII 小寫與字元視窗，任意 Unicode 不再 panic"
```

### 任務 7：只採信收信端自己寫的 `Authentication-Results`（發現 #4，medium）

**檔案：**
- 修改：`app/control/src/main.rs:36-105`（`Config` 加 `mail_authserv_id`）、`:1179`（呼叫 `mail::parse`）
- 修改：`app/control/src/mail.rs:87-101`（`parse` 簽章與表頭挑選）
- 修改：`.env.example`、`docker-compose.yml:58-90`（control 的 `environment`）
- 測試：`app/control/src/mail.rs` 的 `mod tests`

> [!WARNING]
> 這個修正**嚴格優於現狀**（不再把寄件者塞的表頭全部串起來看），但它依賴一件
> Cloudflare 沒有文件保證的事：收信端的 `Authentication-Results` 永遠在最上面、
> 而且寄件者自己塞的同名表頭不會排在它前面。`authserv-id` 不是秘密。收信端也會把寄件者可控的欄位（`smtp.mailfrom=` 等）回寫進自己的表頭，
> 所以段內比對必須以 token 錨定（見補強 commit）。另有回報指出
> Worker 收到的信可能根本沒有這個表頭（[workerd#6740](https://github.com/cloudflare/workerd/issues/6740)）——
> 那種情況會被判成未驗證，方向是安全的。Worker 端做不到「正規化外來表頭再送
> attestation」：它拿不到任何獨立於原始信的驗證結果。所以這項發現的關閉條件是
> 步驟 5 的 canary，不是單元測試。

- [ ] **步驟 1：撰寫失敗的測試**

```rust
fn raw_with(headers: &str) -> Vec<u8> {
    format!("{headers}\r\nFrom: a@netflix.com\r\nSubject: x\r\n\r\nbody\r\n").into_bytes()
}

/// 寄件者可以自己塞任意同名表頭，但收信端（Cloudflare）的表頭永遠加在
/// 最頂端。只看第一個，而且它的 authserv-id 要是我們自己的。
#[test]
fn forged_auth_results_below_the_real_one_are_ignored() {
    let m = parse(
        &raw_with(
            "Authentication-Results: mx.cloudflare.net; dkim=fail header.d=netflix.com\r\n\
             Authentication-Results: mx.cloudflare.net; dkim=pass header.d=netflix.com",
        ),
        "mx.cloudflare.net",
    );
    assert!(!m.auth.is_trusted(&["netflix.com".into()]));
}

#[test]
fn auth_results_from_an_unknown_authserv_are_ignored() {
    let m = parse(
        &raw_with("Authentication-Results: evil.example; dkim=pass header.d=netflix.com"),
        "mx.cloudflare.net",
    );
    assert!(m.auth.dkim_domains.is_empty());
}

/// RFC 8601 允許 authserv-id 後面帶版本號。
#[test]
fn the_real_authserv_still_passes() {
    let m = parse(
        &raw_with("Authentication-Results: MX.Cloudflare.NET 1; dkim=pass header.d=netflix.com"),
        "mx.cloudflare.net",
    );
    assert!(m.auth.is_trusted(&["netflix.com".into()]));
}
```

- [ ] **步驟 2：執行測試以確認它失敗**

執行：`cd app/control && cargo test -- forged_auth_results unknown_authserv real_authserv`
預期：編譯錯誤，`parse` 只接受一個參數。

- [ ] **步驟 3：撰寫實作**

`mail.rs` 的 `parse` 簽章改為 `pub fn parse(raw: &[u8], authserv_id: &str) -> Parsed`，把「可能有多個 Authentication-Results」那段換成：

```rust
    // 只採信**第一個**、且 authserv-id 是我們自己收信端的 Authentication-Results。
    // 寄件者可以在原始信裡塞任意同名表頭，但收信端的表頭永遠加在最頂端；
    // 把全部串起來看，等於讓寄件者替自己蓋「已認證」的章。
    // 先以名稱選出第一個表頭、再讀它的值，兩步不能合併：第一個表頭若值是空的，
    // `filter_map(as_text).next()` 會跳過它、拿到寄件者塞的第二個。
    let auth_raw = msg
        .headers()
        .iter()
        .find(|h| h.name().eq_ignore_ascii_case("authentication-results"))
        .and_then(|h| h.value().as_text())
        .filter(|v| authserv_matches(v, authserv_id))
        .unwrap_or_default();
    let (dkim_domains, dmarc_from) = parse_auth_results(auth_raw);
```

並在 `parse_auth_results` 之前加：

```rust
/// RFC 8601：第一個分號前是 authserv-id，可帶版本號，如 `mx.cloudflare.net 1`。
fn authserv_matches(value: &str, expected: &str) -> bool {
    let head = value.split(';').next().unwrap_or("");
    head.split_whitespace()
        .next()
        .is_some_and(|id| id.eq_ignore_ascii_case(expected))
}
```

`main.rs` 的 `Config` 加欄位與讀取：

```rust
    /// 收信端在 `Authentication-Results` 裡署名的 authserv-id。只有它寫的
    /// 驗證結果算數；Cloudflare Email Routing 是 `mx.cloudflare.net`。
    mail_authserv_id: String,
```
```rust
            mail_authserv_id: env_or("NFHH_MAIL_AUTHSERV_ID", "mx.cloudflare.net"),
```

`mail_ingest` 的 `let p = mail::parse(&body);` 改為 `let p = mail::parse(&body, &st.cfg.mail_authserv_id);`。用 `grep -n 'parse(' app/control/src/mail.rs` 找出 `mod tests` 內既有的 `parse(` 呼叫，一律補第二個參數 `"mx.cloudflare.net"`。

`.env.example` 在 `NFHH_MAIL_ALLOWED_SENDERS` 附近加：

```
# 收信端在 Authentication-Results 署名的 authserv-id。只有它寫的驗證結果算數。
# Cloudflare Email Routing 固定是 mx.cloudflare.net，換收信服務才需要改。
NFHH_MAIL_AUTHSERV_ID=mx.cloudflare.net
```

`docker-compose.yml` 的 control `environment` 在 `NFHH_MAIL_KEEP_DAYS` 之後加（只改 `.env.example` 不會傳進容器）：

```yaml
      # 只採信這個 authserv-id 寫的 Authentication-Results。見 docs/DECISIONS.md
      NFHH_MAIL_AUTHSERV_ID: "${NFHH_MAIL_AUTHSERV_ID:-mx.cloudflare.net}"
```

- [ ] **步驟 4：執行測試以確認它通過**

執行：`cd app/control && cargo test`
預期：全部通過（`eml()` 測試樣本本來就以 `mx.cloudflare.net;` 開頭）。

- [ ] **步驟 5：部署後 canary（人工，決定這項發現能不能關）**

用專案已有的 Resend 金鑰寄一封**帶偽造表頭**的信到平台信箱（Resend 支援自訂表頭）：

```bash
curl -s https://api.resend.com/emails -H "Authorization: Bearer $RESEND_API_KEY" -H 'content-type: application/json' -d '{
  "from": "canary@'"$NFHH_MAIL_DOMAIN"'",
  "to": ["netflix@'"$NFHH_MAIL_DOMAIN"'"],
  "subject": "authserv canary",
  "text": "verification code 000000",
  "headers": { "Authentication-Results": "mx.cloudflare.net; dkim=pass header.d=netflix.com" }
}'
```

然後：

```bash
docker compose logs --since 5m control | grep -A1 'authserv canary'
```

再寄第二封，**不帶**偽造表頭，但寄件位址的 local part 夾帶 token：
`"from": "dkim=pass.header.d=netflix.com@'"$NFHH_MAIL_DOMAIN"'"`（Resend 若拒絕這個位址，
改用任何你能控制的網域寄出）。這封測的是收信端把 `smtp.mailfrom=` 回寫進自己的
`Authentication-Results` 時，解析器不會被夾帶的 `dkim=pass header.d=` 騙到。

預期：兩封信的日誌都是 `verified=false`，且稽核各有一筆 `mail_sender_unverified`（`grep -c` 面板 `/api/audit`）。

- 若兩封都 `verified=false`：Cloudflare 確實把自己的表頭放在最前面（或剝掉了外來的），而且解析器對回寫的寄件者欄位免疫。在 DECISIONS.md 的「只採信第一個 Authentication-Results」一節記下 canary 日期與結果，這項發現關閉。
- 若 `verified=true`：偽造成功，程式端沒有更多可做的事。記為**未關閉的殘餘風險**，向專案擁有者提出決策：要嘛把 `sender_verify_mode` 與 `forward_enforce` 視為不可信（等於沒有寄件者驗證，只剩 `platform_senders` 的信封比對），要嘛換一個會提供獨立驗證結果的收信服務。

- [ ] **步驟 6：Commit**

```bash
git add app/control/src/mail.rs app/control/src/main.rs .env.example docker-compose.yml
git commit -m "寄件者驗證只採信第一個、authserv-id 為收信端的 Authentication-Results"
```

### 任務 8：`mail_ingest` 改讀面板設定的可信網域（發現 #12，low）

**檔案：**
- 修改：`app/control/src/main.rs:1180`（`verified` 的來源）、`:3017-3024`（seed 區塊抽成函式）、`mod tests` 的 `state_with`
- 測試：`app/control/src/main.rs` 的 `mod tests`

- [ ] **步驟 1：撰寫失敗的測試**

```rust
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
            mail_ingest(State(st), h(), eml("code", "code 123456", id)).await.map_err(|e| e.0).unwrap().0
        }
    };

    db::set_setting_list(&st.db, db::keys::SENDER_DOMAINS, &["example.org".into()], None).unwrap();
    assert_eq!(ingest("d1").await["verified"], false, "UI 移除 netflix.com 後要立刻不信任");

    db::set_setting_list(&st.db, db::keys::SENDER_DOMAINS, &["netflix.com".into()], None).unwrap();
    assert_eq!(ingest("d2").await["verified"], true);
}
```

- [ ] **步驟 2：執行測試以確認它失敗**

執行：`cd app/control && cargo test -- sender_domains_edited_in_the_panel`
預期：FAIL，第一個斷言 `verified` 仍為 true。

- [ ] **步驟 3：撰寫實作**

`mail_ingest`：`let verified = p.auth.is_trusted(&st.cfg.mail_allowed_senders);` 改為

```rust
    // 以面板設定為準：環境變數只是首次啟動的種子（見 seed_settings）。
    // 以前這裡讀 Config，UI 撤銷的網域會一直被信任到下次重啟。
    let verified = p.auth.is_trusted(&db::get_setting_list(&st.db, db::keys::SENDER_DOMAINS));
```

把 `main()` 裡從「環境變數只是**種子**」註解到 `seed_platform_mailboxes(&db, &cfg);` 那整段抽成：

```rust
/// 環境變數只是**種子**：面板改過的設定不該被下一次重啟蓋回去，所以一律
/// 只在鍵不存在時寫入。測試的 state 也要走這裡，否則 ingest 讀到空清單。
fn seed_settings(db: &db::Db, cfg: &Config) {
    let _ = db::seed_setting(db, db::keys::SENDER_MODE, if cfg.mail_enforce_sender { "enforce" } else { "observe" });
    let _ = db::set_setting_list_if_absent(db, db::keys::SENDER_DOMAINS, &cfg.mail_allowed_senders);
    let _ = db::set_setting_list_if_absent(db, db::keys::CODE_KEYWORDS, &[]);
    let _ = db::set_setting_list_if_absent(db, db::keys::CODE_EXCLUDES, &[]);
    let _ = db::seed_setting(db, db::keys::FORWARD_ENFORCE, "1");
    let _ = db::seed_setting(db, db::keys::MAIL_DOMAIN, &cfg.mail_domain);
    seed_platform_mailboxes(db, cfg);
}
```

`main()` 原位置改成 `seed_settings(&db, &cfg);`。`state_with` 改為先建 db 再 seed：

```rust
        let db = db::test_db();
        seed_settings(&db, &cfg);
        Arc::new(AppState { db, webauthn, cfg, … })
```

- [ ] **步驟 4：執行測試以確認它通過**

執行：`cd app/control && cargo test`
預期：全部通過。

- [ ] **步驟 5：Commit**

```bash
git add app/control/src/main.rs
git commit -m "寄件者可信網域改由 DB 決定，環境變數僅作首次種子"
```

### 任務 9：ingest 回具型別的狀態碼，Worker 只在「不可用」時 fail-open（發現 #5，medium）

**檔案：**
- 修改：`app/control/src/main.rs:1140-1165`（`require_mail_secret`）、`:1170-1300`（`mail_ingest`）
- 修改：`app/cloudflare/email-worker.js`
- 建立：`app/cloudflare/email-worker.test.js`
- 測試：`app/control/src/main.rs` 的 `mod tests`

- [ ] **步驟 1：撰寫失敗的測試（後端）**

```rust
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

    assert_eq!(
        IngestError::Unprocessable("壞信".into()).into_response().status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        IngestError::Internal(anyhow::anyhow!("db")).into_response().status(),
        StatusCode::INTERNAL_SERVER_ERROR
    );
}
```

- [ ] **步驟 2：撰寫失敗的測試（Worker）**

建立 `app/cloudflare/email-worker.test.js`：

```js
import { test, expect } from "bun:test";
import { classifyResponse, decideTargets } from "./email-worker.js";

const env = {
  FALLBACK_TO: "me@x",
  FORWARD_MAP: JSON.stringify({ "netflix@share.x": ["fam@x"] }),
};

test("面板拒收：只轉給 FALLBACK_TO，不走 FORWARD_MAP", () => {
  expect(decideTargets({ kind: "rejected", status: 422 }, "netflix@share.x", env)).toEqual(["me@x"]);
});

test("面板不可用或未設定：才走 FORWARD_MAP", () => {
  expect(decideTargets({ kind: "unavailable" }, "netflix@share.x", env)).toEqual(["me@x", "fam@x"]);
  expect(decideTargets({ kind: "unconfigured" }, "netflix@share.x", env)).toEqual(["me@x", "fam@x"]);
});

test("面板回覆：照單轉", () => {
  expect(decideTargets({ kind: "ok", panel: { forward_to: ["a@x"] } }, "netflix@share.x", env))
    .toEqual(["me@x", "a@x"]);
});

test("4xx 與壞 JSON 是拒收，5xx 是不可用", async () => {
  expect((await classifyResponse(new Response("", { status: 401 }))).kind).toBe("rejected");
  expect((await classifyResponse(new Response("", { status: 422 }))).kind).toBe("rejected");
  expect((await classifyResponse(new Response("not json", { status: 200 }))).kind).toBe("rejected");
  expect((await classifyResponse(new Response("", { status: 503 }))).kind).toBe("unavailable");
  expect((await classifyResponse(new Response('{"forward_to":[]}', { status: 200 }))).kind).toBe("ok");
});
```

- [ ] **步驟 3：執行測試以確認它們失敗**

執行：`cd app/control && cargo test -- ingest_errors_carry_a_status`
預期：編譯錯誤 `cannot find type IngestError`。

執行：`cd app/cloudflare && bun test`
預期：FAIL，`classifyResponse` 不是 export。

- [ ] **步驟 4：撰寫實作（後端）**

`main.rs` 在 `require_mail_secret` 之前加：

```rust
/// ingest 的錯誤要讓 Worker 分得出「拒收」與「面板掛了」：前者不該退回
/// 未過濾的 FORWARD_MAP，後者才該。一般 `AppError` 一律回 400，分不出來。
#[derive(Debug)]
enum IngestError {
    /// 密鑰不符或端點未啟用 —— 永久性，Worker 不得 fail-open
    Unauthorized,
    /// 這封信本身解析不了 —— 永久性，重送也不會變
    Unprocessable(String),
    /// 面板自己的問題（DB 等）—— 暫時性，Worker 可走退路
    Internal(anyhow::Error),
}

impl IntoResponse for IngestError {
    fn into_response(self) -> Response {
        let (status, msg) = match self {
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
```

`require_mail_secret` 回傳型別改為 `std::result::Result<(), IngestError>`，兩個 `Err(AppError(…))` 都改成 `Err(IngestError::Unauthorized)`（`tracing::warn!` 保留）。用 `grep -n 'require_mail_secret(' app/control/src/main.rs` 確認只有 `mail_ingest` 呼叫它。

`mail_ingest` 回傳型別改為 `std::result::Result<Json<serde_json::Value>, IngestError>`，解析那行改為：

```rust
    // 任務 6 已修掉已知的 panic；這層是防線：解析器再出問題也只影響這封信，
    // 而且回的是 422，Worker 會當「拒收」而不是「面板掛了」。
    let authserv = st.cfg.mail_authserv_id.clone();
    let p = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| mail::parse(&body, &authserv)))
        .map_err(|_| IngestError::Unprocessable("信件解析失敗".into()))?;
```

函式內其餘 `?` 都是 `anyhow::Result`，透過 `From<anyhow::Error>` 自動轉換。既有 ingest 測試裡的 `.map_err(|e| e.0)` 全部移除（`IngestError` 有 `Debug`，可直接 `unwrap`）；任務 8 的測試也一併改。

- [ ] **步驟 5：撰寫實作（Worker）**

`email-worker.js` 的 `email()` 開頭改成：

```js
    const outcome = await pushToPanel(message, env);
    if (outcome.kind === "ok") {
      const panel = outcome.panel;
      console.log(
        `面板回覆：篩選器${panel.actionable ? "通過" : "擋下"}` +
          ` verified=${panel.verified} 家人 ${(panel.forward_to || []).length} 人` +
          `${panel.new === false ? "（重送，已存在）" : ""}`
      );
    }
    const targets = decideTargets(outcome, message.to, env);
```

`pushToPanel` 改為回 outcome 物件：

```js
/**
 * 推信給面板並分類結果，絕不 throw：
 *   { kind: "ok", panel }            2xx 且 JSON 合法
 *   { kind: "rejected", status }     任何 4xx、或 2xx 但 JSON 壞掉 —— 永久性
 *   { kind: "unavailable", reason }  5xx、逾時、連不上 —— 暫時性
 *   { kind: "unconfigured" }         沒有 PANEL_ENDPOINT / PANEL_SECRET —— 部署狀態，
 *                                    不是攻擊面（攻擊者改不了 Worker 的環境變數）。
 *                                    面板還沒上線時 FORWARD_MAP 是唯一的轉發路徑。
 */
async function pushToPanel(message, env) {
  if (!env.PANEL_ENDPOINT || !env.PANEL_SECRET) {
    console.log("⚠️ PANEL_ENDPOINT / PANEL_SECRET 未設定：沒有寄件者驗證，只依 FORWARD_MAP 轉發");
    return { kind: "unconfigured" };
  }
  try {
    const res = await fetch(env.PANEL_ENDPOINT, { /* 原本的參數不變 */ });
    return await classifyResponse(res);
  } catch (e) {
    console.log("push failed:", e && e.message);
    return { kind: "unavailable", reason: e && e.message };
  }
}

export async function classifyResponse(res) {
  if (res.status >= 500) return { kind: "unavailable", reason: `HTTP ${res.status}` };
  if (!res.ok) {
    // 面板的錯誤回應是 {"error": …}，不含信件內容，可安全記錄
    const detail = await res.text().catch(() => "");
    console.log("panel rejected:", res.status, detail.slice(0, 200));
    return { kind: "rejected", status: res.status };
  }
  try {
    return { kind: "ok", panel: await res.json() };
  } catch {
    return { kind: "rejected", status: res.status, reason: "bad json" };
  }
}

/**
 * 決定轉發名單。只有「面板不可用」才准走 FORWARD_MAP；「面板拒收」只給
 * FALLBACK_TO —— 拒收的信本來就不該無過濾地送進家人信箱。
 */
export function decideTargets(outcome, to, env) {
  switch (outcome.kind) {
    case "ok":
      return withFallback(outcome.panel.forward_to, env);
    case "rejected":
      console.log(`⚠️ 面板拒收（${outcome.status}），只轉給 FALLBACK_TO，不走 FORWARD_MAP`);
      return withFallback([], env);
    case "unconfigured":
      return withFallback(fallbackRecipients(to, env), env);
    default:
      console.log("⚠️ 面板無回應，改用 FORWARD_MAP 轉發（面板不會有這封信的紀錄）");
      return withFallback(fallbackRecipients(to, env), env);
  }
}
```

檔頭註解「面板掛掉不能讓信轉不出去」那段補一句：「**拒收**（4xx）不算掛掉，只轉給 FALLBACK_TO。」

- [ ] **步驟 6：執行測試以確認它們通過**

執行：`cd app/control && cargo test`
預期：全部通過。

執行：`cd app/cloudflare && bun test`
預期：`4 pass, 0 fail`。

- [ ] **步驟 7：Commit**

```bash
git add app/control/src/main.rs app/cloudflare/email-worker.js app/cloudflare/email-worker.test.js
git commit -m "ingest 回 401/422/500，Worker 只在面板不可用時退回 FORWARD_MAP"
```

> [!NOTE]
> 部署順序是**後端先、Worker 後**。舊 Worker 把新後端的 401／422／500 全當 `null`
> 走 FORWARD_MAP，跟今天完全相同；反過來新 Worker 對舊後端的 400 會當拒收、只轉
> FALLBACK_TO，會有一段少轉的空窗。見「部署與驗收」。

---

## 階段三：資源耗盡

### 任務 10：以可信的 `ingested_at` 做排序與保留，並限制 metadata 長度（發現 #7，medium）

**檔案：**
- 修改：`app/control/src/db.rs:21-73`（`migrate` 加 v12）、`:937-975`（`insert_mail`）、`:976-982`（`recent_mails` 排序）、`:1179-1185`（`purge_old_mails`）
- 修改：`app/control/src/mail.rs:104-150`（metadata 上限）
- 修改：`app/control/src/main.rs:1192`（`received` 夾範圍）
- 測試：`app/control/src/db.rs`、`app/control/src/main.rs` 的 `mod tests`

- [ ] **步驟 1：撰寫失敗的測試**

`db.rs` tests：

```rust
/// `received_at` 是寄件者說的 `Date:`，寄件者說了算。排序與清除只能用
/// 面板自己的時鐘 `ingested_at`，否則一封未來日期的信永遠排第一、永遠不被清。
#[test]
fn purge_and_ordering_use_ingested_at_not_the_claimed_date() {
    let db = test_db();
    let far_future = now() + 10 * 365 * 86400;
    insert_mail(&db, Some("future"), far_future, None, None, Some("未來"), None, None, None, &[], true, Some("netflix"), None).unwrap();
    insert_mail(&db, Some("normal"), now(), None, None, Some("正常"), None, None, None, &[], true, Some("netflix"), None).unwrap();

    let list = recent_mails(&db, 10).unwrap();
    assert_eq!(list[0].subject.as_deref(), Some("正常"), "後收到的排前面，不看 Date");

    // 把「未來」那封的 ingested_at 撥回 30 天前，保留期 14 天要把它清掉
    db.lock().unwrap()
        .execute("UPDATE mails SET ingested_at = ?1 WHERE message_id = 'future'", params![now() - 30 * 86400])
        .unwrap();
    assert_eq!(purge_old_mails(&db, 14).unwrap(), 1);
    assert_eq!(recent_mails(&db, 10).unwrap().len(), 1);
}

/// 升級前就存在的未來日期郵件，不能把偽造的時間原樣搬進可信欄位。
/// 造一顆 v11 的庫（跟 `migration_is_idempotent` 同一招：退版號、拿掉欄位）。
#[test]
fn v12_backfill_clamps_pre_existing_future_dates() {
    let conn = Connection::open_in_memory().unwrap();
    migrate(&conn).unwrap();
    conn.execute_batch("DROP INDEX IF EXISTS idx_mails_ingested; ALTER TABLE mails DROP COLUMN ingested_at;").unwrap();
    conn.pragma_update(None, "user_version", 11).unwrap();
    let future = now() + 10 * 365 * 86400;
    conn.execute(
        "INSERT INTO mails (message_id, received_at) VALUES ('f', ?1), ('p', 100)",
        params![future],
    )
    .unwrap();

    migrate(&conn).unwrap();
    let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
    assert_eq!(v, 12);
    let f: i64 = conn.query_row("SELECT ingested_at FROM mails WHERE message_id = 'f'", [], |r| r.get(0)).unwrap();
    let p: i64 = conn.query_row("SELECT ingested_at FROM mails WHERE message_id = 'p'", [], |r| r.get(0)).unwrap();
    assert!(f <= now(), "未來日期要被夾到遷移當下");
    assert_eq!(p, 100, "正常的保留原值");
}

/// 總量配額：不管日期怎麼寫，超過上限就從最舊收到的開始丟。
#[test]
fn mail_table_is_capped_by_row_count() {
    let db = test_db();
    for i in 0..(MAX_MAILS + 5) {
        insert_mail(&db, Some(&format!("m{i}")), now(), None, None, None, None, None, None, &[], true, None, None).unwrap();
    }
    purge_old_mails(&db, 14).unwrap();
    let n: i64 = db.lock().unwrap().query_row("SELECT count(*) FROM mails", [], |r| r.get(0)).unwrap();
    assert_eq!(n, MAX_MAILS);
}
```

`main.rs` tests：

```rust
/// 寄件者宣告的日期只當顯示用，太舊或在未來都改用現在。
#[test]
fn claimed_dates_are_clamped_to_a_sane_window() {
    let now = 1_800_000_000;
    assert_eq!(clamp_received(None, now), now);
    assert_eq!(clamp_received(Some(now - 60), now), now - 60);
    assert_eq!(clamp_received(Some(now + 10 * 365 * 86400), now), now);
    assert_eq!(clamp_received(Some(now - 400 * 86400), now), now);
    assert_eq!(clamp_received(Some(now + 1800), now), now + 1800, "時鐘誤差一小時內接受");
}
```

`mail.rs` tests：

```rust
/// 主旨、寄件者、Message-ID 沒有上限的話，每封信都能帶幾 MB 的表頭進資料庫。
#[test]
fn header_metadata_is_capped() {
    let long = "x".repeat(5000);
    let raw = format!(
        "From: {long}@example.com\r\nTo: {long}@share.example.com\r\nSubject: {long}\r\nMessage-ID: <{long}@x>\r\n\r\nhi\r\n"
    );
    let m = parse(raw.as_bytes(), "mx.cloudflare.net");
    assert!(m.subject.unwrap().chars().count() <= MAX_SUBJECT_CHARS);
    assert!(m.sender.map_or(true, |s| s.chars().count() <= MAX_ADDR_CHARS));
    assert!(m.recipient.map_or(true, |s| s.chars().count() <= MAX_ADDR_CHARS));
    assert!(m.message_id.map_or(true, |s| s.chars().count() <= MAX_MSGID_CHARS));
}
```

- [ ] **步驟 2：執行測試以確認它們失敗**

執行：`cd app/control && cargo test -- use_ingested_at v12_backfill capped_by_row_count clamped_to_a_sane header_metadata_is_capped`
預期：編譯錯誤（`MAX_MAILS`、`clamp_received`、`MAX_SUBJECT_CHARS` 不存在）。

- [ ] **步驟 3：撰寫實作**

`db.rs` 的 `migrate()` 在 v11 區塊後加：

```rust
    if version < 12 {
        migrate_v12(conn)?;
        conn.pragma_update(None, "user_version", 12)?;
    }
```

在 `migrate_v11` 之前加：

```rust
/// v12：可信的接收時間。
///
/// `received_at` 來自寄件者的 `Date:` 表頭 —— 拿它排序與清除，一封未來日期的
/// 信可以永遠排第一、永遠不被清。`ingested_at` 是面板自己的時鐘，
/// 排序、分頁、保留期一律只看它；`received_at` 降為顯示用。
fn migrate_v12(conn: &Connection) -> Result<()> {
    add_column(conn, "mails", "ingested_at", "INTEGER")?;
    // 回填不能原樣抄 received_at：升級前就躺在表裡的偽造未來日期會直接變成
    // 「可信」時間，繼續置頂、繼續逃過清理。夾到遷移當下。
    conn.execute_batch(
        "UPDATE mails SET ingested_at = min(received_at, unixepoch()) WHERE ingested_at IS NULL;
         CREATE INDEX IF NOT EXISTS idx_mails_ingested ON mails(ingested_at DESC);",
    )?;
    Ok(())
}
```

`insert_mail`：INSERT 欄位清單加 `ingested_at`，VALUES 加 `?13`，`params!` 尾端加 `now()`。

`recent_mails`：`ORDER BY received_at DESC` 改為 `ORDER BY ingested_at DESC, id DESC`。

`purge_old_mails` 改為：

```rust
/// 信件總量上限。日期可以偽造、Message-ID 可以每封不同，只有列數是寄件者
/// 控制不了的。
pub const MAX_MAILS: i64 = 2000;

/// 清除逾期信件並套用總量上限。兩者都只看 `ingested_at`。
pub fn purge_old_mails(db: &Db, keep_days: i64) -> Result<usize> {
    let conn = db.lock().unwrap();
    let a = conn.execute(
        "DELETE FROM mails WHERE ingested_at < ?1",
        params![now() - keep_days * 86400],
    )?;
    let b = conn.execute(
        "DELETE FROM mails WHERE id NOT IN
           (SELECT id FROM mails ORDER BY ingested_at DESC, id DESC LIMIT ?1)",
        params![MAX_MAILS],
    )?;
    Ok(a + b)
}
```

`main.rs` 在 `mail_ingest` 之前加，並把 `let received = p.date.unwrap_or_else(db::now);` 改為 `let received = clamp_received(p.date, db::now());`：

```rust
/// 寄件者宣告的日期只當顯示用，而且要夾在合理範圍：一年前到一小時後之外
/// 一律改用現在。排序與保留期另外看 `ingested_at`（見 db::migrate_v12）。
fn clamp_received(claimed: Option<i64>, now: i64) -> i64 {
    match claimed {
        Some(t) if t <= now + 3600 && t >= now - 365 * 86400 => t,
        _ => now,
    }
}
```

`mail.rs` 在 `Parsed` 之前加常數與輔助，並在 `parse` 組 `Parsed` 時套用：

```rust
/// 表頭欄位的上限。RFC 5322 一行是 998 bytes，主旨與位址實務上遠小於此；
/// 超過的不是正常信，是想撐大資料庫的人。
pub const MAX_SUBJECT_CHARS: usize = 500;
pub const MAX_ADDR_CHARS: usize = 320;
pub const MAX_MSGID_CHARS: usize = 998;

fn cap(s: String, max: usize) -> String {
    if s.chars().count() <= max { s } else { s.chars().take(max).collect() }
}
```

`Parsed { … }` 內：`subject` 改 `subject.map(|s| cap(s, MAX_SUBJECT_CHARS))`（`subject` 在 `haystack` 用過之後才 move，順序不變）；`message_id` 包 `cap(…, MAX_MSGID_CHARS)`；`sender` 與 `recipient` 包 `cap(…, MAX_ADDR_CHARS)`（`sender` 也被 `envelope_domain` 用到，先算 `envelope_domain` 再 move）。

- [ ] **步驟 4：執行測試以確認它們通過**

執行：`cd app/control && cargo test`
預期：全部通過（含既有 `migration_is_idempotent`）。

- [ ] **步驟 5：Commit**

```bash
git add app/control/src/db.rs app/control/src/mail.rs app/control/src/main.rs
git commit -m "郵件改以 ingested_at 排序與清除，Date 夾範圍，表頭欄位與總列數設上限"
```

### 任務 11：稽核表上限、輸入長度與公開端點限流（發現 #8，medium）

**檔案：**
- 建立：`app/control/src/ratelimit.rs`
- 修改：`app/control/src/db.rs:391-399`（`audit`）、新增 `purge_old_audit`
- 修改：`app/control/src/main.rs`：`mod` 清單、`Config`、`AppState`、`join_start`（`:261-280`）、`allow_add`／`allow_rename`（label 長度）、背景迴圈（`:2990-3003`）、`state_with`
- 測試：三個檔案的 `mod tests`

- [ ] **步驟 1：撰寫失敗的測試**

`ratelimit.rs`（整檔含測試，見步驟 3）。`db.rs` tests：

```rust
/// detail 會夾帶攻擊者控制的輸入（未受邀的 Email、白名單標籤）。
/// 不截斷等於讓公開端點無限寫入磁碟。
#[test]
fn audit_detail_is_truncated() {
    let db = test_db();
    audit(&db, None, "t", Some(&"é".repeat(10_000)), None);
    let row = recent_audit(&db, 1).unwrap().remove(0);
    assert_eq!(row.detail.unwrap().chars().count(), AUDIT_DETAIL_MAX_CHARS);
}

#[test]
fn audit_is_pruned_by_age_and_row_count() {
    let db = test_db();
    for i in 0..30 {
        audit(&db, None, "t", Some(&i.to_string()), None);
    }
    db.lock().unwrap().execute("UPDATE audit SET at = 0 WHERE id <= 5", []).unwrap();
    purge_old_audit(&db, 90, 20).unwrap();
    let n: i64 = db.lock().unwrap().query_row("SELECT count(*) FROM audit", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 20);
    let oldest: i64 = db.lock().unwrap().query_row("SELECT min(at) FROM audit", [], |r| r.get(0)).unwrap();
    assert!(oldest > 0, "0 秒那幾筆要先因保留期被清");
}
```

`main.rs` tests：

```rust
#[test]
fn email_must_look_like_an_address_and_fit_rfc_5321() {
    assert!(valid_email("a@b.c"));
    assert!(!valid_email("no-at"));
    assert!(!valid_email("a b@c"));
    assert!(!valid_email(&format!("{}@x", "a".repeat(MAX_EMAIL_LEN))));
}

/// 公開的 join/start 每次失敗都寫一列稽核；沒有限流就是一台免費寫入機。
#[tokio::test]
async fn join_start_is_rate_limited_per_ip() {
    let st = test_state();
    let h = || hdrs(&[("cf-connecting-ip", "203.0.113.9")]);
    let mut last = None;
    for _ in 0..(JOIN_LIMIT_PER_IP + 1) {
        last = Some(join_start(State(st.clone()), h(), Json(EmailReq { email: "x@y.z".into() })).await.map_err(|e| e.0).unwrap_err());
    }
    assert!(last.unwrap().to_string().contains("太頻繁"));
    let n: i64 = st.db.lock().unwrap().query_row("SELECT count(*) FROM audit", [], |r| r.get(0)).unwrap();
    assert!(n <= JOIN_LIMIT_PER_IP as i64, "被限流的請求不能再寫稽核");
}
```

- [ ] **步驟 2：執行測試以確認它們失敗**

執行：`cd app/control && cargo test -- audit_detail_is_truncated audit_is_pruned valid_email rate_limited_per_ip`
預期：編譯錯誤。

- [ ] **步驟 3：撰寫實作**

建立 `app/control/src/ratelimit.rs`：

```rust
//! 極簡固定視窗限流，給沒有登入的公開端點用（`/api/join/start`）。
//! 記憶體內、重啟歸零 —— 這裡要擋的是自動化灌入，不是精準計費。

use std::collections::HashMap;
use std::sync::Mutex;

pub struct Limiter {
    window_secs: i64,
    per_key: u32,
    global: u32,
    inner: Mutex<State>,
}

struct State {
    window_start: i64,
    total: u32,
    per_key: HashMap<String, u32>,
}

impl Limiter {
    pub fn new(window_secs: i64, per_key: u32, global: u32) -> Self {
        Self {
            window_secs,
            per_key,
            global,
            inner: Mutex::new(State { window_start: 0, total: 0, per_key: HashMap::new() }),
        }
    }

    /// 回 true = 放行並計數。視窗到期時整組歸零，所以 HashMap 不會無限長。
    pub fn allow(&self, key: &str, now: i64) -> bool {
        let mut s = self.inner.lock().unwrap();
        if now - s.window_start >= self.window_secs {
            s.window_start = now;
            s.total = 0;
            s.per_key.clear();
        }
        if s.total >= self.global {
            return false;
        }
        let n = s.per_key.entry(key.to_string()).or_insert(0);
        if *n >= self.per_key {
            return false;
        }
        *n += 1;
        s.total += 1;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn per_key_limit_then_window_reset() {
        let l = Limiter::new(60, 2, 100);
        assert!(l.allow("a", 0));
        assert!(l.allow("a", 1));
        assert!(!l.allow("a", 2), "第三次要擋");
        assert!(l.allow("b", 2), "別的 key 不受影響");
        assert!(l.allow("a", 60), "視窗過了要放行");
    }

    #[test]
    fn global_limit_caps_everyone() {
        let l = Limiter::new(60, 10, 3);
        assert!(l.allow("a", 0));
        assert!(l.allow("b", 0));
        assert!(l.allow("c", 0));
        assert!(!l.allow("d", 0));
    }
}
```

`db.rs`：

```rust
/// 稽核明細的上限。detail 可能夾帶攻擊者控制的輸入。
pub const AUDIT_DETAIL_MAX_CHARS: usize = 512;

pub fn audit(db: &Db, actor: Option<&str>, action: &str, detail: Option<&str>, ip: Option<&str>) {
    let detail = detail.map(|d| d.chars().take(AUDIT_DETAIL_MAX_CHARS).collect::<String>());
    if let Ok(conn) = db.lock() {
        let _ = conn.execute(
            "INSERT INTO audit (at, actor, action, detail, client_ip) VALUES (?1,?2,?3,?4,?5)",
            params![now(), actor, action, detail, ip],
        );
    }
}

/// 稽核的保留期與列數上限。這張表以前只進不出，公開端點的每一次失敗都永久佔一列。
pub fn purge_old_audit(db: &Db, keep_days: i64, max_rows: i64) -> Result<usize> {
    let conn = db.lock().unwrap();
    let a = conn.execute("DELETE FROM audit WHERE at < ?1", params![now() - keep_days * 86400])?;
    let b = conn.execute(
        "DELETE FROM audit WHERE id NOT IN (SELECT id FROM audit ORDER BY at DESC, id DESC LIMIT ?1)",
        params![max_rows],
    )?;
    Ok(a + b)
}
```

`main.rs`：

- `mod` 清單加 `mod ratelimit;`。
- `Config` 加 `audit_keep_days: i64`、`audit_max_rows: i64`；`from_env` 加 `audit_keep_days: env_or("NFHH_AUDIT_KEEP_DAYS", "90").parse().unwrap_or(90)`、`audit_max_rows: env_or("NFHH_AUDIT_MAX_ROWS", "20000").parse().unwrap_or(20000)`。
- `AppState` 加 `join_limiter: ratelimit::Limiter`；`main()` 建 state 時給 `join_limiter: ratelimit::Limiter::new(JOIN_LIMIT_WINDOW_SECS, JOIN_LIMIT_PER_IP, JOIN_LIMIT_GLOBAL)`；`state_with` 同樣。
- 常數（放在「加入（Email 驗證碼）」註解區塊之後）：

```rust
/// RFC 5321 的位址上限。再長的不是信箱，是想撐大資料庫的人。
const MAX_EMAIL_LEN: usize = 254;
/// 白名單標籤、裝置名稱等可顯示文字的上限。
const MAX_LABEL_LEN: usize = 128;
/// 公開端點 join/start 的限流：每個來源 IP 每 10 分鐘 10 次、全域 200 次。
const JOIN_LIMIT_WINDOW_SECS: i64 = 600;
const JOIN_LIMIT_PER_IP: u32 = 10;
const JOIN_LIMIT_GLOBAL: u32 = 200;

fn valid_email(s: &str) -> bool {
    s.len() <= MAX_EMAIL_LEN && s.contains('@') && !s.contains(char::is_whitespace)
}
```

- `join_start`：`if !email.contains('@')` 改為 `if !valid_email(&email)`；緊接著、在任何 DB 存取之前加：

```rust
    if !st.join_limiter.allow(ip.as_deref().unwrap_or("?"), db::now()) {
        return Err(AppError(anyhow::anyhow!("請求太頻繁，請稍後再試")));
    }
```

- `allow_add` 與 `allow_rename`：在 label 計算後加

```rust
    if req.label.as_deref().is_some_and(|l| l.chars().count() > MAX_LABEL_LEN) {
        return Err(AppError(anyhow::anyhow!("名稱最多 {MAX_LABEL_LEN} 個字")));
    }
```

- 300 秒背景迴圈的閉包多抓 `let (keep, max) = (cfg.audit_keep_days, cfg.audit_max_rows);`，`tick.tick().await;` 之後加 `let _ = db::purge_old_audit(&db, keep, max);`。
- `.env.example` 加 `NFHH_AUDIT_KEEP_DAYS=90` 與 `NFHH_AUDIT_MAX_ROWS=20000`（附一行說明）。
- `docker-compose.yml` 的 control `environment` 在 `NFHH_MAIL_KEEP_DAYS` 附近加：

```yaml
      # 稽核表的保留期與列數上限。公開端點的每次失敗都寫一列，沒有上限就是免費的磁碟
      NFHH_AUDIT_KEEP_DAYS: "${NFHH_AUDIT_KEEP_DAYS:-90}"
      NFHH_AUDIT_MAX_ROWS: "${NFHH_AUDIT_MAX_ROWS:-20000}"
```

- [ ] **步驟 4：執行測試以確認它們通過**

執行：`cd app/control && cargo test`
預期：全部通過。

- [ ] **步驟 5：Commit**

```bash
git add app/control/src/ratelimit.rs app/control/src/db.rs app/control/src/main.rs .env.example docker-compose.yml
git commit -m "稽核表加保留期與列數上限，公開 join/start 限流，Email 與標籤設長度上限"
```

### 任務 12：推播訂閱配額、欄位驗證與有界扇出（發現 #9，medium）

**檔案：**
- 修改：`app/control/src/push.rs:30`（`B64` 改 `pub(crate)`）、`:63-102`（`send` 的錯誤內容上限）、新增 `valid_keys`
- 修改：`app/control/src/db.rs:1766-1782`（`add_push_sub` 加配額）、`:1849-1863`（`push_subs_for_platform` 排除反覆失敗）
- 修改：`app/control/src/main.rs:2025-2056`（`push_subscribe`）、`:1334-1355`（`fan_out`）
- 測試：`app/control/src/db.rs`、`app/control/src/main.rs` 的 `mod tests`

- [ ] **步驟 1：撰寫失敗的測試**

`db.rs` tests：

```rust
/// 訂閱是「每台裝置一筆」，一個人不可能有幾百台。配額檢查與寫入在同一把鎖內。
/// 接手別人的 endpoint 也算新裝置 —— 否則配額用「重新登記別人的 endpoint」就繞掉了。
#[test]
fn push_subscriptions_are_capped_per_user() {
    let db = test_db();
    create_user_with_platforms(&db, "u", "u", "u", "member", None, &[]).unwrap();
    create_user_with_platforms(&db, "v", "v", "v", "member", None, &[]).unwrap();
    for i in 0..3 {
        assert!(add_push_sub(&db, "u", &format!("https://p.example/{i}"), "k", "a", None, 3).unwrap());
    }
    assert!(!add_push_sub(&db, "u", "https://p.example/new", "k", "a", None, 3).unwrap(), "第 4 台要拒絕");
    // 既有、自己的 endpoint 重新訂閱不算新裝置
    assert!(add_push_sub(&db, "u", "https://p.example/1", "k2", "a2", None, 3).unwrap());
    // 別人的 endpoint：對 u 來說是新裝置，配額已滿就拒絕，所有權也不會轉移
    assert!(add_push_sub(&db, "v", "https://p.example/v", "k", "a", None, 3).unwrap());
    assert!(!add_push_sub(&db, "u", "https://p.example/v", "k", "a", None, 3).unwrap());
    assert_eq!(list_push_subs(&db, "u").unwrap().len(), 3);
    assert_eq!(list_push_subs(&db, "v").unwrap().len(), 1);
}

/// 反覆失敗的訂閱不再參與扇出：攻擊者的假 endpoint 只能拖慢一陣子。
#[test]
fn repeatedly_failing_subscriptions_leave_the_fanout() {
    let db = test_db();
    create_user_with_platforms(&db, "u", "u", "u", "member", None, &["netflix".into()]).unwrap();
    add_push_sub(&db, "u", "https://p.example/1", "k", "a", None, 8).unwrap();
    let id = list_push_subs(&db, "u").unwrap()[0].id;
    for _ in 0..PUSH_MAX_FAILS {
        bump_push_fail(&db, id).unwrap();
    }
    assert!(push_subs_for_platform(&db, "netflix").unwrap().is_empty());
}
```

`main.rs` tests：

```rust
/// 扇出對每個訂閱開一個 task 且沒有上限：一個 member 先堆幾千筆訂閱，
/// 再寄一封信給自己，就能同時打開幾千條連線。這裡用本機假推送服務量
/// 「同時在飛的請求數」。
#[tokio::test]
async fn fan_out_never_exceeds_the_concurrency_cap() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let inflight = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let (i2, p2) = (inflight.clone(), peak.clone());
    let app = Router::new().route("/push", post(move || {
        let (i, p) = (i2.clone(), p2.clone());
        async move {
            let n = i.fetch_add(1, Ordering::SeqCst) + 1;
            p.fetch_max(n, Ordering::SeqCst);
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            i.fetch_sub(1, Ordering::SeqCst);
            StatusCode::CREATED
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    // 真實可用的金鑰材料：encrypt 要對 p256dh 做 ECDH，隨便塞會在送出前就失敗
    let ua = p256::SecretKey::random(&mut p256::elliptic_curve::rand_core::OsRng);
    let p256dh = push::B64.encode(ua.public_key().to_encoded_point(false).as_bytes());
    let auth = push::B64.encode([7u8; 16]);

    let st = test_state();
    let subs: Vec<db::PushSub> = (0..20)
        .map(|i| db::PushSub {
            id: i, user_id: "u".into(), endpoint: format!("http://{addr}/push"),
            p256dh: p256dh.clone(), auth: auth.clone(), label: None,
            created_at: 0, last_ok_at: None, fail_count: 0,
        })
        .collect();
    let n = push::Notification { title: "t".into(), body: "b".into(), tag: "netflix".into(), url: "/".into(), code: None };

    let started = std::time::Instant::now();
    fan_out(&st, subs, &n).await;

    assert!(peak.load(Ordering::SeqCst) <= PUSH_FANOUT_CONCURRENCY, "峰值 {}", peak.load(Ordering::SeqCst));
    assert!(started.elapsed() >= std::time::Duration::from_millis(500), "20 筆分 3 輪至少 600ms");
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
    assert!(!push::valid_keys(&good, &push::B64.encode([1u8; 8])));
    assert!(!push::valid_keys(&"A".repeat(4096), &auth), "先擋字串長度，不先解碼");
}
```

測試模組需要 `use p256::elliptic_curve::sec1::ToEncodedPoint;`。

- [ ] **步驟 2：執行測試以確認它們失敗**

執行：`cd app/control && cargo test -- capped_per_user leave_the_fanout concurrency_cap key_material_must_be_real`
預期：編譯錯誤。

- [ ] **步驟 3：撰寫實作**

`push.rs`：`const B64` 改 `pub(crate) const B64`；新增

```rust
/// 瀏覽器給的 p256dh 是 65 bytes 的未壓縮 P-256 公鑰、auth 是 16 bytes。
/// 先擋字串長度（87 與 22 個 base64url 字元，多一點容錯），再解碼，
/// 最後確認 p256dh 真的是曲線上的點 —— 解不開、長度不對、不在曲線上的
/// 都不是訂閱，是想塞進資料庫的任意字串。
pub fn valid_keys(p256dh: &str, auth: &str) -> bool {
    if p256dh.len() > 88 || auth.len() > 24 {
        return false;
    }
    let (Ok(pk), Ok(a)) = (B64.decode(p256dh), B64.decode(auth)) else {
        return false;
    };
    a.len() == 16 && p256::PublicKey::from_sec1_bytes(&pk).is_ok()
}
```

`send` 的非成功分支改為只讀前 4 KiB：

```rust
        if !status.is_success() {
            // 只讀前 4 KiB：推送服務的錯誤內容不值得為它佔記憶體，
            // 惡意 endpoint 更可能回一大坨
            let mut res = res;
            let mut buf = Vec::new();
            while let Ok(Some(chunk)) = res.chunk().await {
                buf.extend_from_slice(&chunk);
                if buf.len() >= 4096 {
                    break;
                }
            }
            let detail = String::from_utf8_lossy(&buf);
            bail!("推送服務回 {status}：{}", detail.chars().take(200).collect::<String>());
        }
```

`db.rs`：

```rust
/// 連續失敗這麼多次就不再對它扇出。
pub const PUSH_MAX_FAILS: i64 = 10;

/// 新增或更新一筆訂閱。回 false = 這個人的裝置數已達 `max_per_user`。
///
/// endpoint 衝突時整筆蓋掉並把 `fail_count` 歸零 —— 那台裝置又活著了。
/// 只有「已經是自己的 endpoint」不佔新配額；接手別人的算新裝置，否則
/// 配額用「重新登記別人的 endpoint」就繞掉了。整段在同一把鎖內。
pub fn add_push_sub(
    db: &Db,
    user_id: &str,
    endpoint: &str,
    p256dh: &str,
    auth: &str,
    label: Option<&str>,
    max_per_user: i64,
) -> Result<bool> {
    let conn = db.lock().unwrap();
    let endpoint = endpoint.trim();
    let owner: Option<String> = conn
        .query_row(
            "SELECT user_id FROM push_subscriptions WHERE endpoint = ?1",
            params![endpoint],
            |r| r.get(0),
        )
        .optional()?;
    if owner.as_deref() != Some(user_id) {
        let n: i64 = conn.query_row(
            "SELECT count(*) FROM push_subscriptions WHERE user_id = ?1",
            params![user_id],
            |r| r.get(0),
        )?;
        if n >= max_per_user {
            return Ok(false);
        }
    }
    conn.execute(
        "INSERT INTO push_subscriptions
             (user_id, endpoint, p256dh, auth, label, created_at)
         VALUES (?1,?2,?3,?4,?5,?6)
         ON CONFLICT(endpoint) DO UPDATE SET
             user_id = excluded.user_id, p256dh = excluded.p256dh,
             auth = excluded.auth, label = excluded.label, fail_count = 0",
        params![user_id, endpoint, p256dh, auth, label, now()],
    )?;
    Ok(true)
}
```

`push_subs_for_platform` 的 WHERE 改為 `WHERE p.platform = ?1 AND u.notify_codes = 1 AND s.fail_count < ?2`，`params![platform, PUSH_MAX_FAILS]`。用 `grep -rn 'add_push_sub(' app/control/src` 更新其他呼叫點（含既有測試）補上 `max_per_user` 參數並處理 `bool`。

`main.rs` 在推送區塊加常數並改寫 `push_subscribe`：

```rust
/// 一個人實際上有幾台裝置。訂閱永久寫入 SQLite，沒有上限就是免費的磁碟。
const MAX_PUSH_SUBS_PER_USER: i64 = 8;
const MAX_ENDPOINT_LEN: usize = 2048;
/// 同時存在的推送 task 數（也就是同時在飛的連線數）。
const PUSH_FANOUT_CONCURRENCY: usize = 8;
/// 整批扇出的總 deadline。每個請求各有 10 秒 timeout，但分輪送時會疊加。
const PUSH_FANOUT_DEADLINE_SECS: u64 = 60;
```

```rust
async fn push_subscribe(
    State(st): State<Shared>,
    session: Session,
    Json(req): Json<SubscribeReq>,
) -> ApiResult<Json<serde_json::Value>> {
    let (uid, _) = require_user(&st, &session).await?;

    let endpoint = req.endpoint.trim();
    if !endpoint.starts_with("https://") || endpoint.len() > MAX_ENDPOINT_LEN {
        return Err(AppError(anyhow::anyhow!("推送 endpoint 必須是 https 且不超過 {MAX_ENDPOINT_LEN} 字元")));
    }
    let (p256dh, auth) = (req.p256dh.trim(), req.auth.trim());
    if !push::valid_keys(p256dh, auth) {
        return Err(AppError(anyhow::anyhow!("加密金鑰材料格式不正確")));
    }
    let label = req.label.as_deref().map(str::trim).filter(|s| !s.is_empty());
    if label.is_some_and(|l| l.chars().count() > MAX_LABEL_LEN) {
        return Err(AppError(anyhow::anyhow!("裝置名稱最多 {MAX_LABEL_LEN} 個字")));
    }

    if !db::add_push_sub(&st.db, &uid, endpoint, p256dh, auth, label, MAX_PUSH_SUBS_PER_USER)? {
        return Err(AppError(anyhow::anyhow!(
            "這個帳號的裝置訂閱已達上限（{MAX_PUSH_SUBS_PER_USER} 台），請先移除不用的"
        )));
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}
```

`fan_out` 改為（滿了就等一個做完再開下一個：限制的是**存在的 task 數**，不只是同時連線數，幾千筆訂閱不會先變成幾千個 task 排隊）：

```rust
async fn fan_out(st: &Shared, subs: Vec<db::PushSub>, n: &push::Notification) {
    let run = async {
        let mut set = tokio::task::JoinSet::new();
        for sub in subs {
            if set.len() >= PUSH_FANOUT_CONCURRENCY {
                set.join_next().await;
            }
            let (st, n) = (st.clone(), n.clone());
            set.spawn(async move {
                match st.push.send(&st.db, &sub, &n).await {
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
        set.join_all().await;
    };
    let deadline = std::time::Duration::from_secs(PUSH_FANOUT_DEADLINE_SECS);
    if tokio::time::timeout(deadline, run).await.is_err() {
        tracing::warn!("推送扇出超過 {PUSH_FANOUT_DEADLINE_SECS} 秒，剩餘請求已放棄");
    }
}
```

（`JoinSet` 隨 `run` 一起被 drop 時會 abort 未完成的 task，timeout 之後不會有殘留連線。）

- [ ] **步驟 4：執行測試以確認它們通過**

執行：`cd app/control && cargo test`
預期：全部通過；`fan_out_never_exceeds_the_concurrency_cap` 約 0.6–1.2 秒。

- [ ] **步驟 5：Commit**

```bash
git add app/control/src/push.rs app/control/src/db.rs app/control/src/main.rs
git commit -m "推播：每人 8 筆訂閱配額（接手他人 endpoint 也計入）、金鑰驗曲線、扇出限 8 個 task 與 60 秒 deadline"
```

### 任務 13：郵件清單改回摘要，全文改單封端點；前端 single-flight（發現 #6，medium）

**檔案：**
- 修改：`app/control/src/db.rs:911-933`（新增 `MailSummary`）、`:976-1002`（新增 `recent_mail_summaries`）
- 修改：`app/control/src/mail.rs:253-269`（`extract_links` 上限）、`:104-150`（先截斷再抽連結）
- 修改：`app/control/src/main.rs:1394-1439`（`mail_list`、`mail_inbox`）、新增 `mail_get`、`:2885-2887`（路由）
- 修改：`app/control/web/src/lib/api.js:72-75`、`screens/Home.svelte:62-79`、`screens/Codes.svelte:15-30`、`screens/admin/Inbox.svelte:48-50,109`
- 測試：`app/control/src/db.rs`、`mail.rs`、`main.rs` 的 `mod tests`

> [!NOTE]
> 摘要查詢仍會從 SQLite 讀 `body`（只用來跑關鍵字篩選，不序列化）。刻意不在
> ingest 時持久化 `actionable`：關鍵字與排除字改了要**立刻**生效是現有行為，
> 持久化就得在每次改設定時重算全部列。這個成本有界（60 列 × 最多 20k 字元、
> 本機 SQLite），真正被放大的是網路端，摘要 DTO 已經把那一段拿掉。

- [ ] **步驟 1：撰寫失敗的測試**

`db.rs` tests：

```rust
/// 首頁每 20 秒輪詢一次清單，清單裡不能有 body / html / links ——
/// 一封信最大 8 MiB，30 封就是每 20 秒幾百 MB。
#[test]
fn mail_summaries_carry_no_content_fields() {
    let db = test_db();
    insert_mail(&db, Some("a"), now(), None, None, Some("s"), None, Some("body"), Some("<b>h</b>"), &["https://x.example/y".into()], true, Some("netflix"), None).unwrap();
    let v = serde_json::to_value(recent_mail_summaries(&db, None, 10).unwrap()).unwrap();
    let row = &v[0];
    assert!(row.get("body").is_none());
    assert!(row.get("html").is_none());
    assert!(row.get("links").is_none());
    assert_eq!(row["subject"], "s");
}

/// 平台過濾要進 SQL：先取 60 封再過濾，別的平台塞滿前 60 封，
/// 成員自己的信就從清單消失。
#[test]
fn summaries_filter_by_platform_before_the_limit() {
    let db = test_db();
    for i in 0..70 {
        insert_mail(&db, Some(&format!("d{i}")), now(), None, None, None, None, None, None, &[], true, Some("disneyplus"), None).unwrap();
    }
    insert_mail(&db, Some("n"), now() - 100, None, None, Some("我的"), None, None, None, &[], true, Some("netflix"), None).unwrap();
    let mine = recent_mail_summaries(&db, Some(&["netflix".into()]), 60).unwrap();
    assert_eq!(mine.len(), 1);
    assert!(recent_mail_summaries(&db, Some(&[]), 60).unwrap().is_empty(), "沒有授權就什麼都看不到");
    assert_eq!(recent_mail_summaries(&db, None, 60).unwrap().len(), 60, "admin 不過濾");
}
```

`mail.rs` tests：

```rust
/// 一個 URL 沒有長度上限，一封信就能帶 8 MiB 進 links 欄位與清單回應。
#[test]
fn oversized_links_are_dropped() {
    let huge = format!("https://x.example/{}", "a".repeat(MAX_LINK_LEN));
    let text = format!("{huge} https://ok.example/path");
    assert_eq!(extract_links(&text), vec!["https://ok.example/path"]);
}
```

`main.rs` tests：

```rust
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
    let ids: std::collections::BTreeMap<String, i64> = db::recent_mails(&st.db, 10).unwrap()
        .into_iter().map(|m| (m.subject.unwrap(), m.id)).collect();

    let session = test_session();
    session.insert(S_USER, &"m".to_string()).await.unwrap();
    session.insert(S_NAME, &"m@x".to_string()).await.unwrap();
    let get = |id: i64| {
        let (st, s) = (st.clone(), session.clone());
        async move { mail_get(State(st), s, Path(id)).await.map_err(|e| e.0) }
    };

    get(ids["code"]).await.expect("自己平台、observe 模式：看得到");
    assert!(get(ids["新片上架"]).await.is_err(), "同平台但清單看不到的信，單封也看不到");
    assert!(get(ids["code"]).await.is_ok());
    db::set_setting(&st.db, db::keys::SENDER_MODE, "enforce", None).unwrap();
    assert!(get(ids["code"]).await.is_err(), "enforce 下未通過驗證的信不給看");
    // disneyplus 那封：主旨也是 code，但 BTreeMap 只留一個 key —— 用 id 直接查
    let d = db::recent_mails(&st.db, 10).unwrap().into_iter().find(|m| m.platform.as_deref() == Some("disneyplus")).unwrap().id;
    assert!(get(d).await.is_err(), "別的平台");
}
```

- [ ] **步驟 2：執行測試以確認它們失敗**

執行：`cd app/control && cargo test -- mail_summaries_carry summaries_filter_by_platform oversized_links mail_detail_reapplies`
預期：編譯錯誤。

- [ ] **步驟 3：撰寫實作（後端）**

`db.rs` 在 `Mail` 之後加：

```rust
/// 清單用的瘦身版：沒有 html / links，body 只用來跑篩選器、不序列化。
/// 全文只在點開原始信件時才以 `get_mail` 單封取得。
#[derive(Debug, Serialize)]
pub struct MailSummary {
    pub id: i64,
    pub received_at: i64,
    pub sender: Option<String>,
    pub recipient: Option<String>,
    pub subject: Option<String>,
    pub code: Option<String>,
    #[serde(skip_serializing)]
    pub body: Option<String>,
    pub verified: Option<bool>,
    pub platform: Option<String>,
    pub skip_reason: Option<String>,
    pub primary_link: Option<String>,
}

/// `platforms` = None 表示不過濾（管理收件匣）；Some 只取這些平台的信，
/// 過濾放進 SQL 而不是先取 N 封再過濾。
pub fn recent_mail_summaries(db: &Db, platforms: Option<&[String]>, limit: i64) -> Result<Vec<MailSummary>> {
    let conn = db.lock().unwrap();
    let json = platforms.map(|p| serde_json::to_string(p).unwrap_or_else(|_| "[]".into()));
    let mut stmt = conn.prepare(
        "SELECT id, received_at, sender, recipient, subject, code, body, verified,
                platform, skip_reason, links
         FROM mails
         WHERE (?2 IS NULL OR platform IN (SELECT value FROM json_each(?2)))
         ORDER BY ingested_at DESC, id DESC LIMIT ?1",
    )?;
    let rows = stmt.query_map(params![limit, json], |r| {
        let links: Vec<String> =
            serde_json::from_str(&r.get::<_, String>(10).unwrap_or_else(|_| "[]".into())).unwrap_or_default();
        Ok(MailSummary {
            id: r.get(0)?,
            received_at: r.get(1)?,
            sender: r.get(2)?,
            recipient: r.get(3)?,
            subject: r.get(4)?,
            code: r.get(5)?,
            body: r.get(6)?,
            verified: r.get::<_, Option<i64>>(7).unwrap_or(None).map(|v| v != 0),
            platform: r.get(8)?,
            skip_reason: r.get(9)?,
            primary_link: crate::mail::primary_link(&links),
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}
```

`mail.rs`：

```rust
/// 單一 URL 的上限。再長的不是要按的連結，是要塞進資料庫的酬載。
pub const MAX_LINK_LEN: usize = 2048;
```

`extract_links` 中 `if url.len() > 12 && !out.contains(&url)` 改為 `if url.len() > 12 && url.len() <= MAX_LINK_LEN && !out.contains(&url)`。`parse` 組 `Parsed` 前先截斷再抽連結：

```rust
    // 先截斷再抽連結：以前是對完整 body 抽，body 的 20k 上限對 links 沒有效果
    let body: String = body.chars().take(20_000).collect();
    let links = extract_links(&body);
```

`Parsed { … }` 內改成 `links,`、`body,`。

`main.rs`（`MailScope` 與 `mode_allows` 已在任務 5 定義）：

```rust
async fn mail_list(State(st): State<Shared>, session: Session) -> ApiResult<Json<Vec<db::MailSummary>>> {
    let (uid, _) = require_user(&st, &session).await?;
    let _ = db::purge_old_mails(&st.db, st.cfg.mail_keep_days);

    let scope = MailScope::load(&st, &uid)?;
    let mails = db::recent_mail_summaries(&st.db, Some(&scope.granted), 60)?
        .into_iter()
        .filter(|m| scope.allows(m.platform.as_deref(), m.verified, m.subject.as_deref(), m.body.as_deref(), m.code.is_some()))
        .take(30)
        .collect();
    Ok(Json(mails))
}

async fn mail_inbox(State(st): State<Shared>, session: Session) -> ApiResult<Json<Vec<db::MailSummary>>> {
    require_admin(&st, &session).await?;
    Ok(Json(db::recent_mail_summaries(&st.db, None, 60)?))
}

/// 全文的唯一出口。授權跟清單、刪除用同一個 MailScope。
async fn mail_get(State(st): State<Shared>, session: Session, Path(id): Path<i64>) -> ApiResult<Json<db::Mail>> {
    let (uid, _) = require_user(&st, &session).await?;
    let me = db::get_user(&st.db, &uid)?.context("帳號已不存在")?;
    let m = db::get_mail(&st.db, id)?.context("查無此信件")?;
    if !me.is_admin() && !MailScope::load(&st, &uid)?.allows_mail(&m) {
        // 不分辨「不存在」與「不是你的」：不給枚舉 id 的人存在性 oracle
        return Err(AppError(anyhow::anyhow!("查無此信件")));
    }
    Ok(Json(m))
}
```

路由：`.route("/api/mail/{id}", delete(mail_delete))` 改為 `.route("/api/mail/{id}", get(mail_get).delete(mail_delete))`。既有測試 `every_endpoint_accepts_the_method_the_frontend_sends` 若列舉路由方法，補上 `GET /api/mail/1`。

- [ ] **步驟 4：撰寫實作（前端）**

`api.js`：在 `mails:` 之後加 `mail: (id) => req(`/api/mail/${id}`),`。

`Home.svelte`、`Codes.svelte`：`load()` 改成 single-flight，`onview` 改成先取全文：

```js
  // 慢回應不能跟下一次 interval 疊加：上一次還沒回來就不再發
  let inflight = null
  function load() {
    if (inflight) return inflight
    inflight = api.mails()
      .then((r) => { mails = r })
      .catch(fail)
      .finally(() => { inflight = null })
    return inflight
  }

  // 清單只有摘要，全文點開時才拿
  async function view(m) {
    try { viewing = await api.mail(m.id) } catch (e) { fail(e) }
  }
```

樣板的 `onview={(m) => (viewing = m)}`／`onview={(x) => (viewing = x)}` 改為 `onview={view}`。`Inbox.svelte` 的 `onclick={() => (viewing = m)}` 改為 `onclick={() => view(m)}`，並加同樣的 `view` 函式。

- [ ] **步驟 5：執行測試並建置前端**

執行：`cd app/control && cargo test && cd web && bun run build`
預期：全部通過；build 無錯誤。

- [ ] **步驟 6：Commit**

```bash
git add app/control/src app/control/web/src
git commit -m "郵件清單改回摘要 DTO，全文走 GET /api/mail/{id}；連結設上限；前端輪詢 single-flight"
```

### 任務 14：未驗證郵件不得包成品牌按鈕；連結 host 必須落在平台網域（發現 #17，low）

**檔案：**
- 修改：`app/control/Cargo.toml`（加 `url = "2"`；它已是 `webauthn-rs` 的傳遞相依，不會多拉東西）
- 修改：`app/control/src/platforms.rs`（新增 `domains`）
- 修改：`app/control/src/main.rs`（`brand_link_allowed`、`strip_unbranded_links`；`mail_list`／`mail_get` 套用）
- 修改：`app/control/web/src/components/CodeCard.svelte:53-71`
- 修改：`docs/DECISIONS.md`（domain-set 清單只能放平台持有的網域）
- 測試：`app/control/src/main.rs`、`platforms.rs` 的 `mod tests`

- [ ] **步驟 1：撰寫失敗的測試**

`main.rs` tests：

```rust
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
}
```

`platforms.rs` tests（若該檔沒有 `mod tests` 就新建，用 `tempfile::tempdir()`）：

```rust
#[test]
fn domains_reads_the_list_skipping_comments() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("netflix.list"), "# platform-name: Netflix\n\nnetflix.com\nNFLXEXT.com\n").unwrap();
    assert_eq!(domains(dir.path().to_str().unwrap(), "netflix"), vec!["netflix.com", "nflxext.com"]);
    assert!(domains(dir.path().to_str().unwrap(), "nope").is_empty());
}
```

- [ ] **步驟 2：執行測試以確認它們失敗**

執行：`cd app/control && cargo test -- branded_button domains_reads_the_list`
預期：編譯錯誤。

- [ ] **步驟 3：撰寫實作**

`Cargo.toml` 的 `[dependencies]` 加 `url = "2"`。

`platforms.rs`：

```rust
/// 該平台清單檔裡的網域（跳過註解與空行、一律小寫）。給連結背書用：
/// 卡片只替落在這些網域下的連結畫品牌按鈕。
///
/// ⚠️ 因此清單裡只能放**平台自己持有**的網域（見 DECISIONS.md）。
pub fn domains(dir: &str, code: &str) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(format!("{dir}/{code}.list")) else {
        return Vec::new();
    };
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| l.to_lowercase())
        .collect()
}
```

`main.rs` 在 `MailScope` 之後加：

```rust
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
    domains.iter().any(|d| host == d || host.ends_with(&format!(".{d}")))
}

/// 對一批摘要套用 `brand_link_allowed`，不合格的把 `primary_link` 拿掉。
fn strip_unbranded_links(st: &Shared, mails: &mut [db::MailSummary]) {
    let mut cache: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for m in mails.iter_mut() {
        let Some(link) = m.primary_link.clone() else { continue };
        let code = m.platform.clone().unwrap_or_default();
        let domains = cache
            .entry(code.clone())
            .or_insert_with(|| platforms::domains(&st.cfg.domain_set_dir, &code));
        if !brand_link_allowed(m.verified, &link, domains) {
            m.primary_link = None;
        }
    }
}
```

`mail_list`：`let mails = …collect();` 改為 `let mut mails: Vec<_> = …collect(); strip_unbranded_links(&st, &mut mails);`。`mail_inbox` 同樣套用（admin 也不該看到假品牌按鈕）。`mail_get`：`let m` 改 `let mut m`，回傳前加：

```rust
    if let Some(link) = m.primary_link.clone() {
        let domains = platforms::domains(&st.cfg.domain_set_dir, m.platform.as_deref().unwrap_or(""));
        if !brand_link_allowed(m.verified, &link, &domains) {
            m.primary_link = None;
        }
    }
```

`CodeCard.svelte`：在 `<script>` 加

```js
  // 按鈕下方一定露出目的網域：品牌卡片是背書，使用者要看得到背書的是哪裡。
  // URL.hostname 給的是 punycode，同形字網域不會被畫成本尊。
  const host = (u) => { try { return new URL(u).hostname } catch { return u } }
```

`{:else if mail.primary_link}` 分支的說明段落改成：

```svelte
    <p class="mt-2 text-label leading-relaxed text-fg-muted text-pretty">
      會開啟 <span class="font-mono">{host(mail.primary_link)}</span>。這封信沒有直接附上號碼，要到平台的頁面取得。
    </p>
```

最後的 `{:else}` 分支改成：

```svelte
  {:else}
    <p class="mt-3 text-body text-fg-faint">
      {#if mail.verified !== true}
        這封信未通過寄件者驗證，連結不會顯示。請開原始信件自行判斷。
      {:else}
        這封信沒有可用的號碼或連結，請開原始信件查看。
      {/if}
    </p>
  {/if}
```

`docs/DECISIONS.md` 檔尾加：

```markdown
## domain-set 清單只能放平台自己持有的網域

`config/smartdns/domain-set/<平台>.list` 除了決定哪些網域被改寫到 proxy，現在也是
「驗證碼卡片替哪些連結畫品牌按鈕」的依據（`platforms::domains`）。放進第三方
CDN 或分析服務的網域，等於替它們背書。

→ 影片 CDN 之類的請照既有慣例放 `*-cdn.list.disabled`，不要混進主清單。
```

- [ ] **步驟 4：執行測試並建置前端**

執行：`cd app/control && cargo test && cd web && bun run build`
預期：全部通過；build 無錯誤。

- [ ] **步驟 5：Commit**

```bash
git add app/control/Cargo.toml app/control/Cargo.lock app/control/src app/control/web/src/components/CodeCard.svelte docs/DECISIONS.md
git commit -m "品牌取碼按鈕只給通過驗證且 host（url crate 判讀）在平台網域內的連結，卡片露出目的網域"
```

---

## 階段四：部署與開發環境

### 任務 15：Docker 在 systemd 層依賴防火牆 unit 成功（發現 #3，medium）

**檔案：**
- 建立：`deploy/docker.service.d/10-nfhh-firewall.conf`
- 修改：`docs/SETUP.md:111-127`（第 4 步）
- 修改：`deploy/nfhh-firewall.service:4-8`（註解）

沒有自動化測試能在 CI 內跑 systemd；驗證步驟在目標主機手動執行。

> [!CAUTION]
> 步驟 3 會停掉 Docker 並刪除正式的 nft 表幾十秒：所有樓層的 DNS／proxy 會斷，
> 而且刪表的瞬間 `:53` 若還開著就是 open resolver（所以順序是**先停 Docker 再刪表**）。
> 在維護時段做、事先告知家人、而且要有 SSH 以外的進入方式（主機 console），
> 因為 Docker 起不來時管理面板也不在。整段用 `trap` 包起來，任何一步失敗都會把
> 防火牆與 Docker 帶回來。

- [ ] **步驟 1：建立 drop-in**

`deploy/docker.service.d/10-nfhh-firewall.conf`：

```ini
# OTT Household — Docker 的資料平面（:53/:443/:853 host-network 容器）
# 只能在 nftables ACL 就位之後啟動。
#
# nfhh-firewall.service 原本只有 Before=docker.service 的排序關係：
# 它失敗時 systemd 照樣起 Docker，容器一綁埠、路由器的 port forward
# 就把 open resolver 與公開 SNI proxy 送上 Internet。
#
# Requires=  防火牆 unit 失敗或被停止，Docker 不啟動／一起停（fail-closed）
# ExecStartPre 再驗一次 `inet nfhh` 表真的存在 —— Requires 看的是 unit 狀態
#              （RemainAfterExit=yes 的 oneshot 在表被手動刪掉後仍是 active），
#              不是核心裡的規則
[Unit]
Requires=nfhh-firewall.service
After=nfhh-firewall.service

[Service]
ExecStartPre=/usr/sbin/nft list table inet nfhh
```

`deploy/nfhh-firewall.service` 的 `# 必須在 docker 之前就位` 註解下補一行：

```
# 反向的依賴（docker 需要本 unit 成功）在 deploy/docker.service.d/10-nfhh-firewall.conf
```

- [ ] **步驟 2：更新 SETUP.md 第 4 步**

安裝命令改為一併安裝 drop-in：

```bash
sudo cp /opt/nfhh/deploy/nfhh-*.{service,timer,path} /etc/systemd/system/ && sudo mkdir -p /etc/systemd/system/docker.service.d && sudo cp /opt/nfhh/deploy/docker.service.d/10-nfhh-firewall.conf /etc/systemd/system/docker.service.d/ && sudo systemctl daemon-reload && sudo systemctl enable --now nfhh-firewall.service nfhh-sync-ip.timer
```

Unit 表格加一列：

```
| `docker.service.d/10-nfhh-firewall.conf` | Docker 的 drop-in：`Requires=` 防火牆 unit，且啟動前確認 `inet nfhh` 表存在。防火牆載入失敗時 Docker **不會**啟動 |
```

表格後加一個 admonition：

```
> [!WARNING]
> 這是刻意的 fail-closed：nft 規則載入失敗時整套服務（含管理面板）都不會起來。
> 用 `systemctl status nfhh-firewall.service` 看原因、修好後
> `sudo systemctl restart nfhh-firewall.service && sudo systemctl start docker.service`
> （是 `restart` 不是 `start`：這個 unit 是 `RemainAfterExit=yes` 的 oneshot，
> 表被刪掉後它仍是 active，`start` 什麼都不會做）。
```

- [ ] **步驟 3：在主機驗證（維護時段）**

整段貼進同一個 shell：

```bash
set +e
trap 'sudo systemctl restart nfhh-firewall.service; sudo systemctl start docker.service; echo "已復原：$(systemctl is-active nfhh-firewall.service docker.service | tr "\n" " ")"' EXIT
sudo systemctl daemon-reload
echo "requires/after: $(systemctl show docker.service -p Requires -p After | grep -c nfhh-firewall)"   # 預期 2
sudo systemctl stop docker.service
sudo nft delete table inet nfhh
sudo systemctl start docker.service; echo "docker: $(systemctl is-active docker.service)"          # 預期 start 報錯、印出 failed 或 inactive
echo "public listeners: $(sudo ss -ltnp | grep -cE ':53 |:443 |:853 ')"                            # 預期 0
sudo systemctl restart nfhh-firewall.service && sudo nft list table inet nfhh >/dev/null && sudo systemctl start docker.service
systemctl is-active docker.service nfhh-firewall.service                                            # 預期兩行 active
trap - EXIT
```

- [ ] **步驟 4：Commit**

```bash
git add deploy/docker.service.d/10-nfhh-firewall.conf deploy/nfhh-firewall.service docs/SETUP.md
git commit -m "Docker 以 systemd drop-in 依賴 nfhh-firewall 成功並驗證 nft 表存在（fail-closed）"
```

### 任務 16：開發截圖流程移除 host network、root 與主機可寫掛載，釘選映像（發現 #14、#15，low）

**檔案：**
- 建立：`app/control/web/dev/shoot.sh`
- 修改：`app/control/web/dev/README.md`
- 修改：`app/control/web/dev/shoot.js:66`

- [ ] **步驟 1：shoot.js 的輸出目錄改由環境變數決定**

第 66 行 `await Bun.write(`/tmp/shots/${s.shot}.png`, …)` 改為：

```js
    await Bun.write(`${process.env.OUT ?? '/tmp/shots'}/${s.shot}.png`, Buffer.from(data, 'base64'))
```

（截圖是 host 端的 Bun 透過 CDP 收下來寫檔的；容器從頭到尾不需要寫主機目錄。）

- [ ] **步驟 2：建立 wrapper `dev/shoot.sh`**

```bash
#!/usr/bin/env bash
# 一次性無頭 Chromium + CDP 截圖。容器只把 CDP 發佈到 127.0.0.1、不是 root、
# 不掛主機目錄；正常結束或 Ctrl-C 都會把容器清掉（detached 的 --rm 不保證這點）。
#
# 用法：dev/shoot.sh <url> '<steps json>'    環境變數：MOCK（預設 dev/mock.js）、OUT（預設 /tmp/shots）
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"

# 釘 digest 而不是 tag：浮動 tag 的內容會變。這個值是本機 `docker image inspect`
# 對 zenika/alpine-chrome 印出的 RepoDigest；要升級就重新 pull、inspect、換掉這行。
IMAGE='zenika/alpine-chrome@sha256:47da877e5622528039625218d15d7e1ccae4e426c6cc7671d165837ea98aacc8'
OUT="${OUT:-/tmp/shots}"
mkdir -p "$OUT"

trap 'docker rm -f nfhh-shot >/dev/null 2>&1 || true' EXIT
docker run -d --rm --name nfhh-shot \
  -p 127.0.0.1:9333:9333 \
  --entrypoint chromium-browser "$IMAGE" \
  --headless --no-sandbox --disable-gpu --hide-scrollbars \
  --remote-debugging-port=9333 --remote-debugging-address=0.0.0.0 \
  --disable-dev-shm-usage about:blank >/dev/null

# 映像預設以 chrome 使用者執行；萬一哪天不是，寧可中止
uid="$(docker exec nfhh-shot id -u)"
[[ "$uid" != "0" ]] || { echo "容器以 root 執行，中止" >&2; exit 1; }

for _ in $(seq 1 50); do
  curl -fs http://127.0.0.1:9333/json/version >/dev/null 2>&1 && break
  sleep 0.2
done

MOCK="${MOCK:-$here/mock.js}" OUT="$OUT" bun "$here/shoot.js" "$@"
```

`chmod +x app/control/web/dev/shoot.sh`。

- [ ] **步驟 3：改寫 README 的用法段落**

````markdown
## 用法

```bash
cd app/control/web
dev/shoot.sh 'https://dnf.example.com/' '[
  {"dark":false,"goto":true,"shot":"home"},
  {"click":"白名單","shot":"allow"},
  {"click":"展開","shot":"allow-queries"}
]'
```

截圖在 `/tmp/shots/`（可用 `OUT=` 改）。容器由 `shoot.sh` 起、也由它清掉。

跟舊版的差別，每一條都是安全理由：

- **沒有 `--network host`**：CDP 只發佈到 `127.0.0.1:9333`。專案的 nftables 只攔 53/443/853，
  以前 9333 對整個 LAN／VPN 開著，任何連得到的人都能用 DevTools 讀寫這個瀏覽器。
  容器內的 `--remote-debugging-address=0.0.0.0` 是給 Docker 的 port publish 用的，出不了主機。
- **沒有 `--user root`、沒有 `-v /tmp/shots:/out`**：容器不需要寫任何主機目錄。
  以前腳本被複製進容器可寫的目錄再從那裡執行，一個被污染的映像可以改寫腳本，
  下一行 `bun` 就在你的帳號下跑它的程式碼。現在腳本從 repo 執行，容器碰不到。
  `shoot.sh` 會用容器內的 `id -u` 確認不是 root。
- **`@sha256:` 釘選**：浮動 tag 的內容會變。升級：`docker pull zenika/alpine-chrome:latest && docker image inspect --format '{{index .RepoDigests 0}}' zenika/alpine-chrome:latest`，把印出的值換進 `shoot.sh`。
- `--no-sandbox` 保留：這個映像的 Chromium 在 Docker 預設 seccomp 下沒有 sandbox 跑不起來
  （映像作者文件如此）。少了 host network 與主機掛載之後，它能影響的只剩容器自己。
- `trap … EXIT`：正常結束或中斷都會清掉容器，不留一個在聽的 9333。
````

- [ ] **步驟 4：驗證**

跑一次 README 的命令，中途在另一個 shell：

```bash
ss -ltn | grep 9333
```
預期：只有 `127.0.0.1:9333`，沒有 `0.0.0.0:9333` 或 `*:9333`。

```bash
docker exec nfhh-shot id -u; docker inspect nfhh-shot --format '{{.HostConfig.NetworkMode}} {{len .Mounts}}'
```
預期：第一行非 `0`（映像的 `chrome` 使用者）；第二行 `bridge 0`（或 `default 0`）。

命令結束後 `docker ps -a --filter name=nfhh-shot` 為空，`/tmp/shots/home.png` 等檔案存在。

- [ ] **步驟 5：Commit**

```bash
git add app/control/web/dev/README.md app/control/web/dev/shoot.js app/control/web/dev/shoot.sh
git commit -m "開發截圖：改用 shoot.sh 包裝，CDP 只綁 loopback、非 root、無主機掛載、映像釘 digest、trap 清理"
```

---

## 階段五：文件

### 任務 17：DECISIONS、CONTROL、README、.env.example

**檔案：**
- 修改：`docs/DECISIONS.md`（檔尾新增五節）
- 修改：`docs/CONTROL.md:516-569`（§8 API 一覽）與 §2、§5、§11 對應段落
- 修改：`README.md:278`（安全須知）
- 修改：`.env.example`（任務 7、11 已加的變數，這裡只確認）

- [ ] **步驟 1：DECISIONS.md 新增約束**

檔尾追加：

```markdown
## 登入與註冊的 session 狀態互斥，不能共用鍵

`login_start` 存 `S_LOGIN_USER`、`register_start` 存 `S_REG_USER`，任一流程開始
時 `clear_auth_flows` 清掉全部。以前共用 `S_REG_USER`：member 先啟動「新增
Passkey」、再對 admin 的 Email 啟動登入、最後提交註冊回應，新金鑰就寫進 admin 列。
`register_finish` 另外硬檢查「目標 = 目前登入者」（`check_registration_owner`）。

→ 別為了少一個鍵把兩條流程合回去；別把 `check_registration_owner` 拿掉。

## 信箱所有權證明綁在 session 上，不只是全域旗標

`email_otp.verified_at` 是全域的（以 Email 為鍵）。`register_start` 另外要求
`S_EMAIL_PROOF` 等於該信箱 —— 證明是**這個瀏覽器**做的。否則攻擊者只要知道
受邀 Email、等真正持有人驗證完，就能在自己的瀏覽器搶先建帳號。

## v6 遷移路徑已移除：帳號一定有 email，登入只認 email

補填 Email 的端點、`username` 登入退路與 `rename_owner` 都拿掉了。它們讓一個
只有舊 Passkey 的帳號可以自填任意信箱、搶走家人的身分與轉發控制。
`users.username` 欄位仍在（WebAuthn user handle 與歷史稽核的 actor），但不再
是登入識別。啟動時若發現沒有 email 的帳號會記 error 並列出 username：那種帳號仍可用可探索登入進來，但不能用 Email 登入、也對不上平台分權與轉發，只能刪掉重邀。

→ 別把 `find_user(username)` 加回登入流程。

## Worker 的 fail-open 只對「面板不可用」，不對「面板拒收」

ingest 回 401／422 = 拒收（永久）、5xx／逾時 = 不可用（暫時）。只有後者退回
FORWARD_MAP；前者只轉 FALLBACK_TO。以前所有失敗都是 400、都走 FORWARD_MAP，
寄一封讓解析器出錯的信就能繞過寄件者驗證直達家人信箱。

→ `mail_ingest` 不能改回 `AppError`（那是一律 400）。

## `received_at` 是寄件者說的，`ingested_at` 才是面板的時鐘

排序、分頁、保留期只看 `ingested_at`；`received_at` 夾在一年前到一小時後之間、
只做顯示。以前用 `received_at`：一封未來日期的信永遠排第一、永遠不被清。

## 只採信第一個 Authentication-Results，而它的可靠性來自 Cloudflare，不來自我們

`mail::parse` 只看最上面那個、authserv-id 等於 `NFHH_MAIL_AUTHSERV_ID` 的表頭。
這比以前「全部串起來看」嚴格，但仍假設收信端把自己的表頭放最前面。`authserv-id`
不是秘密。部署後要用任務 7 的 canary 證實；結果與日期記在這裡：

- （canary 日期）：（verified=false／true）

解析器已對 token 錨定、註解與引號字串免疫。唯一無法在解析器端處理的情況：收信端
把寄件者可控的欄位**不加引號**地回寫、而該欄位含原始 `;`（違反 RFC 8601 §2.2）——
任何以 `;` 切段的解析器都會多出一段。canary 的第二封信測的就是這件事。

→ canary 若失敗，程式端沒有更多可做的事；決策見任務 7 步驟 5。

## Worker 沒有面板設定時照 FORWARD_MAP 轉發是刻意的

`PANEL_ENDPOINT`／`PANEL_SECRET` 缺席是部署狀態，攻擊者改不了 Worker 的環境變數，
所以它不是攻擊面；而「Worker 先於面板部署」時它是唯一的轉發路徑。分類上它是
`unconfigured`，跟「面板拒收」（只轉 FALLBACK_TO）與「面板不可用」分開記錄。

## Docker 依賴 nfhh-firewall 成功：fail-closed 是刻意的

`deploy/docker.service.d/10-nfhh-firewall.conf` 讓 nft 載入失敗時 Docker 不啟動，
連管理面板一起不起來。代價是「規則檔寫錯就全停」；不這樣做的代價是
open resolver 上 Internet。跟 `nfhh-firewall.service` 刻意沒有 `ExecStop` 是同一個
判斷的兩面：**規則在的時候不要拿掉，規則不在的時候不要開埠。**
```

- [ ] **步驟 2：CONTROL.md**

§8 表格改動：

```
| POST | `/api/join/start` `/verify` | 公開；寄與核對 Email 驗證碼。start 每 IP 每 10 分鐘 10 次 |
| POST | `/api/mail/ingest` | 共用密鑰；回覆轉發名單。401 = 密鑰不符、422 = 信件解析失敗、5xx = 面板故障（Worker 只在 5xx／逾時走 FORWARD_MAP） |
| GET | `/api/mail` | 登入；**摘要**（無內文），經平台分權與顯示策略過濾 |
| GET | `/api/mail/{id}` | 登入；單封全文，授權同清單 |
| DELETE | `/api/mail/{id}` | 登入；member 限自己平台的信，`platform` 為空者限管理員 |
| POST | `/api/allow` | 登入；他人的 IP 只能延長到期，不改標籤與 TTL |
| GET POST | `/api/push/subs` | 登入；列出或新增自己的裝置訂閱，**每人 8 筆** |
```

§2「註冊」段落補一句：「所有認證流程開始時清除其他流程的 session 狀態；`register_finish` 檢查目標等於目前登入者。」§6「寄件者驗證」把「先以 `;` 切段再於段內比對」改寫成現況：只採信**第一個**、authserv-id 等於 `NFHH_MAIL_AUTHSERV_ID` 的 `Authentication-Results`；段內以 token 錨定比對 `dkim=pass`／`header.d=`，寄件者可控的 `smtp.mailfrom=` 等欄位夾帶的字串不算數。§5 補：「清單是摘要，全文走 `/api/mail/{id}`；品牌取碼按鈕只在寄件者通過驗證且連結 host 落在該平台網域時顯示。」§11 補配額。§9 設定補「可信寄件網域以 DB 為準、即時生效」。§10 資料庫補 `mails.ingested_at`（v12）與 audit 保留期。

- [ ] **步驟 3：README 安全須知**

`## 安全須知` 段落補一個 admonition：

```markdown
> [!WARNING]
> `deploy/docker.service.d/10-nfhh-firewall.conf` 讓 Docker 在 nft 規則載入失敗時**不啟動**。
> 這是刻意的：規則不在的時候，`:53` 開著就是 open resolver。
```

- [ ] **步驟 4：確認 .env.example**

```bash
grep -c 'NFHH_MAIL_AUTHSERV_ID\|NFHH_AUDIT_KEEP_DAYS\|NFHH_AUDIT_MAX_ROWS' .env.example
```
預期：`3`。

- [ ] **步驟 5：全套驗證與 Commit**

```bash
cd app/control && cargo test && cd web && bun run build && cd ../../cloudflare && bun test
```
預期：Rust 全部通過（181 + 約 30 個新測試）、前端 build 成功、Worker 4 pass。

```bash
git add docs/DECISIONS.md docs/CONTROL.md README.md .env.example
git commit -m "文件：記錄安全性修正帶來的約束與 API 變更"
```

---

## 部署與驗收

### 部署前（正式主機）

1. **備份 SQLite volume**（先停 control 再拷，避免拷到半寫入的頁與 WAL）：

```bash
docker compose stop control && docker run --rm -v smartdns_control-data:/data -v "$PWD/backup":/b alpine tar czf "/b/control-data-$(date +%Y%m%d-%H%M%S)-pre-security-fix.tar.gz" -C /data . && docker compose start control
```

2. **Hard gate：缺 email 的帳號必須為 0**（任務 3 移除了它們唯一的補救路徑）：

```bash
docker run --rm -v smartdns_control-data:/data:ro alpine sh -c 'apk add -q sqlite && sqlite3 /data/control.db "SELECT count(*) FROM users WHERE email IS NULL"'
```
印出非 `0` 就停止部署：先在面板刪掉那些帳號並重新邀請，或決定保留任務 3 之前的版本。

3. **不可逆清理的預覽**（新版第一次啟動就會刪這些列）：

```bash
docker run --rm -v smartdns_control-data:/data:ro alpine sh -c 'apk add -q sqlite && sqlite3 /data/control.db "SELECT '"'"'mails 逾期'"'"', count(*) FROM mails WHERE received_at < strftime('"'"'%s'"'"','"'"'now'"'"') - 14*86400; SELECT '"'"'mails 超量'"'"', max(0, count(*) - 2000) FROM mails; SELECT '"'"'audit 逾期'"'"', count(*) FROM audit WHERE at < strftime('"'"'%s'"'"','"'"'now'"'"') - 90*86400; SELECT '"'"'audit 超量'"'"', max(0, count(*) - 20000) FROM audit"'
```
數字就是會被刪的列數。不接受就先在 `.env` 調 `NFHH_MAIL_KEEP_DAYS`／`NFHH_AUDIT_KEEP_DAYS`／`NFHH_AUDIT_MAX_ROWS`。

4. `.env` 視需要加 `NFHH_MAIL_AUTHSERV_ID`（預設 `mx.cloudflare.net`，可省略）。

5. **復原演練**（一次就好）：用上面的備份檔在一個暫時 volume 還原並開得起來：

```bash
docker volume create nfhh-restore-test && docker run --rm -v nfhh-restore-test:/data -v "$PWD/backup":/b alpine sh -c 'tar xzf /b/control-data-*-pre-security-fix.tar.gz -C /data && ls -l /data' && docker volume rm nfhh-restore-test
```

### 部署順序

1. 先裝 systemd drop-in（任務 15）並在維護時段完成該任務的驗證。
2. 後端：

```bash
git checkout security-fix && docker compose up -d --build control
```
`./nfhh apply` 只重產設定並 reload smartdns／nginx，**不會**重建 control。

3. 確認：

```bash
docker compose ps control && docker compose logs --tail=50 control | grep -E '面板啟動於|沒有 email|error' ; curl -s http://127.0.0.1:8081/api/status | head -c 200; echo; curl -s -o /dev/null -w '%{http_code}\n' -X POST -H 'authorization: Bearer wrong' http://127.0.0.1:8081/api/mail/ingest
```
預期：`running`；日誌有「面板啟動於」、沒有「個帳號沒有 email」與 error；status 回 JSON；最後一行 `401`。

4. 記錄部署版本：`git rev-parse HEAD`。
5. Worker：把新的 `email-worker.js` 貼進 Cloudflare Dashboard。**後端先、Worker 後**（理由見任務 9 末尾）。
6. 任務 7 的 canary，結果記進 DECISIONS.md。
7. Mail smoke：從真實平台觸發一封驗證碼信，確認面板顯示、家人收到轉發、推播送達；再從面板刪掉一封不屬於自己平台的信（應回 `ok: false`）。

### 復原

- 程式：`git checkout master && docker compose up -d --build control`。v12 只新增欄位與索引，舊版程式在 v12 資料庫上照常運作（`migrate` 對高於已知的 `user_version` 什麼都不做）。
- 資料（只有在需要回到清理前的列時）：

```bash
docker compose stop control && docker run --rm -v smartdns_control-data:/data -v "$PWD/backup":/b alpine sh -c 'rm -rf /data/* && tar xzf /b/<備份檔名>.tar.gz -C /data' && docker compose start control
```

- Worker：貼回 `git show master:app/cloudflare/email-worker.js`。
- 防火牆 drop-in：`sudo rm /etc/systemd/system/docker.service.d/10-nfhh-firewall.conf && sudo systemctl daemon-reload`。

### 最終驗收清單

基準（master）：`cargo fmt --check` 有 220 處差異、`cargo clippy --all-targets` 有 9 個 warning。不整檔 fmt（會產生無關的大量差異）；要求的是**不變差**。

- [ ] `cd app/control && cargo test`：全部通過
- [ ] `cd app/control && cargo clippy --all-targets 2>&1 | grep -c '^warning: '`：不超過基準的 9（新程式碼零 warning）
- [ ] `cd app/control && cargo fmt --check -- src/ratelimit.rs`：新檔案本身要乾淨
- [ ] `cd app/control/web && bun run build`
- [ ] `cd app/cloudflare && bun test`：4 pass
- [ ] `docker compose config >/dev/null`（compose 語法與變數展開，含新的三個環境變數）
- [ ] `docker compose build control`（正式映像建得起來）
- [ ] 任務 10 的 `v12_backfill_clamps_pre_existing_future_dates` 通過
- [ ] 任務 15 的主機驗證、任務 7 的 canary、上面的 smoke 都有紀錄（日期、結果）
- [ ] `git log --oneline master..security-fix` 核對 17 個 commit 各對應一項發現
- [ ] 對 `security-fix` 跑 `/security-review` 回歸掃描；#4 的狀態依 canary 結果填寫，不得先宣告 17/17
