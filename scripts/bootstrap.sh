#!/usr/bin/env bash
#
# bootstrap.sh — 建立 generated/ 底下自動產生類設定檔的佔位版本，供全新 checkout 使用。
# 內容都是安全的預設值：DoT 關閉、不改寫解析、白名單為空。
# 可重複執行，已存在的檔案不會被覆蓋。
#
# 一般用 ./nfhh bootstrap 呼叫。

set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

seed() {  # $1 = 路徑, $2 = 內容
    if [[ -e "$1" ]]; then
        echo "  跳過（已存在）  ${1#$ROOT/}"
    else
        mkdir -p "$(dirname "$1")"
        printf '%s\n' "$2" > "$1"
        echo "  已建立          ${1#$ROOT/}"
    fi
}

echo "建立佔位設定檔："

seed "$ROOT/generated/smartdns/dot.conf" \
'# DoT 尚未啟用。啟用：./nfhh cert
# 本檔必須存在（可為空），smartdns.conf 以 conf-file 引入。'

seed "$ROOT/generated/smartdns/platforms.conf" \
'# 尚未產生。執行 ./nfhh apply 會依 config/smartdns/domain-set/*.list 填入 domain-set 宣告。'

seed "$ROOT/generated/smartdns/dynamic-ip.conf" \
'# 尚未產生。執行 ./nfhh apply 會依當下的對外 IP 填入內容。'

seed "$ROOT/generated/smartdns/dynamic-ip-lan.conf" \
'# 尚未產生。執行 ./nfhh apply 會依當下的本機 LAN IP 填入內容。'

seed "$ROOT/generated/nginx/sni-allow.conf" \
'# 尚未產生。執行 ./nfhh apply 會依 config/smartdns/domain-set/*.list 填入允許的 SNI。
# 空的代表 proxy 不轉發任何目的地。'

seed "$ROOT/generated/nft/clients.nft" \
'# 白名單為空。由控制平面依 SQLite 內容維護，勿手改。'

cat <<'EOF'

完成。接下來照 docs/SETUP.md 走，或直接：
  1. sudo nft -f config/nft/nfhh.nft           # 建立防火牆表（一定要在 up 之前）
  2. ./nfhh up
  3. ./nfhh apply                              # 填入對外 IP
  4. sudo cp deploy/nfhh-*.{service,timer,path} /etc/systemd/system/
     sudo mkdir -p /etc/systemd/system/docker.service.d
     sudo cp deploy/docker.service.d/10-nfhh-firewall.conf /etc/systemd/system/docker.service.d/
     sudo systemctl daemon-reload
     sudo systemctl enable --now nfhh-firewall.service nfhh-sync-ip.timer nfhh-cert.path
  5. ./nfhh cert                               # 啟用 DoT（需先有 acme.sh 憑證）

順序不能反：smartdns 與 nginx 是 host network，容器一起來就直接綁 53/443/853；
防火牆表還沒建就先 up，這幾秒鐘會是對全網開放的解析器。./nfhh up 也會先確認表存在才啟動。
EOF
