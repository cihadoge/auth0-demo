# auth0

auth0 demo integration with cloudflare tunnel

## Stack

- Rust 2024 · axum 0.8 · tokio · serde
- Vanilla JS PWA (no npm), assets cached in memory and hot-reloaded from `static/` (with compile-time fallback)
- Next available port (scan from 69, skip browser-blocked)
- systemd via `./deploy.sh` (caches Rust build when only `static/` changes)

## Layout

```
auth0/
├── AGENT.md
├── Cargo.toml
├── rust-toolchain.toml
├── deploy.sh
├── config.toml.example
├── src/
│   ├── main.rs      # bind (next free port) → routes → serve
│   └── config.rs    # --config / env / ./config.toml / /etc/auth0/config.toml
└── static/          # index.html, app.js, style.css, manifest, sw.js
```

## Run

```bash
cargo run --release
# or
sudo ./deploy.sh        # skips cargo build & service restart when only static/ changed
sudo ./deploy.sh -f     # force full rebuild & restart
journalctl -u auth0 -f
```

## API

```
GET /health   → {"status":"ok","service":"auth0","port":N}
```

## Config

Lookup: `--config` → `${{NAME_UPPER}}_CONFIG` → `./config.toml` → `/etc/auth0/config.toml`

```toml
[server]
host = "0.0.0.0"
port = 0    # 0 = next available ≥ 69 (PORT env also works)
```

## Checklist before first deploy

- [ ] `cargo build --release` succeeds
- [ ] Port scan skips browser-blocked ports; `PORT` env works
- [ ] `/health` returns ok
- [ ] Manifest + `sw.js` at `/`; SW never caches `/api/*`
- [ ] `deploy.sh` has `SERVICE_NAME=` for mobilecode log discovery
- [ ] `.gitignore` has `/target`
