# DECISIONS.md — 改動前先讀

這裡只記**會反咬的約束**：每一條都是「看起來可以那樣改，但不行」。
操作方式見 [README.md](../README.md)，這份不重複。

---

## 統一出口只能靠應用層，不能靠路由

本機 `192.168.27.15/22` 在上游路由 `192.168.27.1` 之後，是普通 LAN 主機 ——
非閘道、無多 WAN、無 policy routing。各樓的 PPPoE 流量根本不經過本機，
**路由層做不到統一出口**。所以走 DNS → SNI proxy，在應用層把流量匯流。

→ 別再往「nftables 統一出口」的方向改，那需要本機是閘道。

## 剝 AAAA 是必要條件，不是優化

裝置在有選擇時**會優先走 IPv6**（實測：不經 proxy 直連 ifconfig.me 回的是 IPv6）。
只要裝置拿得到 Netflix 的 AAAA，就會用自己樓層的 IPv6 身分直連，統一出口當場失效。

→ smartdns 的 `address /domain/#6` 不能拿掉。

## 但 proxy 對上游刻意允許 IPv6

nginx 的 `resolver` **不加** `ipv6=off`。早期版本鎖成 IPv4，理由是「確保統一 IPv4 身分」——
那個理由是錯的：重點是「所有樓層看起來一樣」，不是「一定得是 IPv4」。
而硬鎖 IPv4 會讓純 IPv6 端點直接掛掉（`notifications.netflix.com` 只有 AAAA，實測噴 16 次 500）。

⚠️ 未驗證的尾巴：純 IPv6 端點會從**會變動的 IPv6** 出去。Netflix 是否因此察覺異常不明。
要絕對一致就把 `ipv6=off` 加回去，代價是該端點回到失敗狀態。

## split-horizon 不能省

本層 LAN（`192.168.24.0/22`）的裝置若拿到公網 IP，會繞出去再繞回來，
而 **TP-Link 未必支援 NAT hairpin** → 可能直接不通。
所以 smartdns 用 `client-rules` 對本層回 LAN IP、對其他樓層回 WAN IP。

## 手機必須填 hostname，不能填 IP

公網 IPv4 與 IPv6 prefix **都會隨 PPPoE 重撥變動**（IPv6 是 SLAAC，`valid_lft` 僅 299s）。
DNS 欄位填 IP 字面值，等於每次重撥所有樓層的所有裝置同時失效。
→ 手機走 DoT `dns.example.com`。電視類只能填 IP，這是已知且無解的限制。

## 白名單認的是「那一戶的 IPv4」，不是連線來源

面板從 Cloudflare Tunnel 進來，`cf-connecting-ip` 是開面板那台裝置的位址；
手機走 IPv6 時那是一個 /128。而 nft set 沒有 `flags interval`，比對單一位址 ——
授權一個 /128 只放行那一台裝置的那個位址：同一戶走 IPv4 的電視照樣被擋，
SLAAC 臨時位址輪替後連那台自己都會失效。IPv4 有 NAT，一筆才代表整戶。

→ 出口 IPv4 只能在**瀏覽器端**問（`web/src/lib/ip.js`），後端問到的是自己的。
→ `?ip=` / `POST /api/allow` 的 `ip` 是客戶端說了算的值，所以只在登入後採信。
→ 想改成支援 IPv6 的話，要動的是「存 /64 前綴」那一整套：nft set 要加
   `flags interval`、`my_ip_allowed` 的字串相等要改成前綴比對、`dnslog` 的
   `stats`/`recent` 也是（它們用完整位址當 key，不改的話自動續期會失效）。

## 白名單的真實來源是 SQLite，nft set 只是投影

每次變更都是**整個 set 重建**而非增量增刪，因此冪等、可自我修復 ——
有人手動改過 nft、或容器重啟造成漂移，下次同步就收斂回正確狀態。

`expires_at` 在 DB 存**絕對時間戳**，不是相對 TTL，所以重開機後剩餘時間才是對的
（nft 自己的 timeout 做不到這點，重載會從頭起算）。

→ 別改成增量增刪，也別改用 nft 的 timeout。

## ACL 只有一份，閘門所有入站埠

白名單曾經同時存在 smartdns client-group 與 nft set 兩份，會不同步。
現在統一由單一 nft set 閘門 `:53`/`:853`/`:443`：非白名單來源**封包直接 drop**，
連 DNS 都問不到 —— 本機完全不是 open resolver。

## 白名單上限是濫用防護，不是頻寬護欄

本機 1G 對稱（實測 933↓/943↑），理論可撐 40+ 條 4K 串流。
上限的作用是「白名單外洩後不被大量盜用」，不是怕塞爆頻寬。

v6 起這個上限是 **per-user**（`NFHH_MAX_PER_USER=4`），全域上限已移除。
換法的理由：全域數字擋的是「總量」，但真正想擋的是「單一帳號被盜之後
能塞進多少條」。改成每人 4 條之後，總量的天花板變成 4 × 成員數，
而成員數由 admin 的 Email 登記把關 —— 那道關卡比一個數字精準。

→ 想重新加回全域上限之前，先想清楚它要擋什麼是 per-user 擋不到的。

## 平台分權只在面板層，網路層仍是全有全無

`user_platforms` 決定「誰看得到哪個平台的驗證碼、誰收得到轉發」。
它**不影響 DNS 與白名單**：一個 IP 進了 nft set，smartdns 就會對所有
domain-set 回 proxy IP，跟這個人有沒有該平台的權限無關。

要在網路層也分，得讓 smartdns 依來源 IP 分 client-group、每組掛不同的
domain-set，nginx 的 SNI 白名單也要跟著分來源。而白名單 IP 是動態的，
等於每次授權都要重寫分組設定並 reload smartdns。代價遠大於收益：
沒有平台密碼的人本來就登不進去，光解開 DNS 不等於拿到帳號。

⚠️ 已知後果：沒有 Disney+ 權限的成員，他的裝置流量照樣走共用出口，
因此**仍然會被算進 Disney+ 的同戶判定**。分權管的是帳號存取，不是流量歸屬。

→ UI 上刻意不標註這件事，但改動分權邏輯前要知道它的邊界在哪。

## smartdns 稽核檔的可見性是分級的

`audit-enable yes` 之後，`/var/lib/smartdns/audit.log` 會記下**每個裝置
查過的每個網域**。面板唯讀掛這個檔，用途是判斷白名單條目還活著（自動續期）
與顯示「這個網路最近在用」。

分級是刻意的：一般成員只拿得到**自己那個 IP** 的逐筆網域；
admin 只拿得到彙總數字（幾筆、最後一次何時），看不到家人查了哪些網域。

→ 別為了「管理方便」把逐筆內容開給 admin。面板的用途是管白名單，
不是當家人的瀏覽紀錄查詢器。

## 面板只綁 loopback —— 這是 `CF-Connecting-IP` 可信的前提

那個標頭決定把哪個 IP 寫進防火牆白名單。只要面板能被直連，
任何人都能自己塞一個標頭把自己加進白名單。程式啟動時會檢查 `NFHH_BIND`，
非 loopback 直接拒絕啟動。同理**刻意不讀** `X-Forwarded-For`（誰都能偽造）。

→ 別為了「本機測試方便」把 bind 改成 `0.0.0.0`。

## Cloudflare Tunnel 不能載資料平面

CF 會終結 TLS，載不了 SNI passthrough，代理也只處理 HTTP（載不了 :853 的 DoT）。
→ 面板走 Tunnel，DNS/proxy 走直接公網埠。`dns.example.com` 的 CNAME 必須是**灰雲**。

## nfhh-firewall.service 刻意沒有 ExecStop

寫了 `ExecStop=nft delete table inet nfhh` 是 fail-open：unit 一旦被停止
（相依變動、手動 stop、關機時序），ACL 就消失，而容器仍在監聽 `:53`/`:443` ——
瞬間變成對全網際網路開放的 open resolver 兼免費 Netflix 跳板。
寧可讓規則留著。真要移除請手動 `nft delete table inet nfhh`。

## RP ID 不能事後改

WebAuthn 憑證綁在 RP ID 上。改網域 = 所有 passkey 全部作廢，得重新註冊。

## smartdns 用 C 版，不是 smartdns-rs

原設計指定 smartdns-rs，但它**沒有可用的容器映像**。改用 `pymumu/smartdns`，
本設計所需指令（`client-rules` / `bind-tls` / `address /domain-set:<name>/` / `#6`）全部實測支援。

## 憑證直接掛載，不複製

compose 把 acme.sh 的目錄唯讀掛進容器：`"/etc/ssl/*.example.com_ecc:/certs:ro"`。
早期版本複製進專案目錄，權限、原子性、mode 比對連踩三個坑，全部源自複製本身。

⚠️ 掛載點必須在 `/etc/smartdns` **之外** —— 那層本身是唯讀掛載，
Docker 無法在裡面建立新的掛載點，會啟動失敗。

## smartdns 沒有 `CAP_DAC_OVERRIDE`，設定檔必須 other 可讀

smartdns 啟動後會把 capabilities 縮到只剩 `NET_BIND_SERVICE` + `NET_RAW`
（工作程序實測 `CapEff=0x2400`）。**丟掉 `CAP_DAC_OVERRIDE` 之後，它的 uid 0
就不再能繞過檔案權限檢查** —— 一個屬於一般使用者的 `640` 檔案，即使身為 root
也會讀到 Permission denied，而載入失敗是致命錯誤，`:53` 會一起消失。

- 憑證：acme.sh 的 `root:root 600` 正好可行（uid 0 靠 owner 位元讀得到）
- 設定檔：所有掛進 smartdns 的都**必須 other 可讀**（目前皆 `664`）。設成 `600`/`640` 會讓它起不來

> 除錯提醒：`docker exec` 測試讀取會**誤導** —— exec 開的是全新程序，拿的是完整
> capability 集。要看真實情況請讀 `/proc/<pid>/status` 的 `CapEff`。

## config/ 與 generated/ 必須分開掛載，不能巢狀

`/etc/smartdns` 是唯讀掛載，**Docker 無法在唯讀掛載裡面再建立掛載點**，
所以產生的設定不能掛成 `/etc/smartdns/generated/`，會直接啟動失敗。
現況是掛成獨立的 `/etc/smartdns-gen`，`smartdns.conf` 用 `conf-file` 引入。

→ 別為了「路徑看起來整齊」把 generated 掛回 `/etc/smartdns` 底下。

## compose 的 `name:` 與 volume 名寫死，不跟資料夾走

compose 預設用資料夾名當專案名，volume 實際名稱是「專案名_volume名」。
資料夾一改名，compose 就認成新專案、建出全新的空 volume ——
面板的 SQLite（帳號、Passkey、白名單、信件）會整組看起來消失。

`docker-compose.yml` 因此寫死 `name: nfhh`，兩個 volume 也寫死成搬遷前的
既有名稱（`smartdns_smartdns-data` / `smartdns_control-data`）。

→ 別為了「名字一致」把 volume 名改掉，除非你先把資料搬過去。

## domain-set 清單只能放平台自己持有的網域

`config/smartdns/domain-set/<平台>.list` 除了決定哪些網域被改寫到 proxy，現在也是
「驗證碼卡片替哪些連結畫品牌按鈕」的依據（`platforms::domains`）。放進第三方
CDN 或分析服務的網域，等於替它們背書。

清單條目要寫 ASCII（IDN 用 punycode）：比對的是瀏覽器眼中的 host，`url` crate 會把
連結的 host 轉成 punycode，清單裡的 unicode 網域永遠對不上、按鈕會靜默消失。

→ 影片 CDN 之類的請放獨立的 `*-cdn.list`（啟不啟用都行，`platforms::domains` 只讀
`<平台>.list`），不要混進主檔。

## 登入與註冊的 session 狀態互斥，不能共用鍵

`login_start` 存 `S_LOGIN_USER`、`register_start` 存 `S_REG_USER`，任一流程開始
時 `clear_auth_flows` 清掉全部（`S_REG`、`S_REG_USER`、`S_AUTH`、`S_LOGIN_USER`、
`S_DISC`；刻意不清登入身分與 `S_EMAIL_PROOF`）。以前共用 `S_REG_USER`：member 先
啟動「新增 Passkey」、再對 admin 的 Email 啟動登入、最後提交註冊回應，新金鑰就寫進
admin 列。`register_finish` 另外硬檢查「目標 = 目前登入者」（`check_registration_owner`）。

→ 別為了少一個鍵把兩條流程合回去；別把 `check_registration_owner` 拿掉。

## 信箱所有權證明綁在 session 上，不只是全域旗標

`email_otp.verified_at` 是全域的（以 Email 為鍵）。`register_start` 另外要求
`S_EMAIL_PROOF` 等於該信箱 —— 證明是**這個瀏覽器**做的（驗證碼與邀請連結兩條路
都寫這把鍵）。否則攻擊者只要知道受邀 Email、等真正持有人驗證完，就能在自己的
瀏覽器搶先建帳號。

## v6 遷移路徑已移除：帳號一定有 email，登入只認 email

補填 Email 的端點、`username` 登入退路與 `rename_owner` 都拿掉了。它們讓一個
只有舊 Passkey 的帳號可以自填任意信箱、搶走家人的身分與轉發控制。
`users.username` 欄位仍在（WebAuthn user handle 與歷史稽核的 actor），但不再
是登入識別。啟動時若發現沒有 email 的帳號會記 error 並列出 username
（`db::users_without_email`）：那種帳號仍可用可探索登入進來，但不能用 Email 登入、
也對不上平台分權與轉發，只能刪掉重邀。

→ 別把 `find_user(username)` 加回登入流程。

## Worker 的 fail-open 只對「面板不可用」，不對「面板拒收」

ingest 回 401／422 = 拒收（永久）；5xx（含端點未啟用的 503）、408／429、逾時、
連不上 = 不可用（暫時）。只有後者退回 FORWARD_MAP；前者只轉 FALLBACK_TO
（2xx 但 JSON 不是物件也算拒收，Worker 才不會在 `forward()` 之前拋例外）。
以前所有失敗都是 400、都走 FORWARD_MAP，寄一封讓解析器出錯的信就能繞過寄件者驗證
直達家人信箱。

→ `mail_ingest` 不能改回 `AppError`（那是一律 400）。

## `received_at` 是寄件者說的，`ingested_at` 才是面板的時鐘

排序、分頁、保留期只看 `ingested_at`；`received_at` 夾在保留期內（最早 `NFHH_MAIL_KEEP_DAYS` 天前）
（`NFHH_MAIL_KEEP_DAYS`，預設 14 天）到一小時之後之間，超出就改用現在，而且
只做顯示（`clamp_received`）。以前用 `received_at`：一封未來日期的信永遠排第一、
永遠不被清。遷移（v12）回填時取 `min(received_at, now)`，升級前就躺在表裡的
偽造日期不會變成可信時間；`ingested_at` 為 NULL 的列一律當過期清掉。

## 只採信第一個 Authentication-Results，而它的可靠性來自 Cloudflare，不來自我們

`mail::parse` 只看最上面那個、authserv-id 等於 `NFHH_MAIL_AUTHSERV_ID` 的表頭
（第一個表頭對不上就當沒有，不會滑到第二個）。這比以前「全部串起來看」嚴格，但仍
假設收信端把自己的表頭放最前面。`authserv-id` 不是秘密。部署後要用任務 7 的 canary
證實；結果與日期記在這裡：

- （canary 日期）：（verified=false／true）

解析器已對 token 錨定、註解與引號字串免疫（`strip_cfws`）。唯一無法在解析器端
處理的情況：收信端把寄件者可控的欄位**不加引號**地回寫、而該欄位含原始 `;`
（違反 RFC 8601 §2.2）—— 任何以 `;` 切段的解析器都會多出一段。canary 的第二封信
測的就是這件事。

→ canary 若失敗，程式端沒有更多可做的事；決策見任務 7 步驟 5。

## Worker 沒有面板設定時照 FORWARD_MAP 轉發是刻意的

`PANEL_ENDPOINT`／`PANEL_SECRET` 缺席是部署狀態，攻擊者改不了 Worker 的環境變數，
所以它不是攻擊面；而「Worker 先於面板部署」時它是唯一的轉發路徑。分類上它是
`unconfigured`，跟「面板拒收」（只轉 FALLBACK_TO）與「面板不可用」分開記錄。

## 白名單條目只有新增者或 admin 能改寫；無主條目由 admin 認領

`upsert_allow_owned` 把「檢查擁有者」與「寫入」放在同一句 SQL（`ON CONFLICT … DO UPDATE
… WHERE allowlist.added_by = excluded.added_by OR ?is_admin`），沒有先查後寫的空隙；
不成立時 `changes()` 是 0，handler 回「不是你新增的」。同一個 NAT 後面的第二個人
不能再對同一個 IP 按「延長」——那個網路本來就通了（`my_ip_allowed` 看全部條目），
面板會直接這樣告訴他。`nft::import_legacy` 匯入的無主列（`added_by IS NULL`）
只有 admin 能改寫，改寫時 admin 成為擁有者（`coalesce`），有主的列永遠不會被搶。

→ handler 一律用 `upsert_allow_owned`；`upsert_allow` 只給匯入與測試。

## 推播訂閱：每人 8 筆，接手別人的 endpoint 算新裝置但仍被允許

配額在同一把鎖內檢查與寫入（`MAX_PUSH_SUBS_PER_USER`）。接手他人 endpoint
（`ON CONFLICT(endpoint)` 轉移 `user_id`）在有空位時仍然允許 —— endpoint 不會
序列化給前端，要拿到得先有資料庫或那台裝置；v2 審查的決定是「計入配額」而不是
「禁止」。`p256dh` 只收 65 bytes 未壓縮點（`push::valid_keys`）：壓縮點能通過
曲線檢查、推送服務也回 201，但裝置永遠解不開。扇出同時最多 8 個 task
（`PUSH_FANOUT_CONCURRENCY`）、整批 60 秒（`PUSH_FANOUT_DEADLINE_SECS`），
連續失敗 10 次（`PUSH_MAX_FAILS`）的訂閱不再參與（含到期提醒）。

## 稽核表有上限，公開端點有限流；兩者一起才守得住

`audit` 表保留 90 天、最多 20 000 列（`NFHH_AUDIT_KEEP_DAYS` 夾在 1 到 3650、
`NFHH_AUDIT_MAX_ROWS` 夾在 100 到 1 000 000，每 5 分鐘清一次），所有未登入就能
寫稽核的端點（join/start、join/verify、join/invite、未登入的 register/start）共用
一個固定視窗限流（每 IP 每 10 分鐘 30 次、全域 200 次）。只有其中一項的話：只設
上限，洪水會把真正的稽核擠掉；只限流，表仍會無限長大。殘餘：全域 200／10 分鐘
≈ 每天 28 800 列，持續的分散式洪水仍能把歷史壓到約 17 小時 —— 要真的防洪，可以讓
`actor IS NOT NULL` 的列不受列數上限影響（公開端點寫的列 actor 一律 NULL）。
洪水期間公開的加入流程會被擋 10 分鐘，登入與已登入的加金鑰不受影響（刻意豁免）。

→ 新增任何不需登入就會寫稽核的端點，都要先過 `throttle_public`，而且要在任何
DB 存取之前。

## Docker 依賴 nfhh-firewall 成功：fail-closed 是刻意的

`deploy/docker.service.d/10-nfhh-firewall.conf` 用 `Requires=` 讓 nft 載入失敗時
Docker 不啟動，連管理面板一起不起來；`ExecStartPre=nft -t list table inet nfhh` 再驗
一次規則真的在核心裡（unit 是 `RemainAfterExit` 的 oneshot，表被手動刪掉後它仍是
active）。代價是「規則檔寫錯就全停」；不這樣做的代價是 open resolver 上 Internet。
跟 `nfhh-firewall.service` 刻意沒有 `ExecStop` 是同一個判斷的兩面：
**規則在的時候不要拿掉，規則不在的時候不要開埠。**
