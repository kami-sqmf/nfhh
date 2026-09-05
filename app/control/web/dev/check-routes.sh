#!/usr/bin/env bash
# 檢查前端呼叫的每個端點都不會回 405。
#
# 這類 bug 從畫面上看不出來，截圖工具也抓不到 —— 它把 fetch 整個樁掉了，
# 根本沒碰到真的 API。而 api.js 的 req() 是「有 body 就 POST」，
# 一個不帶 body 的 POST 會靜靜送成 GET。
#
# 只看方法對不對，不管授權：未登入時回 400/401 都算通過，只有 405 是錯的。
set -uo pipefail
BASE="${1:-http://127.0.0.1:8081}"
fail=0

check() {  # check <方法> <路徑>
    code=$(curl -s -o /dev/null -w '%{http_code}' -X "$1" "$BASE$2")
    if [[ "$code" == "405" ]]; then
        printf '  ✗ %-6s %-40s 405 Method Not Allowed\n' "$1" "$2"
        fail=1
    else
        printf '  ✓ %-6s %-40s %s\n' "$1" "$2" "$code"
    fi
}

echo "檢查 $BASE"
check GET    /api/status
check POST   /api/join/start
check POST   /api/join/verify
check POST   /api/login/any/start
check POST   /api/login/any/finish
check POST   /api/login/start
check POST   /api/login/finish
check POST   /api/register/start
check POST   /api/register/finish
check POST   /api/logout
check GET    /api/passkeys
check POST   /api/passkeys/x
check DELETE /api/passkeys/x
check POST   /api/allow
check POST   /api/allow/1.1.1.1
check DELETE /api/allow/1.1.1.1
check GET    /api/allow/1.1.1.1/queries
check GET    /api/audit
check GET    /api/mail
check DELETE /api/mail
check GET    /api/mail/inbox
check GET    /api/mail/1
check DELETE /api/mail/1
check GET    /api/settings
check PUT    /api/settings
check GET    /api/members
check DELETE /api/members/x
check POST   /api/members/x/role
check POST   /api/members/x/platforms
check DELETE /api/members/x/platforms/netflix
check GET    /api/recipients
check POST   /api/recipients
check DELETE /api/recipients/1
check POST   /api/recipients/1/enabled
check POST   /api/recipients/1/verify
check POST   /api/mailboxes
check DELETE /api/mailboxes/x@y.tw
check GET    /api/invite
check POST   /api/invite
check DELETE /api/invite/a@b.c
check GET    /api/push/key
check GET    /api/push/subs
check POST   /api/push/subs
check DELETE /api/push/subs/1
check POST   /api/push/unsubscribe
check POST   /api/push/check
check GET    /api/me/notify
check POST   /api/me/notify
check GET    /api/me/forwarding
check POST   /api/me/forwarding
check POST   /api/me/forwarding/resend

[[ $fail == 0 ]] && echo "全部通過" || { echo "有端點的方法對不上"; exit 1; }
