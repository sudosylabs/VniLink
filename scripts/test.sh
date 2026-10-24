#!/usr/bin/env bash
# Runs the UI scenarios from a checkout in any state, after VS Code's
# scripts/test.sh: build/lib/preLaunch.ts installs and builds what is missing,
# then `node build/run.ts test:ui` runs the scenarios. Arguments select them,
# for example `scripts/test.sh --scenario appearance`.
#
# On Linux without a display, the scenarios run as in the Linux container: on
# Xvfb with Openbox, which minimises, restores and fullscreens windows as a
# desktop would, and with a temporary unlocked keyring on a private session
# bus. `npm run doctor -- --task test:ui` names the packages this needs.

set -e

if [[ "$OSTYPE" == "darwin"* ]]; then
	realpath() { [[ $1 = /* ]] && echo "$1" || echo "$PWD/${1#./}"; }
	ROOT=$(dirname "$(dirname "$(realpath "$0")")")
else
	ROOT=$(dirname "$(dirname "$(readlink -f "$0")")")
fi

cd "$ROOT"

if [[ "$OSTYPE" == linux* && -z "$DISPLAY" && -z "$WAYLAND_DISPLAY" ]]; then
	for tool in dbus-run-session gnome-keyring-daemon xvfb-run openbox xprop; do
		if ! command -v "$tool" > /dev/null; then
			echo "There is no display, and running headless needs $tool. Run npm run doctor -- --task test:ui for the packages." >&2
			exit 1
		fi
	done
	export VNILINK_HEADLESS_TEST=1
	exec dbus-run-session -- bash -c 'printf "\n" | gnome-keyring-daemon --daemonize --unlock --components=secrets > /dev/null; exec xvfb-run -a "$0" "$@"' "$0" "$@"
fi

if [[ -n "$VNILINK_HEADLESS_TEST" ]]; then
	openbox &
	for _ in $(seq 100); do
		xprop -root _NET_SUPPORTING_WM_CHECK 2> /dev/null | grep -q 'window id' && break
		sleep 0.1
	done
fi

node build/lib/preLaunch.ts
exec node build/run.ts test:ui "$@"
