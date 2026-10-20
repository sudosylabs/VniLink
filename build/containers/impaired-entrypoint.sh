#!/usr/bin/env bash
set -euo pipefail
# Network namespaces need root; the checkout stays read-only and outputs stay in container volumes.
mkdir -p /work /evidence
tar -C /source --exclude=./node_modules --exclude=./target --exclude=./out --exclude=./.git --exclude=./.build --exclude=./dist --exclude=./docs -cf - . | tar -C /work -xf -
cd /work
# build/run.ts loads every task module; install scripts (Electron's download) are not needed here.
npm ci --ignore-scripts --no-audit --no-fund
{
    uname -a
    node --version
    rustc --version
    cargo --version
    tc -V
    nproc
    free -b
} > /evidence/environment.txt
exec node build/run.ts measure:impaired:linux --output /evidence "$@"
