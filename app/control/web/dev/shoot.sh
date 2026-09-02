#!/usr/bin/env bash
# 一次性無頭 Chromium + CDP 截圖。容器只把 CDP 發佈到 127.0.0.1、不是 root、
# 不掛主機目錄；正常結束或 Ctrl-C 都會把容器清掉（detached 的 --rm 不保證這點）。
#
# 用法：dev/shoot.sh <url> '<steps json>'    環境變數：MOCK（預設 dev/mock.js）、OUT（預設 /tmp/shots）
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"

# 釘 digest 而不是 tag：浮動 tag 的內容會變。這個值是本機 `docker image inspect`
# 對 zenika/alpine-chrome 印出的 RepoDigest；要升級就重新 pull、inspect、換掉這行。
IMAGE='zenika/alpine-chrome@sha256:47da877e5622528039625218d15d7e1ccae4e426c6cc7671d165837ea98aacc8'
OUT="${OUT:-/tmp/shots}"
mkdir -p "$OUT"

trap 'docker rm -f nfhh-shot >/dev/null 2>&1 || true' EXIT
docker run -d --rm --name nfhh-shot \
  -p 127.0.0.1:9333:9333 \
  --entrypoint chromium-browser "$IMAGE" \
  --headless --no-sandbox --disable-gpu --hide-scrollbars \
  --remote-debugging-port=9333 --remote-debugging-address=0.0.0.0 \
  --disable-dev-shm-usage about:blank >/dev/null

# 映像預設以 chrome 使用者執行；萬一哪天不是，寧可中止
uid="$(docker exec nfhh-shot id -u)"
[[ "$uid" != "0" ]] || { echo "容器以 root 執行，中止" >&2; exit 1; }

for _ in $(seq 1 50); do
  curl -fs http://127.0.0.1:9333/json/version >/dev/null 2>&1 && break
  sleep 0.2
done

MOCK="${MOCK:-$here/mock.js}" OUT="$OUT" bun "$here/shoot.js" "$@"
