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

→ 影片 CDN 之類的請照既有慣例放 `*-cdn.list.disabled`，不要混進主清單。
