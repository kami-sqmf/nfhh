#!/usr/bin/env bash
#
# check-env.sh — 檢查 .env 會不會讓容器起不來、或讓功能靜默停用。
#
# 這份設定沒有「必填」欄位：app/control/src/main.rs 的 Config::from_env 每一項
# 都有預設值，打錯字不會讓程式爆掉，只會安靜地跑在 example.com 或把 Email、
# Cloudflare 整組關掉。所以這裡檢查的重點不是「有沒有填」，是「填的東西會不會
# 被讀到、讀到之後是不是你以為的值」。
#
# 唯讀：不啟動、不重啟、不改任何檔案。密鑰只印 sha256 指紋與長度，不露任何字元。
# 也刻意不 source .env —— 那等於執行裡面的內容，檢查工具不該有這種副作用。
#
# 一般用 ./nfhh check-env 呼叫。
#
# 離開碼：0 = 沒有會出錯的問題（可能仍有提醒）、1 = 有錯、2 = 連 .env 都不在

set -uo pipefail   # 刻意不用 -e：要一路檢查到底列出所有問題，而不是第一個錯就停

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
ENV_FILE="${1:-.env}"

# 參數給的是別的檔案時，第 6、7 段要跳過：docker compose 與執行中的容器
# 讀的永遠是專案根目錄的 .env，拿它們的結論去描述另一個檔案是錯的。
IS_LIVE=0
[[ "$(readlink -f "$ENV_FILE")" == "$ROOT/.env" ]] && IS_LIVE=1

ERRS=0
WARNS=0
SECT=0          # 本段落到目前為止的發現數，供段尾的「這段沒問題」判斷
err()  { echo "  ✗ $*"; ERRS=$((ERRS + 1)); SECT=$((SECT + 1)); }
warn() { echo "  ⚠ $*"; WARNS=$((WARNS + 1)); SECT=$((SECT + 1)); }
ok()   { echo "  ✓ $*"; }
note() { echo "    $*"; }
section() { echo; echo "── $* ──────────────────────────────"; SECT=0; }

# 密鑰一律只印指紋與長度，不露任何一個字元。指紋足以回答「跟我手上那把是不是
# 同一把」（自己 sha256 一次來比），而輸出可以安心貼進 issue 或截圖 ——
# 這個 repo 是公開的，腳本的輸出遲早會出現在某個公開的地方。
mask() {
    local v="$1"
    printf '指紋 %s…（共 %s 碼）' "$(printf '%s' "$v" | sha256sum | cut -c1-8)" "${#v}"
}

if [[ ! -f "$ENV_FILE" ]]; then
    echo "找不到 $ENV_FILE。複製範本後照著填：cp .env.example .env"
    exit 2
fi

echo "檢查 $ENV_FILE"

# ── 1. 檔案本身 ──────────────────────────────────────
section "檔案"

PERM="$(stat -c '%a' "$ENV_FILE")"
case "$PERM" in
    600|640|400) ok "權限 $PERM" ;;
    *) warn "權限 $PERM —— 裡面有四把密鑰，同機其他使用者讀得到。chmod 600 $ENV_FILE" ;;
esac

if git check-ignore -q "$ENV_FILE" 2>/dev/null; then
    ok "被 .gitignore 忽略"
else
    err ".env 沒有被 .gitignore 忽略 —— 密鑰隨時可能被 commit 出去"
fi
if git ls-files --error-unmatch "$ENV_FILE" >/dev/null 2>&1; then
    err "$ENV_FILE 已經進了版控。git rm --cached 之後，四把密鑰都要當作外洩、全部換掉"
fi

if grep -q $'\r' "$ENV_FILE"; then
    err "有 CRLF 行尾 —— 每個值都會多帶一個 \\r。token 會 401、網域會對不上，而且從輸出看不出來"
    note "修：sed -i 's/\\r$//' $ENV_FILE"
fi
if [[ "$(head -c3 "$ENV_FILE")" == $'\xef\xbb\xbf' ]]; then
    err "檔頭有 BOM —— 第一個鍵的名字會多帶三個看不見的位元組，compose 讀不到它"
fi
[[ -n "$(tail -c1 "$ENV_FILE")" ]] && warn "最後一行沒有換行 —— 有些解析器會整行吃掉"

# ── 2. 語法 ──────────────────────────────────────────
# compose 與 bash 對 .env 的解析規則不完全相同，而這個專案兩邊都會讀它：
# compose 讀它做變數替換，./nfhh 則是 `set -a && . .env`（真的 source）。
# 只在其中一邊成立的寫法就是坑，這段專抓那些。
section "語法"

declare -A SEEN=()
declare -A VAL=()
LINENO_=0
SYNTAX_OK=1
while IFS= read -r line || [[ -n "$line" ]]; do
    LINENO_=$((LINENO_ + 1))
    line="${line%$'\r'}"
    [[ -z "${line//[[:space:]]/}" ]] && continue
    [[ "${line#"${line%%[![:space:]]*}"}" == \#* ]] && continue

    if [[ "$line" == export\ * ]]; then
        err "第 $LINENO_ 行用了 export —— compose 會把鍵名認成「export XXX」而讀不到"
        SYNTAX_OK=0; continue
    fi
    if [[ "$line" != *=* ]]; then
        err "第 $LINENO_ 行不是 KEY=VALUE 也不是註解"
        SYNTAX_OK=0; continue
    fi

    k="${line%%=*}"; v="${line#*=}"
    if [[ ! "$k" =~ ^[A-Za-z_][A-Za-z0-9_]*$ ]]; then
        err "第 $LINENO_ 行的鍵名不合法：$k"
        SYNTAX_OK=0; continue
    fi
    [[ -n "${SEEN[$k]:-}" ]] && warn "$k 重複定義（第 ${SEEN[$k]} 行與第 $LINENO_ 行）—— 後面的靜默蓋掉前面的"
    SEEN[$k]=$LINENO_

    # 值層面的坑
    if [[ "$v" =~ ^[\"\'].*[\"\']$ ]]; then
        q="${v:0:1}"; v="${v:1:${#v}-2}"
        [[ "$q" == '"' ]] && warn "$k 用雙引號包住 —— compose 會展開裡面的 \$ 與跳脫序列，值可能不是字面上那樣"
    elif [[ "$v" == *[[:space:]]* ]]; then
        err "$k 的值含空白又沒有引號 —— compose 讀得進去，但 ./nfhh 的 . .env 會在這行失敗"
    fi
    if [[ "$v" == *'$('* || "$v" == *'`'* ]]; then
        err "$k 的值含指令替換 —— ./nfhh source .env 時會真的去執行它"
    fi
    if [[ "$v" != "${v%[[:space:]]}" ]]; then
        err "$k 的值尾端有空白 —— 這是 token 莫名 401 最常見的死法，肉眼完全看不出來"
    fi
    if [[ "$v" == *' #'* ]]; then
        warn "$k 的值裡有「 #」—— compose 當成行內註解切掉，bash 不會。兩邊會拿到不同的值"
    fi
    VAL[$k]="$v"
done < "$ENV_FILE"

(( SECT == 0 )) && ok "每一行都是合法的 KEY=VALUE"

get() { echo "${VAL[$1]:-}"; }
has() { [[ -n "${VAL[$1]:-}" ]]; }

# ── 3. 鍵名 ──────────────────────────────────────────
# 這份 .env 最容易錯的地方：容器裡的變數名跟 .env 裡的不一樣。
# compose 第 83、93、94 行做了改名對應，填錯邊不會有任何錯誤訊息，
# 只是 Email 與 Cloudflare 整組安靜地停用。
section "鍵名"

declare -A REPORTED=()
declare -A WRONG_SIDE=(
    [NFHH_RESEND_KEY]=RESEND_API_KEY
    [NFHH_CF_ACCOUNT]=CF_ACCOUNT_ID
    [NFHH_CF_TOKEN]=CF_API_TOKEN
)
for wrong in "${!WRONG_SIDE[@]}"; do
    right="${WRONG_SIDE[$wrong]}"
    if has "$wrong"; then
        REPORTED[$wrong]=1
        err "$wrong 是容器裡的名字，.env 要寫 $right —— compose 不會讀 $wrong，該功能會靜默停用"
    fi
done

KNOWN="NFHH_DOMAIN NFHH_CERT_DIR NFHH_RP_ID NFHH_ORIGIN NFHH_DOT_HOST NFHH_MAIL_FROM
NFHH_MAIL_DOMAIN NFHH_MAIL_AUTHSERV_ID NFHH_SKIP_FIREWALL_CHECK NFHH_AUDIT_KEEP_DAYS
NFHH_AUDIT_MAX_ROWS NFHH_MAIL_SECRET RESEND_API_KEY NFHH_INVITE_TEMPLATE
CF_ACCOUNT_ID CF_API_TOKEN"
for k in "${!VAL[@]}"; do
    [[ -n "${REPORTED[$k]:-}" ]] && continue   # 上面已經指名道姓過了，不重複罵
    grep -qw -- "$k" <<<"$KNOWN" || warn "$k 沒有被 compose 或程式讀到 —— 設了也不會生效"
done
(( SECT == 0 )) && ok "沒有拼錯邊的鍵"

# ── 4. 值 ────────────────────────────────────────────
section "值"

DOMAIN="$(get NFHH_DOMAIN)"
if [[ -z "$DOMAIN" ]]; then
    err "NFHH_DOMAIN 沒填 —— RP_ID、ORIGIN、DoT、寄件網域全部會退回 example.com，面板登不進去"
elif [[ "$DOMAIN" == example.com ]]; then
    err "NFHH_DOMAIN 還是範本的 example.com"
elif [[ ! "$DOMAIN" =~ ^[a-z0-9]([a-z0-9-]*[a-z0-9])?(\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)+$ ]]; then
    err "NFHH_DOMAIN=$DOMAIN 不像網域（別加 https:// 或結尾的點）"
else
    ok "NFHH_DOMAIN=$DOMAIN"
    note "衍生：dnf.$DOMAIN（面板）/ dns.$DOMAIN（DoT）/ share.$DOMAIN（轉發信箱）"
fi

# WebAuthn 的硬性條件：origin 的 host 必須等於 rp_id，或是它的子網域。
# 對不上時瀏覽器直接拒發 credential —— 面板會變成登不進去也註冊不了。
RP="$(get NFHH_RP_ID)"; ORIGIN="$(get NFHH_ORIGIN)"
[[ -z "$RP" && -n "$DOMAIN" ]] && RP="dnf.$DOMAIN"
[[ -z "$ORIGIN" && -n "$DOMAIN" ]] && ORIGIN="https://dnf.$DOMAIN"
if [[ -n "$ORIGIN" ]]; then
    if [[ "$ORIGIN" != https://* ]]; then
        err "NFHH_ORIGIN 必須是 https:// 開頭（現在：$ORIGIN）—— Passkey 只在安全來源可用"
    else
        host="${ORIGIN#https://}"; host="${host%%/*}"; host="${host%%:*}"
        if [[ "$host" == "$RP" || "$host" == *".$RP" ]]; then
            ok "RP_ID=$RP 與 ORIGIN=$ORIGIN 相符"
        else
            err "ORIGIN 的網域（$host）不等於 RP_ID（$RP）也不是它的子網域 —— 瀏覽器會拒絕發 Passkey"
        fi
        [[ "$ORIGIN" == */ ]] && err "NFHH_ORIGIN 結尾多了斜線 —— webauthn 的 origin 比對是字串相等，會對不上"
    fi
fi

MAIL_FROM="$(get NFHH_MAIL_FROM)"
if [[ -n "$MAIL_FROM" && ! "$MAIL_FROM" =~ ^[^@[:space:]]+@[^@[:space:]]+\.[a-z]+$ ]]; then
    err "NFHH_MAIL_FROM=$MAIL_FROM 不是合法信箱位址"
fi

AUTHSERV="$(get NFHH_MAIL_AUTHSERV_ID)"
if has NFHH_MAIL_AUTHSERV_ID; then
    if [[ -z "${AUTHSERV// /}" ]]; then
        err "NFHH_MAIL_AUTHSERV_ID 是空的 —— 沒有可信的驗證署名，所有進來的信都會被扣住"
    else
        ok "NFHH_MAIL_AUTHSERV_ID=$AUTHSERV"
        [[ "$AUTHSERV" != mx.cloudflare.net ]] && note "不是 Cloudflare Email Routing 的預設值，換過收信服務才該這樣"
    fi
fi

# 數字項：程式 parse 失敗會靜默吃預設值，超出範圍會靜默被夾。
# 兩種都不會有錯誤訊息，只有稽核行為跟你想的不一樣。
check_num() {  # $1=鍵 $2=下限 $3=上限 $4=預設
    local k="$1" v; v="$(get "$k")"
    [[ -z "$v" ]] && return
    if [[ ! "$v" =~ ^[0-9]+$ ]]; then
        err "$k=$v 不是數字 —— 程式會靜默退回預設值 $4"
    elif (( v < $2 || v > $3 )); then
        err "$k=$v 超出 $2–$3 —— 程式會靜默夾到範圍內"
    else
        ok "$k=$v"
    fi
}
check_num NFHH_AUDIT_KEEP_DAYS 1 3650 90
check_num NFHH_AUDIT_MAX_ROWS 100 1000000 20000

# ── 5. 密鑰 ──────────────────────────────────────────
# 全都可以留空，留空就是把對應功能關掉。所以這裡分兩種講法：
# 沒填 = 提醒你哪個功能是關的；填了但格式不對 = 錯誤。
section "密鑰"

SECRET="$(get NFHH_MAIL_SECRET)"
if [[ -z "$SECRET" ]]; then
    warn "NFHH_MAIL_SECRET 空的 → /api/mail/ingest 停用，Email Worker 推不進信件"
elif [[ ${#SECRET} -lt 32 ]]; then
    err "NFHH_MAIL_SECRET 只有 ${#SECRET} 碼，太短。產生：openssl rand -hex 32"
else
    ok "NFHH_MAIL_SECRET $(mask "$SECRET")"
    [[ ${#SECRET} -ne 64 ]] && note "openssl rand -hex 32 會給 64 碼，這把不是那樣產的（不影響運作）"
    note "必須與 Cloudflare Email Worker 的 PANEL_SECRET 一模一樣，這支腳本驗不到那邊"
fi

RESEND="$(get RESEND_API_KEY)"
if [[ -z "$RESEND" ]]; then
    warn "RESEND_API_KEY 空的 → 註冊的 Email 驗證碼與邀請函都不會寄出"
elif [[ "$RESEND" != re_* ]]; then
    err "RESEND_API_KEY 不是 re_ 開頭 —— 不像 Resend 的金鑰"
else
    ok "RESEND_API_KEY $(mask "$RESEND")"
fi

CF_ACC="$(get CF_ACCOUNT_ID)"; CF_TOK="$(get CF_API_TOKEN)"
if [[ -z "$CF_ACC" && -z "$CF_TOK" ]]; then
    warn "CF_ACCOUNT_ID / CF_API_TOKEN 空的 → 查不到轉發收件人的驗證狀態，登記時也不會自動建位址"
else
    [[ -z "$CF_ACC" || -z "$CF_TOK" ]] && err "CF_ACCOUNT_ID 與 CF_API_TOKEN 必須成對，只有一個等於兩個都沒用"
    if [[ -n "$CF_ACC" ]]; then
        if [[ "$CF_ACC" =~ ^[0-9a-f]{32}$ ]]; then ok "CF_ACCOUNT_ID $(mask "$CF_ACC")"
        else err "CF_ACCOUNT_ID 不是 32 碼十六進位 —— 可能貼成了 Zone ID 或帳戶名稱"; fi
    fi
    if [[ -n "$CF_TOK" ]]; then
        if [[ "$CF_TOK" =~ ^[A-Za-z0-9_-]{30,}$ ]]; then
            ok "CF_API_TOKEN $(mask "$CF_TOK")"
            note "需要帳戶層級 Email Routing Addresses 的讀 + 寫，只有讀的話「重新發送驗證信」會失效"
        else
            err "CF_API_TOKEN 的長度或字元不像 API token（別貼成 Global API Key）"
        fi
    fi
fi

# ── 6. 這台機器 ──────────────────────────────────────
section "主機"

CERT_DIR="$(get NFHH_CERT_DIR)"
[[ -z "$CERT_DIR" && -n "$DOMAIN" ]] && CERT_DIR="/etc/ssl/*.${DOMAIN}_ecc"
if [[ -n "$CERT_DIR" ]]; then
    # 目錄名字面上就含星號，不是 glob —— 用 -e 直接判斷那個字面路徑
    if [[ -e "$CERT_DIR" ]]; then
        ok "憑證目錄存在：$CERT_DIR"
    else
        err "憑證目錄不存在：$CERT_DIR"
        note "compose 第 25 行把它掛進 smartdns。目錄不在，Docker 會自己建一個空的 root 目錄，"
        note "DoT（:853）就拿不到憑證。acme.sh 的目錄名字面上含星號，不是萬用字元。"
    fi
fi

if (( IS_LIVE == 0 )); then
    note "檢查的不是專案的 .env，跳過 compose 解析（compose 只會讀 $ROOT/.env）"
elif docker compose config --quiet 2>/tmp/nfhh-compose-err; then
    ok "docker compose 能完整解析這份設定"
    if [[ -s /tmp/nfhh-compose-err ]]; then
        while IFS= read -r l; do warn "compose: $l"; done < /tmp/nfhh-compose-err
    fi
else
    err "docker compose 解析失敗："
    sed 's/^/    /' /tmp/nfhh-compose-err
fi
rm -f /tmp/nfhh-compose-err

# ── 7. 與正在跑的容器比對 ────────────────────────────
# 最重要的一段：.env 現在長這樣，不代表跑著的 control 就是這樣。
# 兩邊不同 = 下次 ./nfhh restart 行為會變。用 sha256 比，值本身不會出現在畫面上。
section "與執行中的容器比對"

if (( IS_LIVE == 0 )); then
    note "檢查的不是專案的 .env，跳過比對"
elif ! docker inspect nfhh-control >/dev/null 2>&1; then
    note "nfhh-control 沒在跑，跳過比對"
elif ! RUNNING="$(docker inspect nfhh-control --format '{{range .Config.Env}}{{println .}}{{end}}' 2>/dev/null)"; then
    note "讀不到容器環境（權限？），跳過比對"
else
    WANTED="$(docker compose config --format json 2>/dev/null \
        | jq -r '.services.control.environment | to_entries[] | "\(.key)=\(.value // "")"')"
    hashof() { printf '%s' "$1" | sha256sum | cut -c1-16; }
    # 只有這四項是密鑰，其餘（網域、稽核參數）直接印值 —— 比對段的重點就是
    # 「差在哪」，把 NFHH_AUDIT_KEEP_DAYS 遮成「共 2 碼」等於什麼都沒說。
    show() {
        case "$1" in
            NFHH_MAIL_SECRET|NFHH_RESEND_KEY|NFHH_CF_ACCOUNT|NFHH_CF_TOKEN) mask "$2" ;;
            *) echo "$2" ;;
        esac
    }
    DIFF=0
    while IFS= read -r line; do
        [[ -z "$line" ]] && continue
        k="${line%%=*}"; want="${line#*=}"
        cur="$(awk -F= -v k="$k" '$1==k{sub(/^[^=]*=/,""); print; exit}' <<<"$RUNNING")"
        if ! grep -q "^$k=" <<<"$RUNNING"; then
            warn "$k 是新增的 —— 跑著的容器裡沒有這一項"; DIFF=$((DIFF + 1))
        elif [[ "$(hashof "$want")" != "$(hashof "$cur")" ]]; then
            warn "$k 與執行中的值不同（.env 這邊是 $(show "$k" "$want")）"; DIFF=$((DIFF + 1))
        fi
    done <<<"$WANTED"
    if (( DIFF == 0 )); then
        ok "完全一致 —— 重啟後 control 的環境不會有任何改變"
    else
        note "上面這 $DIFF 項會在下次 ./nfhh restart 時生效。是你剛改的就沒問題。"
    fi
fi

# ── 結論 ─────────────────────────────────────────────
echo
if (( ERRS > 0 )); then
    echo "有 $ERRS 個會出錯的問題，另有 $WARNS 個提醒。修完再 ./nfhh restart。"
    exit 1
elif (( WARNS > 0 )); then
    echo "沒有會出錯的問題，$WARNS 個提醒（多半是「某功能是關的」，刻意的話就不用理）。"
else
    echo "全部通過。"
fi
