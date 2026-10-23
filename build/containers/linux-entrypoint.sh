#!/usr/bin/env bash
set -euo pipefail
if [[ "${1:-}" == --desktop ]]; then
    # Xvfb has no window manager: nothing would minimise, restore or make a
    # window fullscreen. Openbox does, as a desktop would; the scenarios
    # start once it has claimed the display.
    shift
    openbox &
    for _ in $(seq 100); do
        xprop -root _NET_SUPPORTING_WM_CHECK 2>/dev/null | grep -q 'window id' && exec "$@"
        sleep 0.1
    done
    echo 'Openbox did not start on the virtual display' >&2
    exit 1
fi
if [[ "${1:-}" != --prepared ]]; then
    mkdir -p /work /evidence
    # Source remains read-only; OS-specific dependencies and outputs are isolated from the Mac checkout.
    tar -C /source --exclude=./node_modules --exclude=./target --exclude=./out --exclude=./.git --exclude=./.build --exclude=./dist --exclude=./docs --exclude=./.claude -cf - . | tar -C /work -xf -
    chown -R vnilink:vnilink /work /evidence /usr/local/cargo/registry /usr/local/cargo/git
    runuser -u vnilink -- bash -c 'cd /work && npm ci && node -e '\''require("electron")'\'''
    chown root:root /work/node_modules/electron/dist/chrome-sandbox
    chmod 4755 /work/node_modules/electron/dist/chrome-sandbox
    exec runuser -u vnilink -- /usr/local/bin/vnilink-tests --prepared
fi
cd /work
{
    uname -a
    node --version
    npm --version
    rustc --version
    cargo --version
    dpkg-query -W
} > /evidence/environment.txt
trap 'cp -a .build/test-runs /evidence/ 2>/dev/null || true' EXIT
npm run check
cargo fmt --all -- --check
npm run test:rust
# Real secret-service smoke is separate from unavailable-store fixtures.
dbus-run-session -- bash -c 'printf "\n" | gnome-keyring-daemon --daemonize --unlock --components=secrets; xvfb-run -a /usr/local/bin/vnilink-tests --desktop npm test'
