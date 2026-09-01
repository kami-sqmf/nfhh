# CONTROL.md — 控制平面（Passkey 管理面板）

容器 `nfhh-control`，Rust + Axum + SQLite + WebAuthn，程式在 `app/control/`。

手機用 Passkey 登入後，一鍵把「目前所在網路的公網 IP」加進白名單；
另外集中顯示串流平台的同戶驗證碼，並決定那封信要轉發給哪些家人。

操作步驟見 [README.md](../README.md)，不可更動的約束見 [DECISIONS.md](DECISIONS.md)。

---

## 1. 存取路徑與信任邊界

```
[家人手機] ──HTTPS──> Cloudflare Tunnel ──> 127.0.0.1:8081 ─┐
                          設定 CF-Connecting-IP              │
                                                             ▼
[Email Worker] ──HTTPS + Bearer──> /api/mail/ingest ──> SQLite ──> nft set
```

面板**只綁 `127.0.0.1:8081`**，不對外開埠。`NFHH_BIND` 設成非 loopback 位址時程式拒絕啟動。

來源 IP 只從 `CF-Connecting-IP` 取，刻意不讀 `X-Forwarded-For`。該值決定要把哪個 IP
寫進防火牆，兩件事合起來才讓它可信 —— 理由見 DECISIONS.md。

工作階段存在記憶體（`MemoryStore`），**容器重啟後所有人需要重新登入**。
Cookie 為 `nfhh_session`，`Secure` + `HttpOnly`。

---

## 2. 帳號與認證

### 三種註冊情境

`/api/register/start` 只接受這三種，其餘一律拒絕（不開放自助註冊）：

| 情境 | 條件 | 產生的角色 |
|---|---|---|
| 建立第一個帳號 | 系統還沒有任何帳號 + 一次性碼 + Email | `admin` |
| 家人建立帳號 | 位址已被 admin 登記 + **剛通過 Email 驗證碼或邀請連結** | `member` |
| 加註備援裝置 | 已登入本人，不帶任何憑據 | 不變 |

**一次性碼**只在系統完全沒有帳號時發放，印在容器啟動日誌：

```bash
docker logs nfhh-control 2>&1 | grep -A2 一次性
```

第一個帳號**刻意不要求 Email 驗證碼**：面板還沒跑過時寄信服務未必設好，
要求信件送達才能建第一個帳號是個死結。看得到容器日誌的人已經有主機權限。

**登記邀請 Email**（v6 起，取代了原本的邀請碼連結）。admin 在成員管理頁登記
一個位址，對方到登入頁點「用 Email 加入」→ 輸入**完全相同**的位址 →
系統寄一組 6 位數 → 通過後才建 Passkey。

v9 起登記時**順帶寄一封邀請函**（Resend 樣板，見下節）。信裡的連結是
`/join/<token>`，按下去等於「這個信箱是我的」已經證明完畢 —— 前端直接跳到
建立 Passkey 那一步，不必再輸入一次位址、也不必等驗證碼。兩條路徑接的是
**同一道關卡**：兌換連結做的事就是把信箱標成剛剛驗證過（`email_otp.verified_at`），
`register_start` 讀的還是那個旗標，時窗一樣是 15 分鐘。

寄信失敗不會讓登記失敗 —— 位址已經生效，`POST /api/invite` 回 200 帶
`sent: false` 與原因，畫面上照樣給得出連結讓 admin 自己傳。

⚠️ **授權必須排在建立帳號之後。** `user_platforms.user_id` 有外鍵指向 `users`，
順序反了會被 FK 擋下 —— 而 `register_finish` 曾經把授權迴圈寫在 `create_user`
之前、還用 `let _ =` 吞掉錯誤，結果是登記時選好的平台**靜默地一個都沒授權**，
家人註冊完看到空的驗證碼分頁、admin 在成員頁看到平台全是虛線。現在兩件事
綁在 `db::create_user_with_platforms` 一支函式的同一個交易裡，順序不可能再寫反。

v7 起**登記時就選好平台**，註冊完成的那一刻生效。v6 的流程是「登記 → 對方
註冊 → admin 再回去開平台」，中間那段空窗讓家人註冊完看到的是空的驗證碼分頁。
重新登記同一個位址 = 改這筆登記，平台會被覆寫。

為什麼 v6 換掉連結：舊的邀請碼連結**不綁位址**，轉傳出去等於任何人都能拿它
建一個帳號。v9 的邀請連結沒有這個問題 —— 它只對應那一筆登記，用它建出來的
帳號永遠是那個信箱的，轉傳給別人等於把自己的位址送人，而不是多一個名額。
其餘性質跟登記本身一致：單次使用（`used_at` 一落就死）、admin 撤銷或
重新登記時當場失效、**刻意不過期**（家人可能隔幾個月才想起來）。

成員管理上那份清單叫「等待註冊的邀請」，**已撤銷與已註冊的都不列出來**：
它回答的是「還有誰沒進來」，而那兩種狀態都沒有回答它。已撤銷的那筆撤完就
沒有任何功能（連結已死、註冊擋著、UI 給不出動作）；已註冊的那個人就在上面
的成員清單裡，那邊連平台授權都顯示得更準確。留著只會讓清單無限累積死資料，
而且**沒有任何地方清得掉**。要查歷史看稽核：**稽核才是歷史，這張表是現況**。

⚠️ 濾掉是**查詢層**的事，列本身要留著：`used_at` 是「這個位址已經換過帳號」
的閘門，刪了就擋不住第二次註冊（釘在 `a_used_invite_still_blocks_a_second_registration`）。
重新登記會讓撤銷過的那筆原地復活（`ON CONFLICT ... SET revoked_at = NULL`），
移除成員時 `delete_user` 才會把那列真的刪掉。

權杖是 256 bit 的 CSPRNG 輸出，DB 只存 HMAC-SHA256（金鑰 `invite_hmac_key`，
與驗證碼那把分開）。因此**連結只在登記完成的那一刻拿得到一次**，之後要重發
只能重新登記換一條新的。細節在 `src/invite.rs`。

### 邀請函樣板（Resend）

信的版面在 Resend dashboard 上編輯（別名預設 `ott-share-invitation`），面板只送
`template: { id, variables }`。**不能同時帶 `subject` / `html` / `text`**，帶了 API 會回
validation error；主旨由樣板決定。

| 變數 | 內容 |
|---|---|
| `INVITE_LINK` | `{NFHH_ORIGIN}/join/<token>`，「建立帳號」那顆按鈕的 href |
| `Platform` | 可用服務的顯示名，如 `Netflix、Disney+`。沒選平台時是「尚未指定（管理員稍後開通）」 |
| `TARGET_EMAIL` | 收件位址，信上「你的信箱」那一欄 |

改文案不必動程式；**改變數名要同步改 `mailer.rs`** —— 對不上時 Resend 不會報錯，
只會寄出一封有空白的信，所以那三個名字被 `invite_payload_matches_the_template_contract`
釘著。樣板存不存在、發布了沒，只有真的寄一次才知道：`cargo test -- --ignored smoke_send_invite`。

驗證碼的細節在 `src/otp.rs`：存 HMAC-SHA256 而非明碼（金鑰在首次使用時
產生並存進 `settings`）、碼綁著信箱一起簽、10 分鐘失效、錯 5 次鎖住、
重寄冷卻 60 秒、重寄會讓舊碼當場失效。

一次性碼與登記位址都在註冊流程的 **finish 階段才消耗**，不是 start。
消耗用 `UPDATE ... WHERE used_at IS NULL`，兩人同時送出時只有一個成功。

### 登入的兩條路

| 路徑 | 端點 | 用途 |
|---|---|---|
| 可探索憑證 | `/api/login/any/start` `/finish` | 不必輸入信箱，裝置自己挑一把 |
| 信箱 + passkey | `/api/login/start` `/finish` | 退路 |

⚠️ **退路現在還不能拿掉。** `start_passkey_registration` 送出的是
`residentKey: "discouraged"`（webauthn-rs 0.5 寫死，高階 API 沒有非 attested
的 resident key 入口）。iOS／Android／Chrome 的密碼管理器實務上仍會存成可探索
的，所以第一條路對它們有效；但硬體金鑰或設定較嚴格的認證器可能不會。

前端的主按鈕走可探索登入（跳系統選擇器），展開退路時額外掛 conditional UI
（把 passkey 掛進輸入框的自動填入）。兩者共用同一個後端挑戰，差別只在
`navigator.credentials.get()` 有沒有帶 `mediation: 'conditional'`。

### 角色

| 角色 | 權限 |
|---|---|
| `admin` | 全部。登記邀請 Email、升降角色、指派平台、改設定、移除任何人的白名單 |
| `member` | 授權自己所在的網路；**只能移除／改名自己新增的白名單項目** |

### 一封信會不會出現在驗證碼分頁

三層過濾，順序不能顛倒：

1. **平台分權** —— 沒有這個平台的人看不到。認不出平台的（`platform` 為 NULL）
   誰都看不到，只留在管理收件匣。
2. **可用性** —— 有抽到碼、**或**命中「驗證碼篩選器」的關鍵字，且不命中排除字。
3. **顯示策略** —— `sender_verify_mode` 決定未通過寄件者驗證的信要不要顯示。

⚠️ **排除字只比對主旨，關鍵字才比對主旨＋內文。** 這個不對稱是踩過才知道的：
排除字設了「同戶」，而暫時存取碼信的內文正好在解釋規則時寫著「此代碼僅限⋯⋯
在 Netflix 同戶裝置以外的裝置暫時使用」—— 最該顯示的那封信被自己的說明文字
擋掉了。排除是**排他**條件，命中一次就永遠看不到；關鍵字是**包含**條件，
多命中一封只是多顯示一封。附帶好處是 Worker 不解析 MIME、本來就只看得到主旨，
兩邊的排除判斷因此天生一致。

第 2 層的「或」是關鍵。Netflix 的**暫時存取碼**信裡沒有數字 —— 碼在信中
「取得存取碼」那顆按鈕後面（`/account/travel/verify?nftoken=…`）。只用
「抽得到碼」當條件的話，**這個專案存在的理由那封信會對家人完全隱藏**。

抽不到碼的信，卡片上會把**主要連結**渲染成一顆「取得存取碼」按鈕。主要連結
＝ 第一個非頁尾樣板的連結：行動呼籲一定排在頁尾之前，而頁尾佔了連結數量的
大半（實測一封暫時存取碼信有 10 個連結，其中 8 個是 help / 條款 / 隱私 / 瀏覽）。

⚠️ 關鍵字要選得夠窄。「有新裝置正在使用您的帳戶」與新片宣傳信也都有連結，
靠關鍵字才分得開 —— 拿「有沒有連結」當條件會把整個收件匣倒進驗證碼分頁。

### 一封信屬於哪個平台

判定順序，**寄件者優先於收件信箱**：

1. **寄件者位址對應**（設定頁的「平台寄件者位址」，存在 `settings` 的
   `platform_senders`）。樣式含 `@` 比對完整位址；只給網域則比對網域**含子網域**，
   並認 `.` 邊界，所以 `evil-netflix.com` 不會命中 `netflix.com`。
2. **admin 明說的收件信箱對應**（`platform_mailboxes`，在「轉發收件人」頁設定）。
3. **收件信箱的 local part** —— `netflix@share.example.com` → `netflix`。
4. 都認不出就是 `NULL`：那封信只會出現在管理收件匣，**不會外流到任何人的
   驗證碼分頁**。

⚠️ **第 2 層是後來補的，因為第 3 層的約定並不普遍成立。** 平台代號來自
domain-set 的檔名（`disneyplus.list` → `disneyplus`），而實際收件信箱是
`disney@` —— local part 推導對 Netflix 剛好成立，對 Disney+ 就是推不出來。
沒有第 2 層時那封信只能靠寄件者對應救，寄件者也認不出的話就變成誰都看不到。

為什麼寄件者優先：位址對應是管理員明確設定的，信箱只是路由意圖。而且用同一個
catch-all 收全部時，信箱根本推不出東西 —— 那正是這個順序存在的理由。

設定頁另外列出「已收到、但認不出平台的寄件位址」及其封數，點一下就能指派 ——
否則管理員得先去收件匣一封封看才知道要填什麼。存檔後會**重新判定 `platform`
為 NULL 的信件**；已經歸屬的不動，改對應不該讓一封信從某個人的驗證碼分頁
憑空消失。

**平台分權**（`user_platforms`）決定誰看得到哪個平台的驗證碼。
⚠️ 它只作用在面板層 —— 網路層（nft set 與 smartdns 的 domain-set）依然是
全有全無，一個 IP 進了白名單，所有平台的網域都會解到 proxy。理由與已知後果
見 [DECISIONS.md](DECISIONS.md)。

**每個需要登入的動作都會回 DB 確認帳號還在**（`require_user`），角色也是每次
重讀，不信任 session 快取 —— 降權與移除都即時生效，不必等對方重新登入。

⚠️ 這件事以前**只有 `require_admin` 做**。session 存在記憶體（`MemoryStore`），
帳號被刪掉之後那份 session 還活得好好的，所以被移除的成員仍能授權 IP、
看驗證碼，直到容器下次重啟為止 —— 那正是「移除成員」要阻止的事。
釘在 `a_deleted_member_loses_access_immediately`。

### Passkey 管理

帳號頁（首頁右上的信箱）可以列出自己的 passkey、重新命名、撤銷。
註冊時會帶一個裝置名（由 userAgent 猜，可改）—— 三個月後回來看
「哪一把是我弄丟的那台」時，那個名字是唯一的線索。

一律只能操作**自己的**。admin 可以降權某個成員、看他加了哪些 IP，
但碰不到別人的憑證：那是登入手段本身，不是設定。刪除的 WHERE 帶上
`user_id` 而不只是 `id` —— credential id 會出現在登入回應裡，不是機密。

⚠️ **撤銷最後一把會被擋下**（後端與 UI 各擋一次）。這個系統沒有密碼、
沒有信箱救援可以繞過 Passkey，刪光了就永遠登不進來，而且沒有任何介面
能救。剩最後一把時要換裝置的正確順序是：**先在新裝置註冊，再撤銷舊的**。

### 移除成員

`DELETE /api/members/{id}`，三道護欄：

1. **不能刪自己** —— 會在下一次請求時被登出，而且多半是誤按。
2. **不能刪掉最後一個 admin** —— 跟降權同一個理由。
3. 對方**新增的白名單一併移除**並立刻 `nft::sync` —— 移除一個人卻留著他
   授權的網路，等於沒有移除。

### 到底帶走了什麼

這張表就是「移除成員」的完整定義。少刪任何一項，那個人就還留著某種形式的
存取或收件能力 —— 整張表釘在 `removing_a_member_clears_every_trace_except_the_audit_log`。

| 資料 | 怎麼走的 | 為什麼 |
|---|---|---|
| `users` | 直接刪 | — |
| `credentials`（Passkey） | 外鍵 CASCADE | 登入手段 |
| `user_platforms` | 外鍵 CASCADE | 看得到哪些平台 |
| `push_subscriptions` | 外鍵 CASCADE | 不然驗證碼會繼續推到他手機上 |
| `allowlist` | 手動刪（比對 `added_by`） | 留著等於那些網路還能用 |
| **`mail_recipients`** | 手動刪（比對 `address`） | 留著等於**他永遠繼續收到驗證碼** |
| `invited_emails` | 手動刪（比對 `used_by`） | 不刪會永遠卡在「已使用」，位址再也註冊不了 |
| `audit` | **保留** | 那是歷史，人走了不代表做過的事沒發生過 |

⚠️ 轉發那一項是最容易漏、後果也最久的。白名單有 TTL 會自己過期，
轉發不會 —— 而且面板上完全看不出來那個位址屬於一個已經被移除的人。

⚠️ **Cloudflare 上的目的地位址刻意不刪。** 它是帳戶層級的共用資源，
可能還有別的路由規則在用，而且刪掉已驗證的位址不可逆（要對方重新點一次
驗證信）。面板這邊不轉了就夠了。

⚠️ **Worker 的 `FORWARD_MAP` 要自己去拿掉。** 那是面板停機時生效的那份
名單，面板碰不到它 —— 不拿掉的話，面板一停機，已經被移除的人又會開始收到碼。
移除的確認對話框會提醒這件事。

`added_by` 存的是**顯示名稱**而不是 user_id（v1 就留下的形狀）。舊帳號補填
email 時 `rename_owner` 會把它對齊，所以刪除比對得上 —— 這也是為什麼那支
函式不能拿掉。

稽核紀錄刻意**不動**：那是歷史，人走了不代表做過的事沒發生過。

### 裝置遺失

只有一把 passkey 時裝置遺失就再也登不進來，面板偵測到會主動提示註冊備援。
全部遺失時的救援：停容器，清掉 `control-data` volume 內 `users` / `credentials`
兩張表的內容，重啟後會重新發一次性碼。白名單與信件資料不受影響。

⚠️ `NFHH_RP_ID` 不能事後改，改網域等於所有 passkey 作廢。

---

## 2.5 轉發決策：面板決定、Worker 執行

Worker 收到信後**先** POST `/api/mail/ingest`（5 秒逾時），面板解析完 MIME、
跑完寄件者驗證與驗證碼篩選器之後，在回應裡給一份 `forward_to`，Worker 才照著
轉發。順序跟 v6 之前相反 —— 那時是先轉發再推送。

為什麼改：篩選器的**關鍵字要比對內文**，而 Worker 不解析 MIME，拿得到的只有
主旨。判斷留在 Worker 就只能做排除字，`code_keywords` 對轉發永遠沒有效果。
交給面板之後，家人收到的信與驗證碼分頁顯示的才是同一套判準（同一支
`mail::is_actionable`）。

為什麼不乾脆由面板寄信：`message.forward()` 是 Email Routing 綁在 Worker 上的
能力，面板沒有寄信管道。`mailer` 那支是 Resend，給邀請信用的 —— 拿交易型 API
重寄原始 MIME 會破壞 DKIM 對齊、附件與表頭。

`forward_to` 為空有兩種成因，靠回應裡的 `verified` / `actionable` 分辨：
未通過寄件者驗證且 `forward_enforce_sender` 為 `"1"`，或被篩選器擋下。
**兩種情況 Worker 都會無條件補上 `FALLBACK_TO`** —— 家人不會收到，但管理員
一定收得到，篩選器設錯才看得見。

⚠️ **面板掛掉絕不能讓信轉不出去。** 推送失敗（逾時／DNS／面板停機／非 2xx）時
Worker 退回自己的環境變數，照原本的行為送。驗證碼有時效，寧可設定舊一點，
也不能不轉。代價是那封信**不會有面板紀錄**，Worker 日誌裡的
「⚠️ 面板無回應」是唯一信號。

⚠️ **請保持 `FORWARD_MAP` 與「轉發收件人」頁同步。** 它是面板停機時唯一的
退路，清空等於面板一掛家人就收不到碼。平常不會被讀到、壞掉也沒人發現，
直到停機那天 —— 移除某位家人時記得兩邊都要拿掉。

Fallback 路徑上**完全不做過濾** —— 沒有 MIME 可解析，篩選器套不了，
寄件者驗證也刻意不做。面板不在的時候資訊最少，正是最不該自作主張的時候：
少轉幾封廣告 vs 漏掉一組碼，代價不對等。

這代表面板停機期間，偽造寄件者的信會被轉給家人。可接受的理由是那段時間很短、
而且信是以轉寄的形式進家人信箱（釣魚仍要騙過收件人），拿它去換「停機時一定
收得到碼」是划算的。真正的寄件者驗證在面板 (`mail::SenderAuth`)，那裡的判定
還會寫進管理收件匣供事後追查。

## 3. 白名單管理

面板的核心功能。SQLite 是唯一真實來源，nft set 只是它的投影。

### 授權對象一律是那一戶的 IPv4

面板走 Cloudflare Tunnel，後端看到的 `cf-connecting-ip` 是**開面板那台裝置**
連到 Cloudflare 用的位址。手機開著 IPv6 時那是一個 /128，拿它當「這個網路的
出口」是錯的 —— 閘門 (`config/nft/nfhh.nft`) 的 `clients_v4` / `clients_v6`
都沒有 `flags interval`，比對的是**單一位址**：

- 同一戶的電視照 §4 把 DNS 填成出口 IP，走的是 IPv4，來源是那戶的 WAN IPv4，
  不在 `clients_v4` 裡 → 照樣被 drop
- IPv6 沒有 NAT，一個位址只等於一台裝置的一張介面
- SLAAC 臨時位址會定期輪替，過幾天連授權的那台自己都會失效

IPv4 因為 NAT，一筆就代表整戶。所以前端在**瀏覽器端**向只有 A 記錄的服務問出
公網 IPv4（`web/src/lib/ip.js`，整頁只問一次），帶進 `GET /api/status?ip=`
與 `POST /api/allow` 的 `ip`。後端只採信公網位址，而且**只在登入後採信** ——
否則 `/api/status` 會變成「某個 IP 在不在白名單裡」的探測器，那支不需要登入。

問不到 IPv4 時退回連線來源，授權彈層會明說那只涵蓋這一台裝置。稽核裡
`detail` 記的是被授權的 IP、`client_ip` 欄記的是連線來源，兩者不同是正常的。

### 授權一個網路

`POST /api/allow`，body 可帶 `ip`（面板一律帶著，見上一節；省略則用呼叫端的
連線位址）、`label`、`ttl_days`。

檢查順序：

1. **必須是公網位址** —— 私有、loopback、link-local、CGNAT `100.64/10`、
   ULA `fc00::/7`、`fe80::/10` 一律拒絕。加進去不會有效果，擋掉才不會讓人
   誤加自己手機的 `192.168.x` 就以為設定好了
2. **每人額度 `NFHH_MAX_PER_USER`**（預設 4）。已在自己名下的同一 IP 是
   「延長授權」，不佔新額度。v6 起沒有全域上限 —— 濫用防護改由
   「每人 4 條 × 成員數」與 admin 的 Email 登記共同構成
3. **TTL** 取 `ttl_days`，預設 `NFHH_TTL_DAYS`（7），夾在 1～30 天。
   這個天數**存在條目上**（`allowlist.ttl_days`），自動續期時才知道要延多久

寫入 DB 後立刻 `nft::sync()`，並寫一筆稽核。

### 自動續期

背景任務每 10 分鐘檢查一次：條目剩不到一天、而且在查詢視窗內有活動，
就延長它自己的 `ttl_days` 並記 `renewed_at`。完全沒有查詢的照常到期並移除。

活躍度來自 smartdns 的稽核檔（`audit-enable yes`），由控制平面持續 tail
餵進一個記憶體滾動視窗（`src/dnslog.rs`）。視窗保留 30 分鐘，重啟後歸零 ——
最差的後果是續期判斷晚一輪生效。

⚠️ **查詢明細的可見性是分級的**：彙總數字（幾筆、最後一次何時）在白名單列表上
人人可見；逐筆網域只有**該條目的擁有者**拿得到，admin 也不例外。
這是刻意的，見 [DECISIONS.md](DECISIONS.md)。

### 同步機制

`nft::sync()` 每次都是**整個 set 重建**（`flush` 後全量寫入），不是增量增刪，
所以冪等、可自我修復。同時維護兩處：

- 執行中的 nft set（`inet nfhh clients_v4` / `clients_v6`），單一 `nft -f -` 交易套用
- `generated/nft/clients.nft`，供 `nfhh-firewall.service` 開機載入

觸發時機：每次白名單變更、啟動時、以及**背景每 5 分鐘**一次（順帶清除過期條目）。

`expires_at` 存絕對時間戳，寫進 nft 時才換算成剩餘秒數的 timeout。

### 首次上線的遷移

`nft::import_legacy()` 會把 `clients.nft` 內既有的條目收進 DB，只在 DB 完全沒有
白名單資料時執行 —— 否則首次同步的 flush 會清掉面板上線前手動加的條目。

---

## 4. 連線教學

面板內建四個分頁，會自動帶入目前的網域名與出口 IP，並在該網路尚未授權時提示。

| 分頁 | 內容 |
|---|---|
| Android | 設定 → 網路 → 私人 DNS → 指定主機名稱 → `dns.example.com` |
| iPhone / iPad | 下載 `.mobileconfig` 描述檔 |
| 檢查 | 引導開 `https://ifconfig.me` 確認出口 IP |
| 電視 | DNS 手動填出口 IP（重撥後需重設） |

### iOS 描述檔（`GET /api/dns-profile`）

需登入。內容只有一個主機名、沒有機密，要求登入是避免未授權的人拿去指向這台伺服器。

三個實作重點：

- **刻意不填 `ServerAddresses`** —— 留空時 iOS 會用網路提供的 DNS 去解析
  `ServerName`。填了等於又把 IP 寫死，動態 IP 一變就失效
- **UUID 固定** —— iOS 以 `PayloadIdentifier` + UUID 判斷是否為同一份描述檔，
  每次給新的會變成重複安裝而不是取代
- **前端用 `location.href` 而非 `fetch`** —— iOS 必須由 Safari 直接接收回應才會
  跳出安裝提示。**必須用 Safari 開面板**

面板依 `dot_ready`（實際檢查 smartdns 載入的設定有無 `bind-tls`）決定是否開放下載。

---

## 5. 驗證碼集中顯示

```
平台寄信 → Cloudflare Email Routing → Email Worker
                                          │
                                          ├─1─→ POST /api/mail/ingest ──→ 回 forward_to
                                          └─2─→ 依 forward_to 轉發給家人
```

### 接收端點

`POST /api/mail/ingest`，收原始 MIME（body 上限放寬到 8 MB，其餘路由維持 2 MB）。

- 認證用 `Authorization: Bearer <NFHH_MAIL_SECRET>`，機器對機器，Worker 做不了 WebAuthn
- 比對是**定時的**（逐位元組 XOR 累加，不提早跳出）
- **密鑰未設定時整個端點停用** —— 能寫進去的人就能在面板顯示假驗證碼騙家人
- `message_id` 有 UNIQUE 約束，Worker 重送不會產生重複

MIME 解析全在面板用 Rust 做（`mail.rs`），Worker 因此可以是一段不需建置的純 JS。

### 驗證碼抽取

規則拿真實信件調出來，`text/plain`、HTML、主旨三處都搜。核心規則：

- 必須有提示字眼（code / 驗證碼 / passcode / OTP…）在附近，取距離最近的
- **沒有提示字眼就不猜**
- 前後緊鄰 `- _ & # ; = / +` → 是更長識別碼的一部分（UUID、HTML 實體），排除。
  刻意不用空白切詞：中文「您的驗證碼：123456」整句沒有空格
- 後面接 年/月/日/%/元/px → 是數量不是碼
- 四位數且落在 1900–2100 → 提示字眼的距離門檻收緊
- HTML 數字實體（`&#8199;`）先解碼，否則會製造假候選

**規則改動會自動套用到既有信件** —— 面板每次啟動用當前規則重跑一遍全部已存信件。

### 原始信件檢視

保留原始 HTML 供檢視，但那是外部內容，兩層防護：

1. `sandbox=""` 的 iframe —— 空值代表全部限制生效（獨立來源、不能執行 script）
2. 注入 CSP `default-src 'none'` 擋掉所有遠端資源。預設不載入遠端圖片（追蹤像素會
   洩漏開信時間與 IP），按鈕可放行

保留天數 `NFHH_MAIL_KEEP_DAYS`（預設 14），每次 ingest 與列表時順帶清除逾期。

---

## 6. Email 轉發（v5 新增）

### 要解決什麼

原本各家人在 Cloudflare Email Routing 各有一條轉發規則，收件人清單硬編在 Worker 裡。
兩個問題：家人的信箱等於寫進版控（跟 `.gitignore` 排除 `generated/nft/clients.nft` 的理由矛盾），
而且要調整收件人得進 Dashboard 改環境變數。

搬進面板之後，收件人是 DB 資料，改動不必重新部署 Worker，
「逐步把扇出收掉」也只是把 `enabled` 設成 0。

### 資料模型

```sql
mail_recipients(id, mailbox, address, label, enabled, added_by, added_at)
UNIQUE(mailbox, address)
```

- `mailbox` 是**收件位址**（如 `netflix@share.example.com`），不是平台名稱
- `mailbox` 與 `address` 一律轉小寫存放，路由查詢靠字串比對
- 刻意不跟 `users` 表綁定 —— 多數家人還沒註冊 passkey，硬綁會讓他們收不到碼
- 重複新增同一人 = 恢復啟用並更新備註，不會產生第二筆
- 停用不等於刪除，之後可恢復，不必重打位址

### 平台 → 收件信箱的對應

存在 `settings` 的 `platform_mailboxes`（`{平台代號: 收件信箱}`），
在「轉發收件人」頁設定。這份對應同時決定三件事：

- 那一頁的分組（每個平台一組，底下是它的收件人）
- **登記邀請時要把家人加到哪個信箱**
- 信件分類的第 2 層（見上）

⚠️ **一定要 admin 明說，不能用 `代號@網域` 推。** 這個約定對 Netflix 剛好
成立、對 Disney+ 是錯的，而推錯的後果是把家人加到一個**根本收不到信的信箱**，
畫面上卻看起來一切正常。沒設對應的平台，登記邀請時直接跳過並在回應裡說明 ——
少建一筆補得回來，猜錯的那筆會安靜地什麼都收不到。

首次啟動會從既有的收件人回填**推得出來**的那幾筆（local part 等於代號），
推不出來的刻意留空，在畫面上顯示成「沒有對應到平台」等 admin 指派。
猜錯比留空更糟：留空至少是顯眼的。

有收件人、卻不對應任何平台的信箱會單獨列出來，並提供
**永久刪除**（`DELETE /api/mailboxes/{mailbox}`）—— 設錯的信箱不該只是停用，
它會繼續看起來像個有效的轉發目標。

### 轉發位址的四種狀態

`mail_recipients` 的一筆登記，跟 Cloudflare 對照起來有四種狀態，
每一種要做的事都不一樣：

| 狀態 | 條件 | 後果與處理 |
|---|---|---|
| 未查詢 | `cf_checked_at IS NULL` | 沒設 token 或還沒查過，什麼都不知道 |
| **未登記** | `cf_present = 0` | Cloudflare 根本沒有這個位址。**轉發一定退信**，而且驗證信從來沒寄過 —— 要按按鈕建立 |
| 尚未驗證 | `cf_present = 1` 且 `cf_verified_at IS NULL` | 信寄了，對方沒點。重寄或請他找垃圾郵件 |
| 已驗證 | `cf_verified_at` 有值 | 正常 |

⚠️ **「未登記」曾經顯示成「未查詢」。** 舊的同步是逐筆
`UPDATE ... WHERE address = ?`，Cloudflare 沒回傳的位址（＝它沒有那個目的地）
根本不會被 UPDATE 到，`cf_checked_at` 就永遠是 NULL。結果是**最危險的狀態
長得跟最無害的一樣**：開關開著、面板叫 Worker 轉過去、每一封都退信，
而畫面上寫的是「未查詢」。

修法是 `sync_cf_status` 拿**完整清單整份覆寫**：先把全部標成
「查過了、不在 Cloudflare」，再把查到的補回去，兩步同一個交易。
附帶好處是位址從 Cloudflare 被刪掉時，那個「已驗證」也會跟著撤掉，
不會停在兩年前的舊狀態。

根因則是新增收件人時沒有同步到 Cloudflare。現在 `POST /api/recipients`
與登記邀請都會順手 `create_destination`，所以不會再產生這種孤兒。

### 路由決策

`routing_mailbox()` 決定用哪個信箱去查名單：

| 優先序 | 來源 | 說明 |
|---|---|---|
| 1 | `X-Nfhh-Mailbox` 標頭 | Worker 帶來的**信封收件位址**，SMTP 實際投遞目標 |
| 2 | 解析出的 `To:` 表頭 | 相容尚未更新的 Worker |
| 3 | 空字串 | 查不到任何收件人，不套用任何預設名單 |

信封位址優先是因為 `To:` 只是信件內容的一部分，寄件者想寫什麼都行；兩者不一致是常態。
空的 `X-Nfhh-Mailbox` 不會蓋掉 `To:`。

**未知信箱不回傳任何人** —— 避免 Email Routing 掛上 catch-all 時整個網域的信被扇出去。

### 寄件者驗證

信任錨點是**通過驗證的品牌網域**，不是信封寄件者網域。兩條獨立路徑，任一成立即通過：

1. `dkim=pass` 且 `header.d` 落在白名單。SES 代寄時會同時有
   `header.d=amazonses.com` 與 `header.d=netflix.com` 兩條簽章，品牌那條通過就算
2. `dmarc=pass` 且 `header.from` 落在白名單

白名單由 `NFHH_MAIL_ALLOWED_SENDERS` 設定，預設 `netflix.com,disneyplus.com`。
後綴比對帶點，`netflix.com.evil.com` 不會通過。

解析 `Authentication-Results` 時先以 `;` 切段再於段內比對，
避免某段的 `dkim=fail` 跟另一段的 `header.d=` 湊成誤判通過。

### 觀察期開關

`NFHH_MAIL_ENFORCE_SENDER`（預設 `0`）：

| 值 | 行為 |
|---|---|
| `0`（預設） | **觀察期**。未通過驗證只寫日誌與稽核，照常回傳收件人 |
| `1` | 未通過驗證時 `forward_to` 回空陣列，收掉扇出 |

分兩段上線是刻意的：先累積真實信件的判斷結果，確認 Netflix 與 Disney+ 都判成通過再打開。

### 回應格式

```json
{ "ok": true, "new": true, "code_found": true, "verified": true,
  "forward_to": ["a@example.com", "b@example.com"] }
```

`forward_to` **不含** `FALLBACK_TO`，那由 Worker 自行加上。

### 使用者自己控制要不要收

成員在個人設定（首頁右上齒輪）有**一顆總開關**，關掉就把自己名下所有
mailbox 的登記一次停用。admin 那頁管的是全部人、以 mailbox 為單位；
這裡管的是「我自己收不收」。兩者操作的是同一張表，沒有第二份狀態。

⚠️ **這裡刻意不加 `user_id` 外鍵。** v5 的註解說得很清楚：這張表不跟
`users` 綁定，因為多數家人還沒註冊 passkey。自助開關改用
`users.email = mail_recipients.address` 比對（兩邊本來就都轉小寫存），
對不上任何帳號的那幾筆維持只有 admin 管得到，行為完全沒變。

關掉之前如果對方**還沒開通知**，UI 會先擋一次 —— 兩個都沒有的話，
新的碼就只剩他自己想起來打開面板才看得到。

**轉發壞掉時首頁會出現警告**（開著、但 Cloudflare 那邊沒驗證過）。這件事
最糟的地方是完全沒有徵兆：面板上開著、Worker 每次都退信、而信退在
Cloudflare 那邊，家人只覺得「怎麼都沒收到」。所以擺在首頁而不是收在設定裡
等人自己發現。條件收得很窄（`enabled && cf_checked_at != null && !cf_verified_at`）——
關掉轉發的人和已驗證的人都不該看到，常駐的警告很快就會變成背景雜訊。

**重新發送 Cloudflare 驗證信**也在這裡。位址沒驗證過的話轉發會失敗，
而那個失敗在面板上看不到（信是 Cloudflare 退的）。實作就是再打一次
建立位址那支 API —— 它對未驗證的位址會重寄。細節與實測結果見
`cloudflare.rs` 的檔頭。

### 登記邀請時就把轉發建好

admin 登記一個 Email 並選了平台時，面板順帶做兩件事：

1. 依平台建立 `mail_recipients` 的登記（`{平台代號}@{mail_domain}`），**預設啟用**
2. 在 Cloudflare 建立目的地位址 —— 驗證信與邀請函幾乎同時到，家人一次處理完

兩支都是**冪等**的：`add_recipient` 的 `ON CONFLICT` 會把已存在的恢復啟用，
Cloudflare 那支對已驗證的位址回原紀錄且不寄信。所以「若已存在則忽略」
不必自己判斷，重打就好。

Cloudflare 那步失敗**不回滾登記**，跟寄信失敗同一個原則：位址已經生效了，
回 200 帶 `warn`，而不是讓整個動作看起來沒發生。

⚠️ **這讓 `FORWARD_MAP` 更容易漂掉。** 自動新增的收件人不會出現在 Worker
的環境變數裡，而那是面板停機時唯一的退路。登記完記得兩邊都補。

---

## 7. 稽核

`GET /api/audit` 回最近 100 筆，登入即可看。記錄的動作：

| action | 何時 |
|---|---|
| `allow_add` / `allow_remove` | 白名單變更，detail 含 IP 與 TTL |
| `invite_email_registered` / `invite_email_revoked` | 登記邀請 Email |
| `join_code_sent` / `join_code_ok` / `join_code_locked` | Email 驗證碼 |
| `invite_mail_sent` / `invite_mail_failed` | 邀請函寄送結果 |
| `invite_link_opened` / `invite_link_bad` | 邀請連結被兌換／權杖對不上 |
| `allow_renewed` | 自動續期（actor 為空 = 機器做的） |
| `member_role_changed` / `platform_granted` / `platform_revoked` | 成員管理 |
| `settings_changed` | 面板設定 |
| `mail_received` | 收到信，detail 含寄件者、主旨、轉發人數 |
| `mail_sender_unverified` | 寄件者未通過驗證，detail 含驗證摘要與是否收掉扇出 |

每筆帶 `actor`、`client_ip`（機器來源的動作兩者為 NULL）。

---

## 8. API 一覽

| 方法 | 路徑 | 權限 |
|---|---|---|
| GET | `/` | 公開（前端頁面） |
| GET | `/api/status` | 公開；未登入時不揭露白名單內容 |
| POST | `/api/join/start` `/verify` | 公開；寄／核對 Email 驗證碼 |
| POST | `/api/join/invite` | 公開；兌換邀請連結的權杖，回信箱與平台 |
| POST | `/api/register/start` `/finish` | 依 §2 三種情境 |
| POST | `/api/login/any/start` `/finish` | 公開；可探索憑證 |
| POST | `/api/login/start` `/finish` | 公開；信箱 + passkey（退路） |
| POST | `/api/logout` | 登入 |
| POST | `/api/me/email` | 登入；v6 遷移用，補填信箱且只能填一次 |
| GET | `/api/passkeys` | 登入；只列自己的，不含憑證材料 |
| POST | `/api/passkeys/{id}` | 登入；重新命名，限自己的 |
| DELETE | `/api/passkeys/{id}` | 登入；限自己的，**擋掉刪到剩零把** |
| POST | `/api/allow` | 登入 |
| POST | `/api/allow/{ip}` | 登入；重新命名，member 限自己新增的 |
| DELETE | `/api/allow/{ip}` | 登入；member 限自己新增的 |
| GET | `/api/allow/{ip}/queries` | 登入；**限條目擁有者，admin 也不例外** |
| GET | `/api/audit` | 登入 |
| GET | `/api/dns-profile` | 登入 |
| POST | `/api/mail/ingest` | 共用密鑰；回覆轉發名單 |
| GET | `/api/mail` | 登入；經平台分權與顯示策略過濾 |
| DELETE | `/api/mail` | **管理員**；全部刪除 |
| GET | `/api/mail/inbox` | **管理員**；不過濾，診斷用 |
| DELETE | `/api/mail/{id}` | 登入 |
| GET | `/api/settings` | **管理員** |
| PUT | `/api/settings` | **管理員** |
| GET | `/api/members` | **管理員** |
| DELETE | `/api/members/{id}` | **管理員**；擋掉自己與最後一個 admin |
| POST | `/api/members/{id}/role` | **管理員**；擋掉降掉最後一個 admin |
| POST | `/api/members/{id}/platforms` | **管理員** |
| DELETE | `/api/members/{id}/platforms/{code}` | **管理員** |
| GET POST | `/api/recipients` | **管理員**；POST 順帶在 Cloudflare 建位址 |
| POST | `/api/recipients/{id}/verify` | **管理員**；建位址／重寄驗證信 |
| POST | `/api/mailboxes` | **管理員**；設定平台的收件信箱 |
| DELETE | `/api/mailboxes/{mailbox}` | **管理員**；永久刪掉該信箱的所有登記 |
| DELETE | `/api/recipients/{id}` | **管理員** |
| POST | `/api/recipients/{id}/enabled` | **管理員** |
| GET POST | `/api/invite` | **管理員**；登記邀請 Email，POST 順帶寄邀請函、建轉發、回連結 |
| GET | `/api/push/key` | 登入；訂閱要用的 VAPID 公鑰 |
| GET POST | `/api/push/subs` | 登入；列出／新增自己的裝置訂閱 |
| DELETE | `/api/push/subs/{id}` | 登入；限自己的 |
| POST | `/api/push/unsubscribe` | 登入；這台裝置帶著 endpoint 自己退訂 |
| GET POST | `/api/me/notify` | 登入；兩顆通知開關 |
| GET POST | `/api/me/forwarding` | 登入；自己的轉發總開關 |
| POST | `/api/me/forwarding/resend` | 登入；重寄 Cloudflare 驗證信 |
| DELETE | `/api/invite/{email}` | **管理員** |
| — | 其餘路徑 | 回前端（SPA fallback）；`/api/` 底下回 JSON 404 |

轉發收件人連讀取都限管理員 —— 那份清單決定驗證碼送到哪些外部信箱，
內容本身就是家人的信箱。

---

## 9. 設定

| 環境變數 | 預設 | 說明 |
|---|---|---|
| `NFHH_RP_ID` | `dnf.example.com` | WebAuthn RP ID，須等於瀏覽器看到的網域。**不能事後改** |
| `NFHH_ORIGIN` | `https://dnf.example.com` | 驗證 origin 用 |
| `NFHH_BIND` | `127.0.0.1:8081` | 非 loopback 會拒絕啟動 |
| `NFHH_DB` | `/data/control.db` | SQLite 路徑（docker volume） |
| `NFHH_CLIENTS_NFT` | `/nft/clients.nft` | 白名單持久化檔，需可寫 |
| `NFHH_DYNAMIC_CONF` | `/smartdns/dynamic-ip.conf` | 讀出口 IP 用，唯讀 |
| `NFHH_DOT_CONF` | `/smartdns/dot.conf` | 判斷 DoT 是否啟用，唯讀 |
| `NFHH_DOT_HOST` | `dns.example.com` | 連線教學與 iOS 描述檔用 |
| `NFHH_MAX_PER_USER` | `4` | **每人**白名單上限。v6 起取代全域的 `NFHH_MAX_ENTRIES` |
| `NFHH_DOMAIN_SET_DIR` | `/domain-set` | 平台清單來源，唯讀掛 `config/smartdns/domain-set` |
| `NFHH_DNS_AUDIT` | `/smartdns-data/audit.log` | smartdns 稽核檔，唯讀掛 `smartdns-data` |
| `NFHH_RESEND_KEY` | 空 | Resend 金鑰。留空則「用 Email 加入」停用 |
| `NFHH_MAIL_FROM` | `share@example.com` | 驗證碼信與邀請函的寄件位址 |
| `NFHH_INVITE_TEMPLATE` | `ott-share-invitation` | 邀請函在 Resend 上的樣板 id 或別名。**樣板要先發布**，草稿寄不出去 |
| `NFHH_CF_ACCOUNT` | 空 | Cloudflare 帳戶 ID |
| `NFHH_CF_TOKEN` | 空 | 需帳戶層級 `Email Routing Addresses`（**讀 + 寫**）。只有讀的話「重發驗證信」與「登記時自動建位址」會停用 |
| `NFHH_TTL_DAYS` | `7` | 預設 TTL，實際值夾在 1–30 |
| `NFHH_MAIL_SECRET` | 空 | ingest 端點密鑰。**空 = 端點停用** |
| `NFHH_MAIL_KEEP_DAYS` | `14` | 信件保留天數 |
| `NFHH_MAIL_ALLOWED_SENDERS` | `netflix.com,disneyplus.com` | 可信的 DKIM 簽章網域，逗號分隔 |
| `NFHH_MAIL_ENFORCE_SENDER` | `0` | `1` = 未通過驗證就收掉扇出 |
| `NFHH_MAIL_DOMAIN` | `share.example.com` | 轉發信箱的網域。登記邀請時用來組出 `{平台}@{網域}`。只是種子值，之後以 `settings` 為準 |

最後兩項目前**沒有寫在 `docker-compose.yml`**，走預設值。要改需自行加進 `environment:`。

---

## 10. 資料庫

`PRAGMA user_version` 記錄遷移進度，可在既有資料上重複執行（容器每次啟動都會跑）。

⚠️ **已經套用過的 migration 不能再改。** 版本號一旦超過，那個區塊永遠不會
再執行 —— 事後補進去的欄位對既有資料庫等於不存在，而且要等某支 SELECT
撞上 `no such column` 才會發現。要補就開新版本（見 v11）。

| 版本 | 內容 |
|---|---|
| v11 | 補跑 v10 那批 `add_column`。⚠️ **已套用的 migration 不能改** —— `cf_present` 曾被後補進 v10，而線上 `user_version` 已經是 10，那個區塊不再執行，直到 SELECT 撞上 `no such column` 才發現。要補就開新版本 |
| v10 | `push_subscriptions`；`users.notify_codes/notify_expiry`；`allowlist.expiry_notified_at`（到期提醒的去重標記，續期時清回 NULL）；`mail_recipients.cf_present`（見 §6 的四種狀態） |
| v1 | `users` / `credentials` / `allowlist` / `audit` / `bootstrap` |
| v2 | `invites`；既有帳號（第一個註冊的）補為 `admin` |
| v9 | `invited_emails.token_hash` 與其唯一索引（部分索引，NULL 不互相衝突）。邀請連結的權杖只存 HMAC |
| v8 | 給 `code_keywords` 一組預設值。**只在 `updated_by IS NULL` 時才動** —— 那代表值是 seed 寫的，不是人設的；有人特意清空是合法設定，不該被升級填回去 |
| v7 | `invited_emails.platforms`（登記時就決定授權）。NULL = v7 之前登記的，註冊後不自動獲得任何平台，跟遷移前行為一致 |
| v6 | `users.email`、`invited_emails`、`email_otp`、`user_platforms`、`settings`；`allowlist.ttl_days/renewed_at`；`mails.platform/skip_reason`；`mail_recipients.cf_*`。**`username` 刻意不動** —— 它是 WebAuthn 的 user handle，改了會讓已註冊的 passkey 對不上 |
| v3 | `mails` |
| v4 | `mails.html`（原始 HTML） |
| v5 | `mail_recipients`；`mails.verified` |

`mails.verified` 是 v5 才加的欄位，舊信件為 `NULL`。**讀取時不能當成 `false`**，
否則面板會把過去所有信件都標成「未通過驗證」。

---

## 11. 推送通知

驗證碼一到就推到家人手機上。面板自己當推送伺服器（RFC 8030），
不經任何第三方 SDK，程式在 `src/push.rs`。

```
收到信 → 三層過濾 → 有這個平台的人 → 加密酬載 → FCM／Apple → 手機
```

### 為什麼酬載加密不是可選的

**驗證碼直接放在通知內文**（因此也會出現在鎖定畫面）。這個決定唯一站得住腳
的前提是 RFC 8291 的端對端加密：金鑰材料只有那台裝置有，FCM 與 Apple
轉手的是一段它們自己也解不開的密文。

⚠️ 加密寫錯**不會有任何錯誤訊息** —— 推送服務照樣收下，手機那邊靜靜地
什麼都沒有。所以正確性靠 RFC 8291 §5 的測試向量釘死
（`matches_the_rfc8291_test_vector`），而不是等真機測試。

### 誰收得到

平台過濾走的是 `user_platforms`，**跟驗證碼分頁同一張表、同一條規則**。
「誰看得到碼」與「誰收得到通知」是同一個問題，分兩份寫遲早會歪。
admin 一樣要被授權才收得到，沒有特例。

⚠️ **推送絕不出現在 ingest 的回應路徑上。** Worker 只等 5 秒，逾時就退回
`FORWARD_MAP` 自己送 —— 推送再重要也不能拿轉發去換。實作是 `tokio::spawn`，
成敗只寫日誌。

### 訂閱

每台裝置一筆，`endpoint`（推送服務給的網址）天生唯一，直接拿它當去重鍵。
同一台裝置重新訂閱會蓋掉舊的那筆並把 `fail_count` 歸零。

`endpoint` **不外流到前端**（`serde(skip)`）—— 它等於「可以推播到這台裝置」
的能力。所以裝置自己退訂走 `/api/push/unsubscribe` 帶 endpoint，
設定頁的清單則用 id。

推送服務回 **404／410 時當場刪掉那筆訂閱**：那是它在說「別再推了」，
留著只會每次都失敗，而且沒辦法自己好起來。

### VAPID 金鑰

首次推送時產生，存在 `settings`（同 HMAC 那幾把的理由）。
⚠️ **換掉等於所有既有訂閱一次作廢** —— 訂閱是拿公鑰在瀏覽器端建立的，
換了之後推送服務會全部回 403。所以只在缺的時候產生，絕不順手輪替。

### 兩顆開關

設計 3a 的兩項，存在 `users` 上（跟著人跑，換裝置也還在）：

| 開關 | 預設 | 觸發 |
|---|---|---|
| `notify_codes` | **開** | 新驗證碼通過三層過濾時 |
| `notify_expiry` | 關 | 白名單剩不到 24 小時**且沒有查詢活動**時 |

第二項只在「不會自動續期」時才提醒 —— 有活動的會自己續下去，提醒只是噪音。
續期檢查每 10 分鐘跑一次而提醒視窗有 24 小時，所以用
`claim_expiry_notice` 原子式認領去重，否則同一條會被提醒 144 次。

### 平台差異（決定了通知能長什麼樣）

| 欄位 | Android Chrome | iOS（已加到主畫面） |
|---|---|---|
| `title` / `body` | ✅ | ✅ |
| `icon`（自訂圖片） | ✅ | ❌ **一律用 manifest 那顆圖示** |
| `image`、`actions` | ✅ | ❌ |
| `tag`（新碼蓋掉舊碼） | ✅ | ✅ |

所以內文只放碼本身，**不寫「點一下複製」** —— 那句話在 iPhone 上做不到。
Android 的複製按鈕由 service worker 掛上去，而剪貼簿在 worker 裡碰不到，
只能 `postMessage` 請頁面代勞（見 `main.js`）。

⚠️ **iOS 的一般 Safari 分頁完全沒有 `PushManager`。** 這不是權限問題，
是那個 API 根本不存在 —— Apple 從 iOS 16.4 至今沒有放寬過（Safari 18.4 的
Declarative Web Push 拿掉的是 service worker 的需求，不是主畫面的需求）。

⚠️ 加入主畫面時**「開啟為網頁 App」必須是開的**，否則加出來的是普通書籤，
推送會靜默失敗。iOS 26 起預設是開的，更舊的系統要自己確認 ——
設計 3b 的第 3 步就是為它存在的。

前端因此把兩件事分開判斷：**能不能推**用特徵偵測（`PushManager` 在不在，
天生涵蓋書籤模式那個坑），**為什麼不能**才認 iOS，單純為了決定顯示哪張說明。

### 什麼時候問

**這台裝置第一次進面板時**（`Home.svelte` 的 `maybeAskPush`）。設計稿原本
寫「第一次成功複製驗證碼之後」，改掉是刻意的：家人多半是收到碼才打開面板，
等他複製完再問，這一輪的碼已經自己抄完了 —— 通知要在**下一組碼來之前**
就設好才有意義。

問過的旗標存 localStorage 而不是 DB：訂閱本來就是每台裝置一筆，
「這台問過了沒」也該是每台裝置各自記，換手機時應該要再問一次。
已經開了、瀏覽器不支援、或權限被擋掉的一律不問 —— 那三種情況彈層
給不出任何他做得到的動作。

### PWA 外殼

`web/public/` 底下三樣，Vite 原樣複製到 `static/`（**不套 hash** ——
service worker 的網址一變瀏覽器就當成另一支，舊的會繼續活著）：

| 檔案 | 用途 |
|---|---|
| `manifest.webmanifest` | `display: standalone` + 圖示。iOS 靠它才認得這是 web app |
| `sw.js` | 只做 `push` 與 `notificationclick`，**刻意不做離線快取** |
| `icon-192/512.png`、`apple-touch-icon.png` | 主畫面圖示，也是 iOS 通知上顯示的那張圖 |

⚠️ **不要在 `sw.js` 加 fetch 快取。** 面板的內容是驗證碼，時效以分鐘計；
快取唯一能做到的事就是讓人看到一組過期的碼還以為是新的。

兩個檔案的 content-type 若不對會**靜默失效**（manifest 不對 iOS 就不當
web app、sw 不對瀏覽器直接拒絕註冊），所以有測試釘著
（`pwa_assets_are_served_with_usable_types`）。

---

## 12. 疑難排解

| 症狀 | 檢查 |
|---|---|
| 面板打不開 | `docker logs nfhh-control`；`NFHH_BIND` 非 loopback 會拒絕啟動 |
| 全部人被登出 | 正常，session 存記憶體，容器重啟即失效 |
| 授權後仍連不上 | `sudo nft list set inet nfhh clients_v4` 看是否真的寫進去 |
| 白名單漂移 | 等 5 分鐘的背景同步，或改動任一項目觸發全量重建 |
| 驗證碼區塊沒出現 | `NFHH_MAIL_SECRET` 沒設，端點是停用的 |
| 信推不進來 | 查 Worker 日誌的 `panel rejected: <status>`；401 = 密鑰不符 |
| 驗證碼抽錯 | 改 `mail.rs` 的規則，重啟會自動重抽全部既有信件 |
| 家人收不到轉發 | 先看「轉發收件人」頁的驗證狀態 —— 未在 Cloudflare 驗證的位址收不到信 |
| 收不到推送通知 | iPhone 要先加到主畫面、且「開啟為網頁 App」是開的（見 §11） |
| iOS 描述檔裝不起來 | 必須用 Safari 開面板；且 `dot_ready` 為 false 時不提供下載 |
