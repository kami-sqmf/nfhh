<div align="center">

<img src="app/control/web/public/icon-192.png" width="72" alt="OTT Household 專案標誌">

# OTT Household

*讓分散在各樓層的裝置，在串流平台眼中收斂成同一個 household*

![Rust](https://img.shields.io/badge/Rust-edition_2024-000000?style=flat-square&logo=rust&logoColor=white)
![Axum](https://img.shields.io/badge/Axum-0.8-4a4a4a?style=flat-square)
![Svelte](https://img.shields.io/badge/Svelte-5-ff3e00?style=flat-square&logo=svelte&logoColor=white)
![SQLite](https://img.shields.io/badge/SQLite-embedded-003b57?style=flat-square&logo=sqlite&logoColor=white)
![Docker](https://img.shields.io/badge/Docker_Compose-3_services-2496ed?style=flat-square&logo=docker&logoColor=white)
![WebAuthn](https://img.shields.io/badge/Auth-Passkey-34a853?style=flat-square)

[概觀](#概觀) • [運作原理](#運作原理) • [快速開始](#快速開始) • [專案結構](#專案結構) • [日常操作](#日常操作) • [驗證統一出口](#驗證統一出口) • [主機運行需求](#主機運行需求)

</div>

同一棟樓、各樓層有各自的 PPPoE 與各自的動態 IP，Netflix 會判定成不同住所。本專案讓這些裝置的串流控制流量統一從一台主機出去，對外只呈現一個 IP。

作法分兩段：自架 smartdns 把平台網域解析到本機，本機再用 nginx 做 SNI passthrough proxy 轉發。流量**不解密**，端到端加密完整保留。

> [!CAUTION]
> 套用順序不可顛倒：**先 `nft -f` 載入白名單，再去路由器開 port forward。**
> 詳見 [安全須知](#安全須知)。

## 概觀

| 你要做的事 | 看這裡 |
|---|---|
| 日常操作、除錯、確認狀態 | 本檔 |
| 從零架一套（只需做一次的步驟） | [docs/SETUP.md](docs/SETUP.md) |
| 改動程式或設定前，先確認哪些不能動 | [docs/DECISIONS.md](docs/DECISIONS.md) |
| 管理面板的 API、權限、資料模型 | [docs/CONTROL.md](docs/CONTROL.md) |

系統由三個容器組成：

- **smartdns** — DNS 改寫與 split-horizon 分流，剝除 AAAA
- **sni-proxy**（nginx）— 讀 ClientHello 的 SNI 比對白名單，純 TCP passthrough
- **control** — Passkey 管理面板（Rust + Axum + SQLite），管理白名單、成員與驗證碼。登入走 Passkey，備援是 Email 驗證碼（只有 member 權限）

## 運作原理

```
[手機／電視 @ 其他樓層]
   │ ① DNS 查 *.netflix.com → 本機 smartdns
   │    來源 ∈ nft 白名單才收得到封包，否則 kernel 直接 drop
   │    回應：203.0.113.10（WAN），AAAA 剝除
   ▼
[TLS 連 DNS，SNI=www.netflix.com]
   │ ② 路由器 port forward → 192.168.27.15:443
   │ ③ nginx 讀 ClientHello 的 SNI，比對白名單
   │ ④ 純 TCP passthrough（不解密）→ 解析真實 Netflix IP → 轉發
   ▼
[本機連 Netflix，出口 = DNS IP]
   ✅ 所有樓層共用同一出口 IP
```

本層 LAN 的裝置走捷徑：DNS 直接回 `192.168.27.15`，不繞出去（TP-Link 未必支援 NAT hairpin）。

### 已驗證的行為

| 測試 | 結果 |
|---|---|
| 本層 LAN 查 `www.netflix.com` | → `192.168.27.15`（LAN IP） |
| 其他來源查 `www.netflix.com` | → `203.0.113.10`（WAN IP） |
| AAAA 查詢 | → 無答案（已剝除，逼走 IPv4） |
| 非 Netflix 網域 | → 正常上游解析 |
| 經 proxy 連 Netflix | → HTTP 200，憑證 `CN=www.netflix.com` (DigiCert)，**未解密** |
| 經 proxy 連 `example.com` | → 502 拒絕（非開放中繼） |
| 經 proxy 問 `ifconfig.me` | → `203.0.113.10`（IPv4 出口） |
| **不**經 proxy 直連 `ifconfig.me` | → `2001:db8:...:977e`（**IPv6**） |

> [!IMPORTANT]
> 最後兩列是本專案最重要的實證：**裝置在有選擇時會優先走 IPv6**。
> 只要裝置拿得到 Netflix 的 AAAA，就會走 IPv6 直連、以自己樓層的 IPv6 身分出去，統一出口當場失效。
> 所以 smartdns **剝除 AAAA 不是優化，而是必要條件**。

反過來說，**proxy 對上游則刻意允許 IPv6**（nginx `resolver` 不加 `ipv6=off`）。早期版本鎖成 IPv4，理由是「確保統一 IPv4 身分」—— 那個理由是錯的：重點在於「所有樓層看起來一樣」，不是「一定得是 IPv4」，走本機 IPv6 出去同樣是單一固定身分。

硬鎖 IPv4 反而會讓純 IPv6 端點直接掛掉（`notifications.netflix.com` 只有 AAAA，實測噴 16 次 500）。實測 nginx 對雙棧目的地仍優先選 IPv4，只在純 IPv6 時才走 v6，對外身分以 `203.0.113.10` 為主。

### 實網驗證

2026-08-07，來源 `203.0.113.45`。其他樓層的 iPad 經本服務完成完整 Netflix 流程，控制平面全數 200：

`www.netflix.com`、`ichnaea-web.netflix.com`（遙測）、`web.prod.cloud.netflix.com`（API）、`oca-api.*.origin.prodaa.netflix.com`（OCA 導流）、`logs.netflix.com`、`assets.nflxext.com`、`occ-*.nflxso.net`、`ae.nflximg.net`、`notifications.netflix.com`

同時確認三件事：

- nft 白名單生效（docker bridge `172.19.0.x` 被 drop，核心有 `nfhh-drop` 記錄）
- Netflix 未發布帶 IP hint 的 HTTPS／SVCB (TYPE65) 記錄 → 無此繞過管道
- `nflxvideo.net`（影片 CDN）仍由裝置直連 CHT 在地 OCA，未經本機 —— 符合預期設計

## 快速開始

### 環境需求

| 項目 | 需求 |
|---|---|
| Docker + Docker Compose | 三個服務都跑在容器裡 |
| `nftables` | 白名單 ACL，於主機層執行 |
| 路由器可設 port forward | TCP + UDP `53`、TCP `443`、TCP `853` → 本機 |
| Cloudflare 管理的網域 | 面板走 Tunnel、DoT 走 CNAME、驗證碼走 Email Routing |
| acme.sh 已在簽發萬用憑證 | DoT 用 |

### 起始設定

```bash
git clone <repo> /opt/nfhh && cd /opt/nfhh
cp .env.example .env          # 唯一必填的是 NFHH_DOMAIN
./nfhh bootstrap              # 建立 generated/ 的佔位設定
sudo nft -f config/nft/nfhh.nft
./nfhh up
```

`nft -f` 一定在 `up` 之前：smartdns 與 nginx 是 host network，容器一起來就直接綁 `:53`／`:443`／`:853`。`./nfhh up` 與 `./nfhh restart` 會先確認 `inet nfhh` 表在（需要 sudo），不在就印修復指令並拒絕啟動；CI／測試機可設 `NFHH_SKIP_FIREWALL_CHECK=1` 跳過。

> [!WARNING]
> 這只是把服務跑起來。完整架設還包含路由器 port forward、Cloudflare Tunnel、
> Email Routing 與憑證，**依序**做完 [docs/SETUP.md](docs/SETUP.md) 的七個步驟才會有可用的系統。

`.env` 只需填 `NFHH_DOMAIN`，`dnf.` ／ `dns.` ／ `share.` 三個子網域都由它衍生。其餘密鑰留空只會停用對應功能，不影響啟動。

## 專案結構

資料夾只有兩種：**`config/` 是你手改的，`generated/` 是機器寫的。**搞不清楚某個檔案能不能改，看它在哪個資料夾就知道。

```
nfhh                          ← 所有日常操作的入口，先跑 ./nfhh help
docker-compose.yml            三個服務：smartdns / sni-proxy / control
.env.example                  設定範本，複製成 .env 後填入網域與密鑰
.env                          網域與密鑰，未進版控

config/                       ⭐ 手改的設定，全部進版控
  smartdns/
    smartdns.conf             DNS 規則（含 split-horizon 分流）
    domain-set/
      netflix.list            Netflix 控制平面網域（啟用中）
      disneyplus.list         Disney+ 控制平面網域（啟用中，尚未實網驗證）
      *-cdn.list.disabled     影片 CDN（預設停用，household 判定失敗才啟用）
      test.list               ifconfig.me 驗證用（常設診斷，不要拆）
  nginx/nginx.conf            SNI passthrough proxy
  nft/nfhh.nft                白名單 ACL ← 安全核心

generated/                    ⚠️ 全部自動產生，改了會被覆蓋，未進版控
  smartdns/dot.conf           DoT 設定            ← scripts/sync-cert.sh 寫的
  smartdns/platforms.conf     domain-set 宣告     ← ./nfhh apply 寫的
  smartdns/dynamic-ip.conf    其他樓層 → WAN IP   ← ./nfhh apply 寫的
  smartdns/dynamic-ip-lan.conf 本層 → LAN IP      ← ./nfhh apply 寫的
  nginx/sni-allow.conf        SNI 白名單          ← ./nfhh apply 寫的
  nft/clients.nft             白名單條目          ← 控制平面寫的（含家人 IP）

scripts/                      ./nfhh 背後實際執行的東西
  bootstrap.sh                建立 generated/ 的佔位版本
  apply-config.sh             由平台清單與當下 IP 重新產生所有衍生設定
  sync-cert.sh                憑證續期後重載 smartdns
  find-missing-domains.sh     找出裝置查過但未被平台清單涵蓋的網域
deploy/                       五個 systemd unit：開機持久化、IP 同步、憑證重載
app/
  control/                    Passkey 管理面板（Rust + Axum + SQLite）
  cloudflare/email-worker.js  驗證碼信件 → 面板，貼進 CF Dashboard 即可，無需建置
docs/                         SETUP.md ／ DECISIONS.md ／ CONTROL.md
```

`generated/` 整個不進版控：內容是機器當下的狀態，進版控只會讓 `git status` 永遠是髒的；`clients.nft` 更含家人所在網路的公網 IP，屬於個資。全新 checkout 請先跑 `./nfhh bootstrap`（`./nfhh up` 會自動代跑）。

## 日常操作

所有東西都走 `./nfhh`：

```bash
cd /opt/nfhh && ./nfhh status
```

| 命令 | 作用 |
|---|---|
| `./nfhh status` | 一頁看完：容器、埠、白名單、對外 IP、憑證、systemd |
| `./nfhh up` | 啟動三個容器（先確認 nft 表 `inet nfhh` 存在，不在就拒絕） |
| `./nfhh logs [服務]` | 追蹤日誌，服務可填 `smartdns` ／ `sni-proxy` ／ `control` |
| `./nfhh restart` | 重啟三個容器（同樣先確認 nft 表） |
| `./nfhh apply` | 改完平台網域清單後，讓變更生效 |
| `./nfhh check <來源IP>` | 找出該來源查過、但沒被平台清單涵蓋的網域 |
| `./nfhh cert` | 重新部署憑證並重載 smartdns（需 sudo） |
| `./nfhh down` | ⚠️ 停掉服務，全家會斷 DNS |

### 新增或調整平台網域

清單是唯一資料來源：一個 `.list` 檔就是一個平台。

```bash
$EDITOR config/smartdns/domain-set/disneyplus.list && ./nfhh apply
```

新增平台就丟一個新的 `.list` 檔進去，停用就改名成 `.disabled`。`apply` 會據此重新產生 domain-set 宣告、address 規則與 nginx 的 SNI 白名單。nginx 用 graceful reload（不中斷播放中的串流），smartdns 只能重啟。

> [!WARNING]
> **這只是網路層那一半。** 還要在 Cloudflare 開收件位址、在面板設定該平台的收件信箱與寄件者位址，驗證碼才收得到 —— 而漏掉時面板上完全看不出來（平台會正常出現在授權矩陣裡）。完整步驟見 [docs/SETUP.md](docs/SETUP.md) 第 6.5 節。

漏掉的網域會**靜默走直連**、用該樓層自己的 IP 出去，沒有任何錯誤訊息。先在該裝置上把目標 App 完整操作一輪，再用 `./nfhh check <該裝置的公網IP>` 撈漏網之魚。

### 白名單管理

**一律經由面板**（`https://dnf.example.com`）。SQLite 是唯一真實來源，控制平面每 5 分鐘會依 DB 內容整個重建 nft set。

> [!CAUTION]
> **不要用 `nft add element` 手動加。** 那只改記憶體狀態，而且下一次背景同步（最多 5 分鐘）就會被 flush 掉，過程沒有任何錯誤訊息。

查目前生效中的條目：

```bash
sudo nft list set inet nfhh clients_v4
```

### 出口 IP 變動

由 `nfhh-sync-ip.timer` 每 5 分鐘自動處理，也可手動觸發：

```bash
cd /opt/nfhh && ./nfhh apply
```

## 驗證統一出口

不要拿 Netflix 當第一個測試對象 —— 它只給你模糊的過或不過。`ifconfig.me` 會直接回傳它看到的來源 IP，等於把整條鏈路的結果變成一個可讀的數字。

**前置條件**：nft ACL 已套用、路由器 port forward 已設定。

1. 手機連到**其他樓層**的 WiFi，先用**一般 DNS** 開 `https://ifconfig.me`，記下顯示的 IP。這是該樓層自己的公網 IP，也是待會要加進白名單的值。

2. 用那支手機開面板 `https://dnf.example.com`，登入後按「授權目前這個網路」。面板讀的就是這個 IP，不必自己抄。

   面板還沒架起來時，才用 `sudo nft add element inet nfhh clients_v4 { <IP> timeout 7d }` 手動加 —— 面板一上線，這種手動條目會在下次背景同步時被清掉。

3. 手機 WiFi 設定 → DNS 手動改成 `203.0.113.10`，關掉再開 WiFi 讓設定生效。

4. 重新開 `https://ifconfig.me`（先清瀏覽器快取或用無痕視窗）。

| 結果 | 意義 |
|---|---|
| 顯示 **`203.0.113.10`** | ✅ 整條鏈路成立 —— DNS 改寫、port forward、SNI proxy、統一出口全部通了 |
| 仍顯示該樓層自己的 IP | ❌ DNS 沒生效（手機沒吃到設定，或走了 DoH／私人 DNS） |
| 顯示某個 IPv6 位址 | ❌ 走了 IPv6 直連繞過 proxy，檢查 AAAA 是否確實被剝除 |
| 連不上或逾時 | ❌ 白名單沒加到，或 port forward 沒設對。查 `sudo nft list set inet nfhh clients_v4` |

確認通過之後，才值得去試 Netflix。

> [!NOTE]
> `config/smartdns/domain-set/test.list`（`ifconfig.me`）是**永久保留**的診斷管道，不是暫時的測試。
>
> 每次調整設定、新增平台、或懷疑某層樓沒生效時，開一次 `https://ifconfig.me` 就能把整條鏈路的狀態變成一個可讀的數字。這比拿 Netflix 當測試對象可靠得多 —— Netflix 只給模糊的過或不過，判定還混雜帳號狀態、裝置數等其他因素。
>
> 管理面板的「連線教學 → 確認設定」分頁也是引導使用者做這個檢查，會自動帶入目前的對外 IP 並附上判讀對照表。

## 主機運行需求

| 項目 | 需求 | 為什麼 |
|---|---|---|
| **對外 IPv4** | **必須固定** | 電視、電視盒只能填 IP 字面值當 DNS。IP 一變它們永久失效，**沒有任何自動化能補救**，只能逐台手動改 |
| **對外 IPv6** | 可變動 | 專案設定與腳本內完全沒有寫死 IPv6（已 grep 驗證）。前提是 ddns-go 的 AAAA 更新正常 |
| **本機 LAN IP** | **需要 DHCP 保留** | 見下方說明，這是整套系統唯一無法自我修復的環節 |
| 對外埠 | TCP + UDP `53`、TCP `443`、TCP `853` 轉發到本機 | 缺 `853` 則 DoT 只在自家網段可用 |

> [!WARNING]
> **LAN IP 是最脆弱的一環。**
>
> `192.168.27.15` 目前是 DHCP 動態取得（`scope global dynamic`、`proto dhcp`），而路由器的 port forward 是指向這個位址的。
>
> 它一變，`:53` ／ `:443` ／ `:853` 的入站全部中斷 —— 而 `./nfhh apply` 只能改 smartdns 的回應，**改不了路由器的轉發目標**。固定對外 IPv4 之後，這反而成為最脆弱的地方。
>
> → 請在 TP-Link 上對本機 MAC 設定 DHCP 保留。

路由器的 IPv6 入站規則不要綁完整位址，理由與設法見 [docs/SETUP.md](docs/SETUP.md) 的「路由器 port forward」一節。

### 一個已知的不確定

純 IPv6 的端點會從會變動的 IPv6 出去，Netflix 是否因此察覺異常尚未經過驗證。取捨理由見 [docs/DECISIONS.md](docs/DECISIONS.md) 的「proxy 對上游刻意允許 IPv6」一節。

## 安全須知

> [!CAUTION]
> **套用順序不可顛倒：先 `nft -f`，再去路由器開 port forward。**

`:53` 和 `:443` 會對整個網際網路暴露，唯一的防線是 nft 白名單：

- 沒有它，`:53` 就是 open resolver → 會被拿去做 DNS 放大攻擊，你的 IP 會進黑名單
- 沒有它，`:443` 就是免費的 Netflix 跳板（SNI 白名單只限制**去哪**，不限制**誰能用**）

> [!WARNING]
> `deploy/docker.service.d/10-nfhh-firewall.conf` 讓 Docker 在 nft 規則載入失敗時**不啟動**。
> 這是刻意的：規則不在的時候，`:53` 開著就是 open resolver。

## 已知限制

1. **電視類裝置只能填 IP**，重撥換 IP 後要手動重設。手機走 DoT hostname 不受影響。
2. **智慧電視與 Chromecast 可能硬編碼 DNS 或走 DoH**，繞過本服務、無法納管。
3. **出口 IP 穩定性**取決於本機 uplink 的 WAN IP，重撥會換，屆時所有樓層需重新登記。
4. **影片 CDN 預設不代理**，若 household 判定仍失敗才需啟用，見 `config/smartdns/domain-set/netflix-cdn.list.disabled`。

## 待辦

初版清單裡的項目已全數完成：nft 套用、port forward、Netflix 實網驗證、DNS-01 憑證、控制平面、IP 自動改寫。剩下的是：

- [ ] **Disney+ 尚未實網驗證。** 網域清單來自公開資料，未經其他樓層實測。Disney+ 端點比 Netflix 分散，且依地區不同。漏掉的網域會**靜默走直連**、用該樓層自己的 IP 出去，沒有錯誤訊息，只會表現成「有時還是被判定不同住所」。補清單的工具是 `./nfhh check <該裝置的公網IP>`，從 smartdns 日誌撈出未涵蓋的網域。
