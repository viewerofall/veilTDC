#!/usr/bin/env sh
set -eu

if [ "$(id -u)" -ne 0 ]; then
    echo "[velogin] run this installer as root" >&2
    exit 1
fi

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
DIST_DIR="$SCRIPT_DIR/dist"
BIN_SRC="$SCRIPT_DIR/velogin"
PAM_SRC="$DIST_DIR/pam.d/velogin"
SERVICE_SRC="$DIST_DIR/velogin.service"

require_file() {
    if [ ! -f "$1" ]; then
        echo "[velogin] missing required file: $1" >&2
        exit 1
    fi
}

require_file "$BIN_SRC"
require_file "$PAM_SRC"
require_file "$SERVICE_SRC"

install -Dm755 "$BIN_SRC" /usr/local/bin/velogin
install -Dm644 "$PAM_SRC" /etc/pam.d/velogin
install -Dm644 "$SERVICE_SRC" /etc/systemd/system/velogin.service

systemctl daemon-reload

cat <<'EOF'
[velogin] installed:
  /usr/local/bin/velogin
  /etc/pam.d/velogin
  /etc/systemd/system/velogin.service

[velogin] next:
  systemctl enable --now seatd.service
  systemctl disable getty@tty1.service
  systemctl enable velogin.service

[velogin] then reboot, or start velogin manually after disabling tty1 getty.
EOF
