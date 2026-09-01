# 多階段建置。執行階段需要 nftables 執行檔：控制平面用 `nft -f -` 操作白名單。

# ── 前端 ─────────────────────────────────────────────
# Svelte + Vite，產物輸出到 /static，下一階段用 rust-embed 打進執行檔。
FROM oven/bun:1-alpine AS web
WORKDIR /web

# 先只用 manifest 裝相依，讓這層可被快取
COPY web/package.json web/bun.lock ./
RUN bun install --frozen-lockfile

COPY web/ ./
# vite.config.js 的 outDir 是 ../static，相對於 /web 就是 /static
RUN bun run build


# ── 後端 ─────────────────────────────────────────────
FROM rust:1-slim-bookworm AS builder
WORKDIR /build

# rusqlite bundled 需 cc，webauthn-rs 與 reqwest 依賴 openssl-sys
RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# 先只用 manifest 編相依套件，讓這層可被快取
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo 'fn main(){}' > src/main.rs \
    && cargo build --release \
    && rm -rf src

COPY src ./src
# rust-embed 在 release 建置時把 static/ 整包編進執行檔，
# 所以這一步必須在 cargo build 之前。
COPY --from=web /static ./static
# 動 mtime 讓 cargo 知道原始碼換了
RUN touch src/main.rs && cargo build --release


FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
        nftables ca-certificates libssl3 \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/nfhh-control /usr/local/bin/nfhh-control

# 以 root 執行：操作 nftables 需要 NET_ADMIN
ENTRYPOINT ["/usr/local/bin/nfhh-control"]
