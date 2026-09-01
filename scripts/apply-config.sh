#!/usr/bin/env bash
#
# apply-config.sh — 由「平台網域清單 + 當下的對外/內網 IP」重新產生所有衍生設定。
#
# 改完 config/smartdns/domain-set/*.list 之後跑這支，變更才會生效。
# 一般用 ./nfhh apply 呼叫。
#
# 唯一資料來源是 config/smartdns/domain-set/*.list，每個 .list 檔就是一個平台。
# 產生三份設定（全部落在 generated/，不進版控）：
#   generated/smartdns/platforms.conf     domain-set 宣告
#   generated/smartdns/dynamic-ip*.conf   address 規則（把網域指向本機）
#   generated/nginx/sni-allow.conf        SNI 白名單（proxy 允許轉發的目的地）
#
# 新增平台 = 丟一個 .list 檔進 config/smartdns/domain-set/ 再跑這支。停用 = 改名成 .disabled。
# ⚠️ 這支只管網路層。收件位址、平台信箱對應、成員授權見 docs/SETUP.md §6.5。
# 由 nfhh-sync-ip.timer 每 5 分鐘呼叫，追蹤 PPPoE 重撥造成的 IP 變動。
#
# 不需要 root（只寫專案目錄 + 呼叫 docker）。

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SETS="$ROOT/config/smartdns/domain-set"
OUT_PLATFORMS="$ROOT/generated/smartdns/platforms.conf"
OUT_WAN="$ROOT/generated/smartdns/dynamic-ip.conf"
OUT_LAN="$ROOT/generated/smartdns/dynamic-ip-lan.conf"
OUT_SNI="$ROOT/generated/nginx/sni-allow.conf"
LAN_IF="br0"

log() { echo "[apply-config] $*"; command -v logger >/dev/null && logger -t nfhh-sync-ip "$*" || true; }

# ── 取得 IP ───────────────────────────────────────────
LAN_IP="$(ip -4 -o addr show dev "$LAN_IF" 2>/dev/null | awk '{print $4}' | cut -d/ -f1 | head -1)"
if [[ ! "$LAN_IP" =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    log "取不到 $LAN_IF 的 IPv4，放棄本次更新"
    exit 0
fi

# 多來源輪替，避免單一服務故障造成誤判。取不到就整個放棄，不改設定。
WAN_IP=""
for url in "https://api.ipify.org" "https://ifconfig.me/ip" "https://ipv4.icanhazip.com"; do
    WAN_IP="$(curl -4 -s --max-time 8 "$url" 2>/dev/null | tr -d '[:space:]')" || true
    [[ "$WAN_IP" =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]] && break
    WAN_IP=""
done
[[ -n "$WAN_IP" ]] || { log "所有來源都取不到對外公網 IP，維持現狀不動"; exit 0; }

# ── 探索平台 ──────────────────────────────────────────
PLATFORMS=()
for f in "$SETS"/*.list; do
    [[ -e "$f" ]] || continue
    PLATFORMS+=("$(basename "$f" .list)")
done
[[ ${#PLATFORMS[@]} -gt 0 ]] || { log "domain-set/ 裡沒有任何 *.list，無事可做"; exit 0; }

# 清單內容的指紋，用來偵測「只改清單內容」的異動（平台名與 IP 都沒變時，
# 產生出來的檔案會一字不差，變動偵測會誤判為無事發生）。
LIST_DIGEST="$(cat "$SETS"/*.list | sha256sum | cut -c1-16)"

HEADER="# ⚠️ 自動產生 —— 由 scripts/apply-config.sh 覆寫，手動修改會消失。
# 要改內容請改 config/smartdns/domain-set/*.list 然後跑 ./nfhh apply。
# 平台：${PLATFORMS[*]}
# 清單指紋：$LIST_DIGEST
# WAN_IP = $WAN_IP   (對外公網 IP，其他樓層連這裡)
# LAN_IP = $LAN_IP   (本機 LAN IP，本層裝置連這裡，避開 NAT hairpin)"

# ── 產生：domain-set 宣告 ─────────────────────────────
gen_platforms() {
    printf '%s\n\n' "$HEADER"
    for p in "${PLATFORMS[@]}"; do
        printf 'domain-set -name %s -type list -file /etc/smartdns/domain-set/%s.list\n' "$p" "$p"
    done
}

# ── 產生：address 規則（$1 = 該組要回的 IP）───────────
gen_rules() {
    printf '%s\n# 本檔回應的位址 = %s\n\n' "$HEADER" "$1"
    for p in "${PLATFORMS[@]}"; do
        printf 'address /domain-set:%s/%s\n' "$p" "$1"
        # 剝 AAAA（回 SOA），理由見 docs/DECISIONS.md
        printf 'address /domain-set:%s/#6\n' "$p"
    done
}

# ── 產生：nginx SNI 白名單 ────────────────────────────
# map 用 hostnames 模式，前綴一個點即可同時匹配該網域與所有子網域。
# 沒列進來的 SNI 落到 default（黑洞埠）。
gen_sni() {
    printf '%s\n\n' "$HEADER"
    for p in "${PLATFORMS[@]}"; do
        printf '# %s\n' "$p"
        grep -vE '^\s*(#|$)' "$SETS/$p.list" | tr -d '\r' | while read -r d; do
            printf '.%-28s $ssl_preread_server_name:443;\n' "$d"
        done
        printf '\n'
    done
}

# ── 只寫入有變動的 ────────────────────────────────────
write_if_changed() {  # $1 = 路徑, $2 = 內容 → 有寫入回傳 0
    if [[ -f "$1" ]] && [[ "$(cat "$1")" == "$2" ]]; then return 1; fi
    mkdir -p "$(dirname "$1")"
    printf '%s\n' "$2" > "$1"
    return 0
}

OLD_WAN="$(grep -oE '^# WAN_IP = [0-9.]+' "$OUT_WAN" 2>/dev/null | awk '{print $4}' || true)"
DNS_CHANGED=0; SNI_CHANGED=0
write_if_changed "$OUT_PLATFORMS" "$(gen_platforms)"     && DNS_CHANGED=1
write_if_changed "$OUT_WAN"       "$(gen_rules "$WAN_IP")" && DNS_CHANGED=1
write_if_changed "$OUT_LAN"       "$(gen_rules "$LAN_IP")" && DNS_CHANGED=1
write_if_changed "$OUT_SNI"       "$(gen_sni)"           && SNI_CHANGED=1

[[ $DNS_CHANGED -eq 1 || $SNI_CHANGED -eq 1 ]] || exit 0
log "設定已更新（WAN ${OLD_WAN:-無} → $WAN_IP，LAN $LAN_IP，平台 ${PLATFORMS[*]}）"

# ── 套用：nginx 用 reload，不中斷既有串流 ─────────────
if [[ $SNI_CHANGED -eq 1 ]]; then
    if docker exec nfhh-sniproxy nginx -t >/dev/null 2>&1 \
       && docker exec nfhh-sniproxy nginx -s reload >/dev/null 2>&1; then
        log "SNI 白名單已重載（graceful，不中斷播放中的串流）"
    else
        log "⚠️ nginx 重載失敗，SNI 白名單未生效。查 docker logs nfhh-sniproxy"
    fi
fi

# ── 套用：smartdns 只能重啟 ───────────────────────────
[[ $DNS_CHANGED -eq 1 ]] || exit 0
docker compose -f "$ROOT/docker-compose.yml" restart smartdns >/dev/null 2>&1 || true

# 重啟後確認 :53 真的回來（憑證有問題時 smartdns 會拒絕啟動）
for _ in $(seq 1 10); do
    sleep 1
    if docker ps --filter name=nfhh-smartdns --filter status=running -q | grep -q . \
       && ss -tuln 2>/dev/null | grep -qE ':53\s'; then
        log "smartdns 已重載，:53 正常"
        exit 0
    fi
done

# 最可能的元凶是 DoT 憑證，停用 DoT 再試一次
log "⚠️ smartdns 重啟後 :53 沒回來，嘗試停用 DoT 後重試"
cat > "$ROOT/generated/smartdns/dot.conf" <<'EOF'
# DoT 已由 apply-config.sh 自動停用 —— 重啟後 :53 沒有恢復。
# 排除問題後執行 ./nfhh cert 重新啟用。
EOF
docker compose -f "$ROOT/docker-compose.yml" restart smartdns >/dev/null 2>&1 || true
for _ in $(seq 1 10); do
    sleep 1
    ss -tuln 2>/dev/null | grep -qE ':53\s' && { log "已停用 DoT，:53 恢復正常"; exit 1; }
done
log "❌ 停用 DoT 後 :53 仍未恢復，需要人工介入"
exit 1
