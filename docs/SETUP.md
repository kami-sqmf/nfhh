# SETUP.md — 首次架設

**只需要做一次的事都在這裡。** 已經在跑的機器不必再看這份 —— 日常操作見 [README.md](../README.md)，不可更動的約束見 [DECISIONS.md](DECISIONS.md)。

每一步都可以獨立驗證，請依序做完，不要跳著做。

| 步驟 | 做什麼 | 何時可以先跳過 |
|---|---|---|
| [0](#0-取得專案並建立佔位設定) | 取得專案、建立佔位設定與 `.env` | 不可跳過 |
| [1](#1-套用防火牆-acl) | 套用 nft 白名單 ACL | 不可跳過 |
| [2](#2-路由器-port-forward) | 路由器 port forward | 不可跳過 |
| [3](#3-啟動容器) | 啟動三個容器 | 不可跳過 |
| [4](#4-安裝開機持久化需-root) | systemd unit，開機持久化 | 不重開機就先不會出事 |
| [5](#5-控制平面首次啟用passkey-管理面板) | 控制平面首次啟用 | 只想跑網路層時可跳過 |
| [6](#6-驗證碼集中顯示email-worker) | 驗證碼集中顯示（Email Worker） | 不需要集中驗證碼時可跳過 |
| [6.5](#65-新增一個平台) | 新增一個平台（四層都要做） | 只用預設的 Netflix ／ Disney+ 時可跳過 |
| [7](#7-啟用-dot) | 啟用 DoT，讓手機填網域名 | 全部裝置都能接受填 IP 時可跳過 |

> [!CAUTION]
> **第 1 步（防火牆）一定要早於第 2 步（port forward）。**
> 顛倒過來，`:53` 會有一段時間是對全網際網路開放的 open resolver。

## 前置需求

| 項目 | 需求 |
|---|---|
| Docker + Docker Compose | 三個服務都跑在容器裡 |
| `nftables` | 白名單 ACL，於主機層執行 |
| 路由器可設 port forward | TCP + UDP `53`、TCP `443`、TCP `853` → 本機 |
| 對外固定 IPv4 | 電視類裝置只能填 IP 字面值，換 IP 就得逐台手動改 |
| 本機 LAN IP 有 DHCP 保留 | port forward 綁的是這個位址，它一變全斷 |
| Cloudflare 管理的網域 | 面板走 Tunnel、DoT 走 CNAME、驗證碼走 Email Routing |
| acme.sh 已在簽發萬用憑證 | DoT 用，第 7 步才需要 |

前兩項「對外固定 IPv4」與「DHCP 保留」的理由見 [README.md](../README.md) 的「主機運行需求」一節。

## 0. 取得專案並建立佔位設定

```bash
cd /opt/nfhh && ./nfhh bootstrap
```

`generated/` 底下的檔案**不進版控** —— 內容是機器當下的狀態，進版控只會讓 `git status` 永遠是髒的；`generated/nft/clients.nft` 更含家人所在網路的公網 IP，屬於個資。`bootstrap` 建立的是安全的預設值：DoT 關閉、不改寫解析、白名單為空。

> [!NOTE]
> 不先跑這步，smartdns 會因為 `conf-file` 指向不存在的檔案而啟動失敗。
> `./nfhh up` 偵測到 `generated/` 不存在時會自動代跑。

接著建立 `.env`：

```bash
cp .env.example .env && printf 'NFHH_MAIL_SECRET=%s\n' "$(openssl rand -hex 32)" >> .env
```

`.env.example` 裡**唯一必填的是 `NFHH_DOMAIN`** —— 面板、DoT、轉發信箱三個子網域（`dnf.` ／ `dns.` ／ `share.`）都由它衍生，compose 會在啟動時代入。其餘密鑰（Resend、Cloudflare）等對應章節用到時再填，留空只會停用該功能，不影響啟動。

> [!IMPORTANT]
> `.env` **不進版控**：它同時含密鑰與自有網域。

## 1. 套用防火牆 ACL

```bash
sudo nft -f /opt/nfhh/config/nft/nfhh.nft
```

> [!CAUTION]
> **套用順序不可顛倒：先 `nft -f`，再去路由器開 port forward。**
>
> `:53` 和 `:443` 會對整個網際網路暴露，唯一的防線是 nft 白名單：
>
> - 沒有它，`:53` 就是 open resolver → 會被拿去做 DNS 放大攻擊，你的 IP 會進黑名單
> - 沒有它，`:443` 就是免費的 Netflix 跳板（SNI 白名單只限制**去哪**，不限制**誰能用**）

驗證：

```bash
sudo nft list table inet nfhh
```

## 2. 路由器 port forward

確認上一步的規則真的在了，再開這些：

| 埠 | 協定 | 目的 |
|---|---|---|
| `53` | TCP + UDP | DNS |
| `443` | TCP | SNI proxy |
| `853` | TCP | DoT（第 7 步才需要，可先跳過） |

全部指向本機 LAN IP（目前是 `192.168.27.15`）。

> [!WARNING]
> **IPv6 入站規則不要綁完整位址。**
>
> 本機 IPv6 是 SLAAC + EUI-64：**interface ID 固定**（由網卡 MAC 推導），**但 `/64` prefix 會隨 PPPoE 重撥變動**。照完整位址開放入站，重撥後那條規則會指向不存在的位址。盡量改用「裝置」或 interface ID 指定。

## 3. 啟動容器

```bash
cd /opt/nfhh && ./nfhh up
```

容器起來之後產生衍生設定：

```bash
cd /opt/nfhh && ./nfhh apply
```

`apply` 會依 `config/smartdns/domain-set/*.list` 與當下的對外 IP 產生所有衍生設定。之後用 `./nfhh status` 確認三個容器都起來、`:53` 與 `:443` 都在聽。

## 4. 安裝開機持久化（需 root）

這台機器**每週二 03:00 自動重開機**（`scheduled-reboot.timer`），但：

- **nft 規則是記憶體狀態，重開機就消失** —— 而路由器的 port forward 會留著。結果是 `:53` 變成毫無保護、對全網際網路開放的 open resolver。
- **白名單條目同樣消失** —— 所有樓層在週二早上一起斷線。

`deploy/` 底下的 unit 就是解這個的。

```bash
sudo cp /opt/nfhh/deploy/nfhh-*.{service,timer,path} /etc/systemd/system/ && sudo mkdir -p /etc/systemd/system/docker.service.d && sudo cp /opt/nfhh/deploy/docker.service.d/10-nfhh-firewall.conf /etc/systemd/system/docker.service.d/ && sudo systemctl daemon-reload && sudo systemctl enable --now nfhh-firewall.service nfhh-sync-ip.timer
```

| Unit | 作用 |
|---|---|
| `nfhh-firewall.service` | 開機載入 nft 規則與白名單。排在 `docker.service` **之前**，確保容器綁 `:53` 時 ACL 已就位 |
| `docker.service.d/10-nfhh-firewall.conf` | Docker 的 drop-in：`Requires=` 防火牆 unit，且啟動前確認 `inet nfhh` 表存在。防火牆載入失敗時 Docker **不會**啟動 |
| `nfhh-sync-ip.timer` | 每 5 分鐘檢查出口 IP，變動時重新產生設定並重載 smartdns |
| `nfhh-sync-ip.service` | 上面 timer 實際執行的工作 |
| `nfhh-cert.path` | 監看 acme.sh 憑證續期（第 7 步啟用） |
| `nfhh-cert.service` | 上面 path 觸發的工作 |

> [!WARNING]
> 這是刻意的 fail-closed：nft 規則載入失敗時整套服務（含管理面板）都不會起來。
> 用 `systemctl status nfhh-firewall.service` 看原因、修好後
> `sudo systemctl restart nfhh-firewall.service && sudo systemctl reset-failed docker.service && sudo systemctl start docker.service`
> （是 `restart` 不是 `start`：這個 unit 是 `RemainAfterExit=yes` 的 oneshot，
> 表被刪掉後它仍是 active，`start` 什麼都不會做。多了 `reset-failed` 是因為
> docker.service 設了 `Restart=always`／`RestartSec=2`／`StartLimitBurst=3`／
> `StartLimitIntervalSec=60`—— `ExecStartPre` 失敗一樣算一次重試，nft 表消失時
> Docker 會在幾秒內燒完 3 次配額並進入 `failed`，之後 60 秒內手動
> `systemctl start` 都會被拒絕：「start request repeated too quickly」）。
>
> 這條相依是雙向的：`stop`／`restart` `nfhh-firewall.service` 現在也會連帶
> 停／重啟 Docker（nft 規則本身不會因此消失，理由見 docs/DECISIONS.md）。
> 只是要讓改過的規則生效，不要 `restart` 這個 unit（那會連 Docker 一起重啟），用：
> `cat /opt/nfhh/config/nft/nfhh.nft /opt/nfhh/generated/nft/clients.nft | sudo nft -f -`
> —— 一個交易把整張表換掉並補回白名單（`nfhh.nft` 檔頭的「先宣告再 delete」讓它可以
> 重複套用；只套 `nfhh.nft` 會把動態白名單清空，家人會被擋在外面直到面板下次寫入）。
> 先 `sudo nft -c -f /opt/nfhh/config/nft/nfhh.nft` 可以只檢查語法不套用。
>
> 緊急時要解除這條相依（例如要單獨除錯 Docker、暫時不想連動防火牆）：
> `sudo rm /etc/systemd/system/docker.service.d/10-nfhh-firewall.conf && sudo systemctl daemon-reload`。

<details>
<summary>驗證 drop-in 真的擋得住（維護時段、需要 console 進入方式）</summary>

> [!CAUTION]
> 這段會停掉 Docker 並刪除正式的 nft 表幾十秒：所有樓層的 DNS／proxy 會斷。
> 順序是**先停 Docker 再刪表**（刪表瞬間 `:53` 若還開著就是 open resolver）。
> 整段用 `trap` 包起來，連線中斷或 shell 結束時 trap 會自動復原。

```bash
set +e
trap 'sudo systemctl restart nfhh-firewall.service; sudo systemctl reset-failed docker.service; sudo systemctl start docker.service; echo "已復原：$(systemctl is-active nfhh-firewall.service docker.service | tr "\n" " ")"' EXIT
sudo systemctl daemon-reload
echo "requires/after: $(systemctl show docker.service -p Requires -p After | grep -c nfhh-firewall)"   # 預期 2
sudo systemctl stop docker.service
sudo nft delete table inet nfhh
sudo systemctl start docker.service; echo "docker: $(systemctl is-active docker.service)"          # 預期 start 報錯，is-active 印出 activating、failed 或 inactive（docker 會自己重試幾次）
echo "public listeners: $(sudo ss -ltunp | grep -vE '127\.0\.0\.|\[::1\]' | grep -cE '[:.](53|443|853)\s')"   # 預期 0（loopback 的 stub resolver 不算）
sudo systemctl restart nfhh-firewall.service && sudo nft list table inet nfhh >/dev/null && sudo systemctl reset-failed docker.service && sudo systemctl start docker.service
systemctl is-active docker.service nfhh-firewall.service                                            # 預期兩行 active
systemctl is-active --quiet docker.service && systemctl is-active --quiet nfhh-firewall.service && trap - EXIT
```

</details>

> [!IMPORTANT]
> unit 檔內的路徑是寫死的 —— **systemd 不吃 `.env`**。專案不在 `/opt/nfhh`、或執行使用者不叫 `nfhh` 時，複製過去後要改 `ExecStart` 與 `User`；`nfhh-cert.path` 監看的憑證目錄同理，見第 7 步。

`nfhh-cert.path` 先不要 enable —— 等第 7 步 DoT 真的通了再開。

## 5. 控制平面首次啟用（Passkey 管理面板）

面板是家人自助把「目前所在網路」加進白名單的唯一入口，也是驗證碼的集中顯示處。

1. **Cloudflare Zero Trust 後台**新增 public hostname：`dnf.example.com` → `http://localhost:8081`

   本機 cloudflared 是 token 模式，ingress 規則只能在後台設，沒有本地設定檔。

2. 啟動容器：

   ```bash
   cd /opt/nfhh && ./nfhh up control
   ```

3. 取得一次性註冊碼：

   ```bash
   docker logs nfhh-control 2>&1 | grep -A2 一次性
   ```

4. 手機開 `https://dnf.example.com`，輸入帳號與一次性碼，註冊第一把 Passkey。

一次性碼用完即失效，且**只有在系統還沒有任何帳號時才會發**。沒有這道關卡，面板一上線第一個找到它的人就能註冊成管理員。

> [!CAUTION]
> **只有一把 passkey 時，那台裝置遺失或重置就再也登不進面板。**
> 請立刻在**第二台裝置**上再註冊一把，面板內有按鈕。

帳號建好之後，面板的其餘功能（邀請家人、角色權限、裝置遺失的救援、白名單同步機制）全部寫在 [CONTROL.md](CONTROL.md)，這裡不重複。

## 6. 驗證碼集中顯示（Email Worker）

串流平台的同戶驗證碼原本經 Cloudflare Email Routing 轉發給每位家人。問題是各自收到就各自去驗證，**同戶裝置反覆被改到別層樓，最後所有人輪流被鎖在外面**。改成集中顯示在面板，才能協調「這次由誰驗證」。

```
平台寄信 → Cloudflare Email Routing → Email Worker
                                          │
                                          ├─1─→ POST 面板 /api/mail/ingest ──→ 回 forward_to
                                          └─2─→ 依 forward_to 轉發給家人（＋管理員後備）
```

**為什麼用 Worker 而不是讓面板收信**：Worker 是推送式的，不必存信箱密碼、沒有輪詢延遲。`app/cloudflare/email-worker.js` 刻意寫成無 import、無 npm 相依的單一檔案，直接貼進 Dashboard 的 Worker 編輯器即可，不必裝 wrangler。

### 部署

1. **Workers & Pages → Create → Worker**，貼上 `app/cloudflare/email-worker.js` 並部署。

2. 該 Worker 的 **Settings → Variables and Secrets**：

   | 名稱 | 值 | 類型 |
   |---|---|---|
   | `PANEL_ENDPOINT` | `https://dnf.example.com/api/mail/ingest` | 一般變數 |
   | `PANEL_SECRET` | 與 `.env` 的 `NFHH_MAIL_SECRET` 相同 | **Secret** |
   | `FALLBACK_TO` | 管理員一人的信箱 | 一般變數 |
   | `FORWARD_MAP` | 面板停機時的退路名單，見下 | **Secret**（含家人個資） |

3. **Email → Email Routing → Routes**：把規則改成 `Send to a Worker` 指向這支。

> [!WARNING]
> **其他家人的轉發規則要刪掉**，否則信仍會各自送達，問題依舊。

`FALLBACK_TO` 保留是刻意的：Worker 或面板萬一壞掉，管理員仍收得到驗證碼，不會整組人被鎖在外面卻拿不到碼。它**永遠會被加進轉發名單** —— 連面板判定「這封不用轉」時也一樣，篩選器設錯才看得見。

Worker 是**先推送再轉發**：轉發名單由面板決定，只有它解析得到內文，關鍵字才比對得了。面板**不可用**（逾時、DNS、面板停機、5xx 含端點未啟用的 503、408／429）時 Worker 退回 `FORWARD_MAP` 照送 —— 面板掛掉絕不能讓信轉不出去；面板**拒收**（其餘 4xx：401 密鑰不符、422 解析失敗）則只轉 `FALLBACK_TO`，不走 `FORWARD_MAP`。詳見 [CONTROL.md](CONTROL.md) §2.5。

> [!IMPORTANT]
> `FORWARD_MAP` 是那條退路唯一的名單來源，請保持它與面板「轉發收件人」頁同步。
> 清空等於面板一掛，家人就收不到碼。

### Cloudflare API token（轉發位址）

面板要讀轉發收件人的驗證狀態，並在登記邀請時**建立目的地位址**（順帶寄出驗證信）。兩件事都走同一個 token，放在 `.env` 的 `CF_API_TOKEN` 與 `CF_ACCOUNT_ID`。

需要的是**帳戶層級**的 `Email Routing Addresses`，而且要**讀 + 寫**：

| 權限 | 少了會怎樣 |
|---|---|
| Read | 「已驗證／尚未驗證」永遠顯示「未查詢」 |
| **Write** | 「重新發送驗證信」與「登記邀請時自動建位址」直接不能用 |

> [!TIP]
> 拿 `/user/tokens/verify` 檢查這種窄權限 token 會得到「Invalid API Token」—— 那支端點自己是 user 層級的。要確認 token 能不能用，直接打 `GET /accounts/{id}/email/routing/addresses`。

留空的話面板不會壞，只是那兩個功能停用，位址得到 Cloudflare 儀表板自己建。

### 安全設計

- `/api/mail/ingest` 用共用密鑰認證（機器對機器，Worker 做不了 WebAuthn），定時比對
- **密鑰未設定時整個端點停用**（fail-closed）。能寫進去的人就能顯示假驗證碼騙家人去驗證，寧可整個關掉也不留一個誰都能寫的洞
- 預設保留 14 天（`NFHH_MAIL_KEEP_DAYS`）。驗證碼時效很短，留著只是徒增外洩面

原始 HTML 供檢視（純文字剝掉排版後難判斷是不是官方信），但那是外部內容，防護兩層：`sandbox=""` 的 iframe（空值 = 全部限制生效，獨立來源、不能執行 script），加上注入 CSP `default-src 'none'` 擋掉所有遠端資源。預設不載入遠端圖片是防追蹤像素洩漏開信時間與 IP，按鈕可放行。

驗證碼的抽取規則，以及它是怎麼從真實信件調出來的，見 [CONTROL.md](CONTROL.md)。

### 推送通知

驗證碼一到就推到家人手機上，不必一直回面板重新整理。**伺服端零設定** —— VAPID 金鑰在第一次推送時自己產生並存進 DB，沒有要註冊的服務、沒有要填的金鑰。

唯一的前提是面板走 HTTPS，而它本來就走 Tunnel。家人那邊要做的事分兩種：

| 裝置 | 步驟 |
|---|---|
| Android Chrome | 面板裡按「開啟通知」，允許權限 |
| **iPhone ／ iPad** | **必須先「加入主畫面」**，再從主畫面的圖示打開面板才按得了 |

> [!WARNING]
> **iOS 的一般 Safari 分頁完全沒有推送能力。** 這不是權限問題 —— `PushManager` 那個 API 根本不存在。Apple 從 iOS 16.4 引進 Web Push 至今沒有放寬過（Safari 18.4 的 Declarative Web Push 拿掉的是 service worker 的需求，不是主畫面的需求）。面板偵測到這個情況會顯示加入步驟。

> [!WARNING]
> 加入主畫面時那顆**「開啟為網頁 App」開關必須是開的**。關掉的話加出來的是普通書籤，推送會**靜默失敗** —— 不報錯、不跳權限、什麼都沒有。iOS 26 起預設是開的，更舊的系統要自己確認。

驗證碼**直接顯示在通知內文**，因此也會出現在鎖定畫面。酬載走 RFC 8291 端對端加密，FCM ／ Apple 轉手的是它們自己也解不開的密文。不想讓碼出現在鎖定畫面的人，可以在個人設定把「新驗證碼」關掉。

## 6.5 新增一個平台

四層都要做。**網路層做完了信件層沒做，畫面上完全看不出來** —— 平台會正常出現在授權矩陣裡，只是那個平台的驗證碼永遠不會抵達。

### 1. 網路層（讓網域真的被代理）

```bash
# config/smartdns/domain-set/hbomax.list
# platform-name: HBO Max
# platform-color: #8A2BE2
hbomax.com
max.com
```

```bash
./nfhh apply
```

`apply-config.sh` 自動掃描 `*.list`，重新產生 smartdns 的 domain-set、address 規則與 nginx 的 SNI 白名單。不必手動註冊到任何地方。

> [!WARNING]
> **沒有 `# platform-name:` 那行就不會出現在面板。** 那是給 `*-cdn.list` 這類附屬清單用的 —— 它們照樣被代理，但不該在授權矩陣裡冒出第二個同名平台。

### 2. Cloudflare（兩件不同的事）

| 東西 | 誰管 |
|---|---|
| **收件位址**（`hbomax@share.example.com` → Worker） | **你自己**到 Email Routing → Routes 開。面板沒有 API 管得到 |
| **目的地位址**（家人的信箱） | 面板自動建立並寄驗證信 |

漏掉第一項的話信根本進不來，而面板那邊一切看起來都正常。

### 3. 面板

| 在哪 | 做什麼 |
|---|---|
| 轉發收件人 | 新平台旁邊「設定信箱」，填 `hbomax@share.example.com` |
| 設定 → 平台寄件者位址 | 填 `hbomax.com` 之類，讓分類優先靠寄件者 |
| 成員管理 | 把平台開給誰（或下次登記邀請時勾選） |

寄件者對應比信箱可靠：用 catch-all 收全部時，信箱推不出任何東西。

面板**不必重啟** —— 平台清單每次請求重讀目錄，而 domain-set 是 bind mount。

### 兩個會靜默出錯的地方

> [!CAUTION]
> **不要假設收件信箱的 local part 等於平台代號。** 代號來自檔名，信箱是你在 Cloudflare 自己取的，兩者沒有保證關係 —— `disneyplus.list` 配 `disney@` 就是真實踩過的例子。所以對應改成要 admin 明說（見 [CONTROL.md](CONTROL.md) §6）；沒設的平台在登記邀請時會被跳過並在回應裡說明，不會再猜。

> [!CAUTION]
> **不要改既有 `.list` 的檔名。** 代號 = 檔名 = `user_platforms` 的鍵。改名之後新信歸到新代號，而所有人身上掛的還是舊代號的授權 —— **所有人會安靜地失去那個平台的驗證碼**，畫面上不會有任何錯誤。要改顯示名就改 `# platform-name:`，那不影響代號。

## 7. 啟用 DoT

讓手機填網域名而非 IP。手機的 DNS 若填 IP 字面值，PPPoE 一重撥換 IP 就得逐台重設。DoT（DNS over TLS）的主機名欄位收的是**網域名**，DDNS 一更新所有裝置自動跟上。

### 憑證來源：沿用既有的萬用憑證

acme.sh 已經在管理 `*.example.com` 這張憑證（實測有效期至 2026-09-04，續期正常），它涵蓋 `dns.example.com`，不必另外簽發。

> [!WARNING]
> **已知取捨**：萬用憑證的私鑰會被放進一個對公網監聽的服務（smartdns）。若 smartdns 遭入侵，攻擊者取得的憑證可冒充**所有** `*.example.com` 服務（music、wol、frigate、面板本身）。
>
> 想收斂風險就改簽一張只含 `dns.example.com` 的憑證，把 `.env` 的 `NFHH_CERT_DIR` 指過去即可。

**刻意不使用 `acme.sh --install-cert`**：acme.sh 每個網域只能有一組部署設定，而 `*.example.com` 已經有一組在派送給 music、wol、frigate，再跑一次會覆寫掉它、弄壞那些服務的憑證部署。改用單向複製，完全不碰 acme.sh 的狀態。

### 步驟

**1. Cloudflare** 新增 CNAME：

| 欄位 | 值 |
|---|---|
| Name | `dns` |
| Target | 你現有的 DDNS 名稱，例如 `home.example.com` |
| Proxy | **DNS only（灰雲）** |

必須是灰雲。Cloudflare 的代理只處理 HTTP，載不了 `:853` 的 DoT，開成橘雲會直接不通。用 CNAME 指向現有的 DDNS 名稱，就不必在 ddns-go 多設一組。

**2. 路由器** 加開 port forward：TCP `853` → `192.168.27.15`。

**3. 續期自動重載**（需 root，只需做一次）：

```bash
sudo cp /opt/nfhh/deploy/nfhh-cert.{path,service} /etc/systemd/system/ && sudo systemctl daemon-reload && sudo systemctl enable --now nfhh-cert.path
```

> [!IMPORTANT]
> `nfhh-cert.path` 裡的 `PathChanged=` 預設是 `/etc/ssl/*.example.com_ecc/`，複製過去後要改成自己的網域（或 `.env` 的 `NFHH_CERT_DIR` 所指的目錄）—— **systemd 不會代入 `.env`**。`sync-cert.sh` 與 compose 兩邊則會自己讀。

`nfhh-cert.path` 監看 acme.sh 的憑證檔，續期後觸發 `sync-cert.sh` 重啟 smartdns —— 憑證是直接掛載的，但 smartdns 只在啟動時載入，不重啟會一直用舊的直到過期。

**4. 手機設定**

面板內建「連線教學」區塊，分 Android、iPhone ／ iPad、電視三頁，會自動帶入目前的網域名與出口 IP，並在該網路尚未授權時提示。家人註冊完直接照著做即可。

| 系統 | 做法 |
|---|---|
| Android | 設定 → 網路和網際網路 → 私人 DNS → 指定主機名稱 → `dns.example.com` |
| iOS ／ iPadOS | 面板「連線教學 → iPhone ／ iPad」下載 `.mobileconfig` 描述檔後安裝 |
| 電視與其他 | DNS 手動填出口 IP。⚠️ 重撥換 IP 後要重設 |

**iOS 描述檔的兩個實作重點：**

1. **刻意不填 `ServerAddresses`** —— 留空時 iOS 會用網路提供的 DNS 去解析 `ServerName`。填了就等於又把 IP 寫死，動態 IP 一變就失效，DoT 就白做了。
2. **下載用 `location.href` 而非 `fetch`** —— iOS 必須由 Safari 直接接收回應才會跳出描述檔安裝提示。也因此**必須用 Safari 開面板**，其他瀏覽器下載的描述檔裝不起來。

面板會依 `dot_ready`（實際檢查 smartdns 載入的設定有無 `bind-tls`）決定是否開放下載 —— DoT 還沒啟用就給描述檔，對方裝了只會整台不能上網。

### 失敗會自動退回

`sync-cert.sh` 部署完會檢查 `:53` 是否真的恢復，沒有就自動把 DoT 退回停用並重啟。憑證自動續期，這種失敗會發生在沒人看著的半夜，不能讓它把整個家的網路帶下去。另有 `flock` 序列化、部署前用 openssl 比對憑證與私鑰成對、寫暫存檔再 rename。

### 驗證

```bash
kdig -d @dns.example.com +tls www.netflix.com
```

或從已授權的網路：

```bash
nslookup www.netflix.com dns.example.com    # 應回 203.0.113.10
```

> [!NOTE]
> 憑證的掛載方式與檔案權限有兩個會讓 smartdns 直接起不來的坑，見 [DECISIONS.md](DECISIONS.md) 的「憑證直接掛載，不複製」與「smartdns 沒有 CAP_DAC_OVERRIDE」。

啟用之後把憑證續期的自動重載也打開：

```bash
sudo systemctl enable --now nfhh-cert.path
```

## 附錄：cloudflared token 的存放

面板經既有的 Cloudflare Tunnel 對外。token **不寫在 `ExecStart` 上** —— 那會同時經由 644 的 unit 檔與 `ps aux` 外洩，而拿到 token 的人可以自己跑一個 cloudflared 冒充這條 tunnel，把面板流量接走。

現況（已套用）：

```
ExecStart=/usr/bin/cloudflared --no-autoupdate tunnel run --token-file /etc/cloudflared/tunnel.token
```

token 存在 `root:root 600` 的 `/etc/cloudflared/tunnel.token`，目錄本身 700。

> [!NOTE]
> 刻意**不**改成 credentials file 加本地 `config.yml`：這條 tunnel 是 Cloudflare 端遠端管理的（ingress 規則在 Dashboard），換成 credentials file 就得把那些規則搬到本地，面板隨時可能因為漏抄一條而斷線。`--token-file` 完全不動 tunnel 語意，只換 token 的讀取來源，安全效果一樣。
