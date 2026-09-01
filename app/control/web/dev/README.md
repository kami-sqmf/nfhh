# 無頭截圖與設計檢視

主機沒有瀏覽器也沒有顯示器，而登入後的 14 個畫面又需要 passkey 才進得去。
這兩支腳本用 CDP 驅動一個一次性的無頭 Chromium，並在頁面載入**之前**注入
`fetch` 樁，讓 app 拿假資料開起來 —— 伺服器完全不知情，也不需要任何憑證。

順便會收集 console 錯誤，那是 CI 之外唯一能驗到「前端有沒有在執行時炸掉」
的地方。

## 用法

```bash
docker run -d --name nfhh-shot --network host -v /tmp/shots:/out --user root \
  --entrypoint chromium-browser zenika/alpine-chrome \
  --headless --no-sandbox --disable-gpu --hide-scrollbars \
  --remote-debugging-port=9333 --remote-debugging-address=0.0.0.0 \
  --disable-dev-shm-usage about:blank

cp dev/*.js /tmp/shots/ && cd /tmp/shots
MOCK=/tmp/shots/mock.js bun shoot.js 'https://dnf.example.com/' '[
  {"dark":false,"goto":true,"shot":"home"},
  {"click":"白名單","shot":"allow"},
  {"click":"展開","shot":"allow-queries"}
]'

docker rm -f nfhh-shot
```

步驟支援 `dark`（模擬 `prefers-color-scheme`，Chrome 的 `--force-dark-mode`
對宣告了 `color-scheme` 的頁面**不會**翻轉這個 media query，只能走 CDP）、
`goto`、`click`（比對按鈕/連結的文字）、`type`、`shot`、`wait`。

## 注意

`mock.js` 的假資料是照設計稿的情境編的（含 v5 之前 `verified` 為 NULL 的
舊信，用來檢查「無驗證資訊」不會被畫成紅色）。改動資料模型時要一起更新，
否則截出來的畫面會跟真實情況不符。
