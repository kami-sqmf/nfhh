# 第二輪安全審查的修訂方案 + Email 驗證碼備援登入

> 2026-09-05 定案並實作完成（任務 A1/A2/B/C/D/E/F 與補強，commit 1a6dbb6..266e614）：admin 強制 Passkey；join／login 起手一律不明講帳號是否存在；匿名 session 15 分鐘（非 5）。對應 2026-09-05 的靜態審查報告（10 項：high 1、medium 6、low 3）。
> 報告釘在 `1dd812e7`（掃描工具的暫時 commit，不在 `security-fix` 的歷史上），
> 但引用的行號與目前 `4b8bef8` 一致，逐項都已對照原始碼重驗。

## 逐項判定

| # | 報告 | 判定 | 處置 | 成本 |
|---|---|---|---|---|
| 1 | 匿名登入起手可撐爆記憶體 session store | **成立**（`MemoryStore::load` 只過濾不刪除；預設 expiry 兩週；兩支 start 都沒過 `throttle_public`） | 限流 + 匿名 session 短壽命 + 會掃過期的有界 store | 中 |
| 2 | 刪成員 vs 進行中的 `allow_add` 競態 | **成立**，但窗口窄 | 併進 #5：檢查 + 額度 + upsert 放進同一個 transaction | 小 |
| 3 | 已登入者可無上限註冊 Passkey | **成立** | 每人上限（建議 10）；`add_credential` 用 `INSERT … SELECT … WHERE count < max` 原子化 | 小 |
| 4 | `./nfhh up/restart` 與 bootstrap 順序可在無防火牆時起資料平面 | **成立**（本機已裝 drop-in，剩餘風險小；但腳本本身仍是錯的） | `up`/`restart` 先 `sudo -n nft -t list table inet nfhh`；bootstrap 步驟 1 與 3 對調 | 小 |
| 5 | 併發 `allow_add` 可超額 | **成立** | 同 #2：`db::allow_add_atomic` 一個鎖、一個 transaction、回三態 | 小 |
| 6 | 推播 endpoint → 跟隨轉址的 SSRF | **成立**（reqwest 預設跟 10 次轉址） | `redirect(Policy::none())` + `https_only(true)`；訂閱時再擋 userinfo／非 443／IP 字面值 | 小 |
| 7 | 認證升級不換 session id（sibling 可植入） | **成立，前提在外部**（同父網域確有 music／Wolfram／Frigate） | 每次證明／登入寫入前 `session.cycle_id()`；cookie 改名 `__Host-nfhh_session` | 小 |
| 8 | `/api/login/start` 洩漏帳號存在 | **成立** | 路由隨 Email+Passkey 退路一起移除；新的驗證碼登入**與 `join_start` 一起**改成一致回應：「若這個信箱有帳號／有被邀請，驗證碼已寄出」，不寄也回同一句、同一個 cooldown；推翻 DECISIONS 原本「封閉系統換明確訊息」的決定並記下理由 | 小（隨第二部分） |
| 9 | 明文 DNS 上游（HiNet 168.95.x、nginx resolver） | **成立，接受風險** | 不改設定；在 DECISIONS 記下「HiNet 上游是為了台灣 CDN 導向與延遲；平台流量端對端 TLS，篡改只會變成連不上」。若之後要收，先拔 smartdns 的兩條 `server`，nginx 的 resolver 沒有 DoT 選項 | 零 |
| 10 | 登入後不回寫 Passkey 狀態（counter／backup flag） | **成立** | `Passkey::update_credential(&result)` 回 `Some(true)` 時連同 `last_used_at` 一起 UPDATE；不再 `let _ =` | 小 |

「Reviewed Surfaces」裡被審查者自行駁回的四項（全域稽核可見、成員任意 IP、成員刪信、白名單狀態布林）都對應 CONTROL.md 已寫明的產品決策，不必回應。

## 第二部分：把「登不進去？改用 Email 登入」換成 Email 驗證碼登入

### 現況

`Login.svelte` 的退路是 **Email + Passkey**（`/api/login/start` 用信箱查出憑證、`allowCredentials` 帶上去）。
它存在的理由是 webauthn-rs 0.5 註冊時送 `residentKey: "discouraged"`，怕有人的 Passkey 不可探索。
使用者確認家裡的憑證都可探索，這條路不再需要 —— 但仍要一條**不靠 Passkey** 的備援。

### 威脅模型會變，要先講清楚

`otp.rs` 檔頭目前寫的是「光有碼不能登入任何東西，登入永遠需要 Passkey」。
加了驗證碼登入之後，**誰控制家人的信箱，誰就進得了面板**（跟大多數消費級服務一樣）。
對家用系統這是可接受的取捨，但要做兩件事把影響縮小：

1. **驗證碼登入的 session 標記為弱認證**（`S_AUTH_VIA = "otp"`）。`require_admin` 多一道：
   只接受 Passkey 登入的 session。member 功能（授權 IP、看驗證碼）不受影響，
   admin 頁會提示「請改用 Passkey 登入」。改動只在一個函式。
2. 登入後首頁沿用既有的「建立備援 Passkey」提示（`passkey_count` 已有），文案改成
   「這台裝置還沒有 Passkey，建一把之後就不必再收驗證碼」。

### 流程

```
Login.svelte
  ├─ [使用 Passkey 登入]           （不變，主路徑）
  └─ [登不進去？改用 Email 驗證碼]  → authStep = 'loginemail'
        LoginEmail.svelte（沿用 Join.svelte 版型，mode='login'）
          POST /api/login/otp/start { email }
            ├─ 查無帳號 → 不寄、寫稽核，但回應跟有帳號**完全一樣**（ok + cooldown）
            └─ 有帳號   → 寄碼（沿用 otp::generate／mailer.send_code）
          畫面一律進到輸入碼頁，說明文字：「若這個信箱有帳號，驗證碼已寄出（10 分鐘內有效）」
        LoginCode.svelte（沿用 JoinCode.svelte，mode='login'）
          POST /api/login/otp/verify { email, code }
            ├─ Ok → cycle_id → S_USER/S_NAME/S_AUTH_VIA=otp → 登入
            └─ Wrong/Expired/TooMany → 同 join_verify 的訊息
```

### 後端

- **新增** `login_otp_start`、`login_otp_verify`（皆 `throttle_public`，跟 join 共用限流器）。
  `start` 的條件跟 `join_start` **相反**：帳號必須存在；沒有 mailer 時回同一句「尚未設定寄信服務」。
- **移除** `login_start`／`login_finish`、`S_AUTH`、`S_LOGIN_USER`、前端 `loginPasskey`，
  以及 CONTROL.md 那段「退路現在還不能拿掉」的 IMPORTANT。
- `email_otp` 加欄位 `purpose TEXT NOT NULL DEFAULT 'join'`（migration v10）：
  `put_otp`／`check_otp` 帶 purpose，登入碼不能拿去過 `register_start` 的信箱證明，反之亦然。
  同一個信箱只會在其中一種狀態（有帳號／沒帳號），共用 PK 沒有衝突。
- `join_start` 同步改成一致回應：沒被邀請、已有帳號、mailer 冷卻中都回 `{ ok, cooldown }`；
  唯一仍會報錯的是格式不對的信箱與限流。冷卻期內重按也回同一句（不寄）。
  `JoinCode` 的說明改成「若這個信箱有被邀請，驗證碼已寄出」。
- `verify` 成功後 `clear_otp`，並 `cycle_id()`（順手處理 #7）。
- 稽核：`login`（detail=`otp`）、`login_otp_sent`、`login_otp_locked`、`login_otp_no_account`。
- 測試：
  - 沒帳號的信箱寄不出碼、也不寫 `email_otp`。
  - join 的碼不能用來登入；login 的碼不能通過 `register_start`。
  - OTP session 進不了任何 `require_admin` 端點；Passkey session 可以。
  - verify 前後 session id 不同。
  - 兩支端點都被 `join_limiter` 擋。

### 前端

- `Join.svelte`／`JoinCode.svelte` 抽 `mode` prop（標題、說明、送出的 API、成功後的動作），
  或複製成 `LoginEmail.svelte`／`LoginCode.svelte`。傾向抽 prop：兩組畫面只差文案與 API。
- `Login.svelte`：退路按鈕文案改「登不進去？改用 Email 驗證碼登入」；`join_enabled=false` 時
  跟「用 Email 加入」一樣停用。移除 conditional UI 的 `$effect`（它掛的是 Email+Passkey 的輸入框）。
- `state.svelte.js`：`authStep` 加 `loginemail | logincode`；`joinEmail` 改名 `flowEmail`（兩個流程共用）。

### 文件

- `otp.rs` 檔頭與 CONTROL.md「登入的兩條路」重寫：可探索 Passkey（主）／Email 驗證碼（備援，弱認證，不能做 admin 操作）。
- DECISIONS 新增：「Email 驗證碼登入是備援，session 標弱；admin 一律要 Passkey」、
  「join／login 起手不再透露信箱是否登記或有帳號：兩支端點都對外開放且現在都會寄信，
  明確訊息的代價從『家人打錯字看不懂』變成『任何人可列舉家人的信箱』」。
  `join_start` 舊註解裡「刻意洩漏」那段一併改掉。

## 執行順序（建議）

| 階段 | 內容 | 涵蓋 |
|---|---|---|
| A | Email 驗證碼登入 + 移除 Email+Passkey 退路 + 所有匿名 start 過 `throttle_public` + `cycle_id` + cookie 改 `__Host-` | 第二部分、#7、#8、#1 的限流半邊 |
| B | 匿名 session 短壽命（start 時 `set_expiry(OnInactivity(5 min))`，登入成功改回預設）+ 會掃過期、有上限的 store | #1 |
| C | `db::allow_add_atomic`（存在 + 額度 + upsert 同 transaction）+ Passkey 每人上限 | #2、#5、#3 |
| D | 推播客戶端關轉址、只走 https；登入後回寫 Passkey 狀態 | #6、#10 |
| E | `nfhh` 腳本 preflight、bootstrap 順序；DECISIONS 記 #9 | #4、#9 |
| F | CONTROL／DECISIONS／README 同步 | 文件 |

### B 的 store 選項

| 選項 | 優點 | 缺點 |
|---|---|---|
| 自寫 `BoundedMemoryStore`（HashMap + 每 60 秒掃過期 + 硬上限，滿了先踢最快到期的） | 零新相依，約 60 行 | 要自己寫測試 |
| `tower-sessions-moka-store` | `max_capacity` 內建、有 TTL | 多一個相依；被灌爆時會踢掉正常人的 session（比 OOM 好） |
| `tower-sessions-sqlx-store`（SQLite） | 重啟不掉登入 | 拉進 sqlx，太重 |

傾向第一個：限流之後，同時存活的匿名 session 上限是 200 筆／10 分鐘，硬上限只是保險。

## 部署提醒

- cookie 改名會讓所有人登出一次；限流器是記憶體內、重啟歸零。
- 新路由要在 Cloudflare Tunnel 端看得到（走同一條 ingress，不需改 Dashboard）。
- 若要多一層防護：Cloudflare 免費方案有一條 Rate Limiting rule，可套在 `/api/login/*` 與 `/api/join/*`。
