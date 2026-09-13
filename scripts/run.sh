#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
cargo build --locked -p issues-plugin -p calendar-plugin --target wasm32-wasip2
./scripts/bundle.sh
open "$PWD/target/Harness POC.app"
