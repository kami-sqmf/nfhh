#!/usr/bin/env bash
#
# sync-cert.sh — acme.sh 續期後重啟 smartdns 以重新載入憑證。
#
# 憑證不複製，compose 直接唯讀掛載 acme.sh 的目錄到容器的 /certs。
# 本腳本唯一的工作是重啟讓它重讀（smartdns 只在啟動時載入憑證）。
# 冪等：憑證未變動則直接跳過。
#
# 需要 root（讀 700 的來源目錄並操作 docker）。自動觸發：nfhh-cert.path
# 一般用 ./nfhh cert 呼叫。

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DOTCONF="$ROOT/generated/smartdns/dot.conf"

# .env 的網域設定（compose 會自己讀，腳本要自己載）
[[ -f "$ROOT/.env" ]] && set -a && . "$ROOT/.env" && set +a
NFHH_DOMAIN="${NFHH_DOMAIN:-example.com}"
CERT_DIR="${NFHH_CERT_DIR:-/etc/ssl/*.${NFHH_DOMAIN}_ecc}"
OWNER="$(stat -c '%U' "$ROOT")"

log() { echo "[sync-cert] $*"; command -v logger >/dev/null && logger -t nfhh-cert "$*" || true; }

[[ $EUID -eq 0 ]] || { echo "需要 root：sudo $0" >&2; exit 1; }

# 序列化：手動執行與 nfhh-cert.path 觸發可能撞在一起
exec 9>/run/nfhh-cert.lock
flock -n 9 || { log "另一個實例正在執行，跳過本次"; exit 0; }

# docker 還沒就緒就什麼都別做
if ! docker ps --filter name=nfhh-smartdns -q 2>/dev/null | grep -q .; then
    log "nfhh-smartdns 容器不存在或 docker 尚未就緒，跳過"
    exit 0
fi

# ── 找到來源憑證 ──────────────────────────────────────
# 目錄名字面上就含星號（acme.sh 對萬用憑證的命名），要引號避免被 shell 展開
SRC=""
for cand in "$CERT_DIR" "/etc/ssl/*.${NFHH_DOMAIN}"; do
    [[ -f "$cand/fullchain.cer" ]] && { SRC="$cand"; break; }
done
[[ -n "$SRC" ]] || { log "找不到憑證來源目錄"; exit 1; }

FULLCHAIN="$SRC/fullchain.cer"
KEY=""
for k in "$SRC"/*.key; do [[ -f "$k" ]] && { KEY="$k"; break; }; done
[[ -n "$KEY" ]] || { log "在 $SRC 找不到私鑰"; exit 1; }

# ── 部署前驗證：過期與憑證/私鑰是否成對 ────────────────
if ! openssl x509 -in "$FULLCHAIN" -noout -checkend 0 >/dev/null 2>&1; then
    log "⚠️ 來源憑證已過期，不重啟。請確認 acme.sh 續期是否正常。"
    exit 1
fi
if [[ "$(openssl x509 -in "$FULLCHAIN" -noout -pubkey | openssl md5)" \
   != "$(openssl pkey -in "$KEY" -pubout | openssl md5)" ]]; then
    log "⚠️ 憑證與私鑰不成對，不重啟"
    exit 1
fi
NOT_AFTER="$(openssl x509 -in "$FULLCHAIN" -noout -enddate | cut -d= -f2)"

# 憑證比容器啟動時間舊 → 載入的已經是這一份，不必重啟
if [[ "${1:-}" != "--force" ]]; then
    started="$(docker inspect -f '{{.State.StartedAt}}' nfhh-smartdns 2>/dev/null || echo '')"
    if [[ -n "$started" ]]; then
        started_ts="$(date -d "$started" +%s 2>/dev/null || echo 0)"
        cert_ts="$(stat -c %Y "$FULLCHAIN")"
        if [[ "$cert_ts" -le "$started_ts" ]] && grep -q '^bind-tls' "$DOTCONF" 2>/dev/null; then
            log "憑證未變動且 DoT 已啟用中（有效至 $NOT_AFTER），無需重啟"
            exit 0
        fi
    fi
fi

# ── 健康檢查與退回 ────────────────────────────────────
smartdns_healthy() {
    for _ in $(seq 1 10); do
        sleep 1
        docker ps --filter name=nfhh-smartdns --filter status=running -q | grep -q . || continue
        ss -tuln 2>/dev/null | grep -qE ':53\s' && return 0
    done
    return 1
}

write_dot() {  # $1 = enabled | disabled
    if [[ "$1" == enabled ]]; then
        cat > "$DOTCONF" <<EOF
# DoT（DNS over TLS）—— 手機填 dns.${NFHH_DOMAIN}：
#   Android → 設定 / 網路 / 私人 DNS → 指定主機名稱
#   iOS     → 由管理面板下載 .mobileconfig 描述檔
#
# 憑證由 compose 唯讀掛載到 /certs。私鑰檔名取自實際找到的那支
# （檔名字面上可能含星號，是 acme.sh 對萬用憑證的命名慣例）。
bind-tls [::]:853
bind-cert-file /certs/fullchain.cer
bind-cert-key-file /certs/$(basename "$KEY")
EOF
    else
        cat > "$DOTCONF" <<'EOF'
# DoT 已停用 —— 上次啟用時 smartdns 無法啟動，已自動退回以保住 :53。
# 查 journalctl -t nfhh-cert 與 docker logs nfhh-smartdns。
EOF
    fi
    chown "$OWNER:$OWNER" "$DOTCONF"
}

# ── 執行 ──────────────────────────────────────────────
write_dot enabled
log "憑證有效至 $NOT_AFTER，重啟 smartdns 以重新載入"
docker restart nfhh-smartdns >/dev/null 2>&1 || true

if ! smartdns_healthy; then
    log "⚠️ smartdns 啟用 DoT 後起不來，自動退回停用狀態以保住 :53"
    write_dot disabled
    docker restart nfhh-smartdns >/dev/null 2>&1 || true
    smartdns_healthy \
        && log "已退回，:53 恢復正常。DoT 保持停用直到問題排除。" \
        || log "❌ 退回後 :53 仍未恢復，需要人工介入"
    exit 1
fi

if ss -tuln 2>/dev/null | grep -qE ':853\s'; then
    log "✅ :53 正常、DoT 於 :853 提供服務（憑證至 $NOT_AFTER）"
else
    log "⚠️ :53 正常但 :853 沒起來。查 docker logs nfhh-smartdns"
fi
