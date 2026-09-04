#!/usr/bin/env bash
# 一次性無頭 Chromium + CDP 截圖。容器只把 CDP 發佈到 127.0.0.1、不是 root、
# 不掛主機目錄；正常結束或 Ctrl-C 都會把容器清掉（detached 的 --rm 不保證這點）。
#
# 用法：dev/shoot.sh <url> '<steps json>'    環境變數：MOCK（預設 dev/mock.js）、OUT（預設 /tmp/shots）
set -euo pipefail
here="$(cd "$(dirname "$(readlink -f "$0")")" && pwd)"

[[ $# -eq 2 ]] || { echo "用法：dev/shoot.sh <url> '<steps json>'" >&2; exit 2; }

# 釘 digest 而不是 tag：浮動 tag 的內容會變。這個值是本機 `docker image inspect`
# 對 zenika/alpine-chrome 印出的 RepoDigest；要升級就重新 pull、inspect、換掉這行。
IMAGE='zenika/alpine-chrome@sha256:47da877e5622528039625218d15d7e1ccae4e426c6cc7671d165837ea98aacc8'
OUT="${OUT:-/tmp/shots}"
mkdir -p "$OUT"

if docker inspect nfhh-shot >/dev/null 2>&1; then
  echo "容器 nfhh-shot 已存在：另一個 shoot.sh 還在跑，或上次被 kill 沒清到。確定沒人在跑就 docker rm -f nfhh-shot" >&2
  exit 1
fi

# 不用 --rm：Chromium 一起來就崩的話，容器要留著給下面印 log；清理一律交給 trap。
# started 旗標讓 trap 只清自己起的容器：docker run 失敗（同名競態、映像不在）時
# started 仍為空，trap 不會 rm 掉別人的容器。docker run 自成一行，失敗就由 set -e 中止 ——
# 寫成 `docker run … && started=1` 的話 set -e 不會管 && 清單裡非末項的失敗，腳本會
# 接著對別人的容器做 exec 與截圖。
started=
trap '[[ -n "${started:-}" ]] && docker rm -f nfhh-shot >/dev/null 2>&1 || true' EXIT
docker run -d --name nfhh-shot \
  -p 127.0.0.1:9333:9333 \
  --security-opt no-new-privileges --cap-drop ALL \
  --entrypoint chromium-browser "$IMAGE" \
  --headless --no-sandbox --disable-gpu --hide-scrollbars \
  --remote-debugging-port=9333 --remote-debugging-address=0.0.0.0 \
  --disable-dev-shm-usage about:blank >/dev/null
started=1

# 先等 CDP 起來再驗 uid：Chromium 崩掉時 exec 只會回 No such container，看不出原因
for _ in $(seq 1 50); do
  curl -fs --max-time 1 http://127.0.0.1:9333/json/version >/dev/null 2>&1 && break
  sleep 0.2
done
curl -fs --max-time 1 http://127.0.0.1:9333/json/version >/dev/null || {
  echo "CDP 10 秒內沒起來；容器最後的輸出：" >&2
  docker logs --tail 20 nfhh-shot >&2 || true
  exit 1
}

# 映像預設以 chrome 使用者執行；萬一哪天不是，寧可中止
uid="$(docker exec nfhh-shot id -u)"
[[ "$uid" != "0" ]] || { echo "容器以 root 執行，中止" >&2; exit 1; }

MOCK="${MOCK:-$here/mock.js}" OUT="$OUT" bun "$here/shoot.js" "$@"
