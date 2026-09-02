# 無頭截圖與設計檢視

主機沒有瀏覽器也沒有顯示器，而登入後的 14 個畫面又需要 passkey 才進得去。
這兩支腳本用 CDP 驅動一個一次性的無頭 Chromium，並在頁面載入**之前**注入
`fetch` 樁，讓 app 拿假資料開起來 —— 伺服器完全不知情，也不需要任何憑證。

順便會收集 console 錯誤，那是 CI 之外唯一能驗到「前端有沒有在執行時炸掉」
的地方。

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

步驟支援 `dark`（模擬 `prefers-color-scheme`，Chrome 的 `--force-dark-mode`
對宣告了 `color-scheme` 的頁面**不會**翻轉這個 media query，只能走 CDP）、
`goto`、`click`（比對按鈕/連結的文字）、`type`、`shot`、`wait`。

## 注意

`mock.js` 的假資料是照設計稿的情境編的（含 v5 之前 `verified` 為 NULL 的
舊信，用來檢查「無驗證資訊」不會被畫成紅色）。改動資料模型時要一起更新，
否則截出來的畫面會跟真實情況不符。
