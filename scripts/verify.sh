#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
cargo build --locked -p issues-plugin -p calendar-plugin --target wasm32-wasip2
cargo test --locked -p cordis-core -p cordis-wire -p cordis-wasm -p cordis-host --all-targets
cargo fmt --all -- --check
cargo clippy --locked -p cordis-core -p cordis-wire -p cordis-wasm -p cordis-host --all-targets -- -D warnings
cargo clippy --locked -p issues-plugin -p calendar-plugin --target wasm32-wasip2 -- -D warnings
