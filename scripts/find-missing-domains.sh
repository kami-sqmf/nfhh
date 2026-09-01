#!/usr/bin/env bash
#
# find-missing-domains.sh — 找出某台裝置查詢過、但沒被任何平台清單涵蓋的網域。
#
# 一般用 ./nfhh check 呼叫。
#
# 從 smartdns 日誌撈出該來源查過的網域，扣掉已涵蓋的。結果會混入該裝置查的
# 其他東西（Apple、Google…），要自己判斷哪些屬於目標平台。
#
# 用法：
#   ./nfhh check 203.0.113.45          # 指定來源 IP
#   ./nfhh check 203.0.113.45 disney   # 只看含關鍵字的
#
# 建議流程：先在該裝置上把目標 App 完整操作一輪（登入、瀏覽、播放），再跑這支。

set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

CLIENT="${1:-}"
FILTER="${2:-}"
if [[ -z "$CLIENT" ]]; then
    echo "用法: $0 <來源IP> [關鍵字]" >&2
    echo >&2
    echo "smartdns 目前看過的來源 IP：" >&2
    docker logs nfhh-smartdns --tail 20000 2>&1 \
      | grep -oE 'client: [0-9a-fA-F.:]+' | awk '{print $2}' \
      | sort | uniq -c | sort -rn | head -10 | sed 's/^/  /' >&2
    exit 1
fi

docker logs nfhh-smartdns --tail 50000 2>&1 \
  | grep -F "client: $CLIENT" \
  | grep -oE 'result: [^,]+' | awk '{print $2}' \
  | sort | uniq -c | sort -rn \
  | FILTER="$FILTER" SETS="$ROOT/config/smartdns/domain-set" python3 -c '
import os, sys, glob

# 只看啟用中的清單（*.list；.disabled 的不算）
covered = []
for path in glob.glob(os.path.join(os.environ["SETS"], "*.list")):
    for line in open(path, encoding="utf-8"):
        line = line.strip()
        if line and not line.startswith("#"):
            covered.append(line.lower())

filt = os.environ.get("FILTER", "").lower()
rows = []
for line in sys.stdin:
    parts = line.split()
    if len(parts) != 2:
        continue
    count, dom = int(parts[0]), parts[1].rstrip(".").lower()
    if filt and filt not in dom:
        continue
    # 比對規則同 smartdns / nginx：完全相同或為其子網域
    if any(dom == c or dom.endswith("." + c) for c in covered):
        continue
    rows.append((count, dom))

if not rows:
    print("✅ 該來源查過的網域都已被現有清單涵蓋")
    sys.exit()

print(f"未涵蓋的網域（共 {len(rows)} 個，依查詢次數排序）：")
print()
for count, dom in rows[:40]:
    print(f"  {count:5}  {dom}")
print()
print("判斷屬於目標平台的，加進 config/smartdns/domain-set/<平台>.list，然後：")
print("  ./nfhh apply")
'
