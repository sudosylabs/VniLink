#!/usr/bin/env bash
# Launches the development application from a checkout in any state, after
# VS Code's scripts/code.sh: build/lib/preLaunch.ts installs and builds what
# is missing, then `node build/run.ts dev` checks what the launch needs,
# prepares Electron and starts it with the watchers. Arguments go to the app.

set -e

if [[ "$OSTYPE" == "darwin"* ]]; then
	realpath() { [[ $1 = /* ]] && echo "$1" || echo "$PWD/${1#./}"; }
	ROOT=$(dirname "$(dirname "$(realpath "$0")")")
else
	ROOT=$(dirname "$(dirname "$(readlink -f "$0")")")
fi

cd "$ROOT"

node build/lib/preLaunch.ts
exec node build/run.ts dev "$@"
