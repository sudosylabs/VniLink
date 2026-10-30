#!/usr/bin/env bash
# ---------------------------------------------------------------------------------------------
# Copyright (c) Microsoft Corporation. All rights reserved.
# Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
# Adapted from VS Code 1.141.0 scripts/code.sh.
# ---------------------------------------------------------------------------------------------

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
