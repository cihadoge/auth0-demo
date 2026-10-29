#!/usr/bin/env bash
# auth0 deploy: build release binary (cached when Rust sources are unchanged),
# sync static assets, install as systemd service, and restart when needed.
# Usage: sudo ./deploy.sh [--force|-f] [--restart]
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SERVICE_NAME="auth0-demo"
APP_DIR="/opt/auth0"
CONF_DIR="/etc/auth0"
CONF="$CONF_DIR/config.toml"
UNIT="/etc/systemd/system/${SERVICE_NAME}.service"

FORCE_BUILD="${FORCE_BUILD:-0}"
FORCE_RESTART="${FORCE_RESTART:-0}"
for arg in "$@"; do
  case "$arg" in
    --force|-f)
      FORCE_BUILD=1
      FORCE_RESTART=1
      ;;
    --restart)
      FORCE_RESTART=1
      ;;
  esac
done

if [ "$(id -u)" -ne 0 ]; then
  echo "error: run as root (sudo ./deploy.sh)" >&2
  exit 1
fi

[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
export PATH="$HOME/.cargo/bin:/root/.cargo/bin:$PATH"

compute_main_hash() {
  {
    rustc --version 2>/dev/null || true
    (
      cd "$ROOT"
      find Cargo.toml Cargo.lock rust-toolchain.toml src -type f -print0 2>/dev/null \
        | LC_ALL=C sort -z \
        | xargs -0 sha256sum
    )
  } | sha256sum | awk '{print $1}'
}

compute_static_hash() {
  if [ ! -d "$ROOT/static" ]; then
    echo "none"
    return
  fi
  (
    cd "$ROOT"
    find static -type f -print0 2>/dev/null \
      | LC_ALL=C sort -z \
      | xargs -0 sha256sum
  ) | sha256sum | awk '{print $1}'
}

MAIN_HASH="$(compute_main_hash)"
STATIC_HASH="$(compute_static_hash)"

BIN="$ROOT/target/release/auth0"
MAIN_HASH_FILE="$ROOT/target/release/.main-hash"
BIN_CACHE_DIR="$ROOT/target/release/.bin-cache"
CACHED_BIN="$BIN_CACHE_DIR/auth0-$MAIN_HASH"

if [ "$FORCE_BUILD" -eq 0 ] && [ -x "$BIN" ] && [ -f "$MAIN_HASH_FILE" ] && [ "$(cat "$MAIN_HASH_FILE")" = "$MAIN_HASH" ]; then
  echo "==> main sources unchanged (${MAIN_HASH:0:12}); skipping cargo build"
elif [ "$FORCE_BUILD" -eq 0 ] && [ -x "$CACHED_BIN" ]; then
  echo "==> restoring cached binary for main hash ${MAIN_HASH:0:12}"
  cp -f "$CACHED_BIN" "$BIN"
  chmod 755 "$BIN"
  printf '%s\n' "$MAIN_HASH" > "$MAIN_HASH_FILE"
else
  echo "==> building (cargo build --release, main ${MAIN_HASH:0:12})"
  cargo build --release --manifest-path "$ROOT/Cargo.toml"
  printf '%s\n' "$MAIN_HASH" > "$MAIN_HASH_FILE"
  install -d -m 755 "$BIN_CACHE_DIR"
  cp -f "$BIN" "$CACHED_BIN"
  ls -1t "$BIN_CACHE_DIR"/auth0-* 2>/dev/null | tail -n +4 | xargs -r rm -f
fi

install -d -m 755 "$APP_DIR" "$CONF_DIR"

NEED_RESTART="$FORCE_RESTART"
if [ "$FORCE_BUILD" -eq 1 ]; then
  NEED_RESTART=1
fi

if [ ! -x "$APP_DIR/auth0" ] || ! cmp -s "$BIN" "$APP_DIR/auth0"; then
  echo "==> installing binary to $APP_DIR"
  install -m 755 "$BIN" "$APP_DIR/auth0.new"
  mv -f "$APP_DIR/auth0.new" "$APP_DIR/auth0"
  NEED_RESTART=1
else
  echo "==> binary in $APP_DIR/auth0 unchanged"
fi

if [ -d "$ROOT/static" ]; then
  DEPLOYED_STATIC_HASH_FILE="$APP_DIR/static/.static-hash"
  if [ "$FORCE_BUILD" -eq 0 ] && [ -f "$DEPLOYED_STATIC_HASH_FILE" ] && [ "$(cat "$DEPLOYED_STATIC_HASH_FILE")" = "$STATIC_HASH" ]; then
    echo "==> static assets unchanged (${STATIC_HASH:0:12})"
  else
    echo "==> syncing static assets (${STATIC_HASH:0:12}) to $APP_DIR/static"
    STAGE_DIR="$(mktemp -d "$APP_DIR/.static.XXXXXX")"
    cp -a "$ROOT/static/." "$STAGE_DIR/"
    chmod -R a+rX "$STAGE_DIR"
    printf '%s\n' "$STATIC_HASH" > "$STAGE_DIR/.static-hash"
    rm -rf "$APP_DIR/.static.old"
    if [ -d "$APP_DIR/static" ]; then
      mv "$APP_DIR/static" "$APP_DIR/.static.old"
    fi
    mv "$STAGE_DIR" "$APP_DIR/static"
    rm -rf "$APP_DIR/.static.old"
  fi
fi

GENERATED=0
if [ ! -f "$CONF" ]; then
  PW="$(openssl rand -base64 18 | tr '+/' '-_' | tr -d '=')"
  # Quoted delimiter: static body, nothing for the shell to expand.
  cat > "$CONF" <<'EOF'
[server]
host = "0.0.0.0"
port = 0
EOF
  chmod 600 "$CONF"
  GENERATED=1
  NEED_RESTART=1
else
  echo "==> keeping existing $CONF"
fi

echo "==> writing systemd unit $UNIT"
# Build the unit in a temp file first so we can compare against the installed
# unit without touching its mtime when nothing changed (avoids spurious
# "changed on disk" warnings from systemctl).
UNIT_TMP="$(mktemp)"
# The Description carries the project's free-text description, so it is written
# through a quoted delimiter: the shell expands nothing the scaffolder
# substituted into it. The remainder needs $APP_DIR/$CONF, hence a plain heredoc.
{
  cat <<'UNIT_HEAD'
[Unit]
Description=auth0-demo - auth0 demo integration with cloudflare tunnel
UNIT_HEAD
  cat <<UNIT_BODY
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=$APP_DIR/auth0 --config $CONF
WorkingDirectory=$APP_DIR
Restart=always
RestartSec=3
User=root
Environment=RUST_LOG=warn
NoNewPrivileges=true

[Install]
WantedBy=multi-user.target
UNIT_BODY
} > "$UNIT_TMP"

if [ -f "$UNIT" ] && cmp -s "$UNIT_TMP" "$UNIT"; then
  rm -f "$UNIT_TMP"
  chmod 644 "$UNIT"
  echo "==> systemd unit unchanged"
  if ! systemctl is-enabled --quiet "$SERVICE_NAME.service" 2>/dev/null; then
    systemctl enable "$SERVICE_NAME.service" >/dev/null
  fi
else
  chmod 644 "$UNIT_TMP"
  mv -f "$UNIT_TMP" "$UNIT"
  systemctl daemon-reload
  systemctl enable "$SERVICE_NAME.service" >/dev/null
  NEED_RESTART=1
fi

RUNNING_HASH_FILE="$APP_DIR/.running-main-hash"
if [ "$NEED_RESTART" -eq 0 ] \
  && systemctl is-active --quiet "$SERVICE_NAME" \
  && [ -f "$RUNNING_HASH_FILE" ] \
  && [ "$(cat "$RUNNING_HASH_FILE")" = "$MAIN_HASH" ]; then
  echo "==> $SERVICE_NAME already running current binary; static assets hot-reloaded"
else
  systemctl restart "$SERVICE_NAME"
  printf '%s\n' "$MAIN_HASH" > "$RUNNING_HASH_FILE"
  for _ in 1 2 3 4 5 6 7 8 9 10; do
    systemctl is-active --quiet "$SERVICE_NAME" && break
    sleep 0.1
  done
fi

if systemctl is-active --quiet "$SERVICE_NAME"; then
  PID="$(systemctl show -p MainPID --value "$SERVICE_NAME")"
  PORT="$(ss -ltnp 2>/dev/null | awk -v pid="$PID" '$0 ~ ("pid=" pid ",") {split($4,a,":"); print a[length(a)]; exit}')"
  if [ -z "$PORT" ]; then
    PORT="$(journalctl -u "$SERVICE_NAME" -n 50 --no-pager 2>/dev/null \
      | grep -oE '0\.0\.0\.0:[0-9]+' | tail -1 | cut -d: -f2 || true)"
  fi
  IP="$(hostname -I 2>/dev/null | awk '{print $1}')"
  echo ""
  echo "✔ $SERVICE_NAME is running on http://${IP:-localhost}:${PORT:-?}/"
else
  echo "service failed to start:" >&2
  journalctl -u "$SERVICE_NAME" -n 30 --no-pager >&2
  exit 1
fi
