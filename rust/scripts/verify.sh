#!/usr/bin/env bash
set -euo pipefail

cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo check
cargo test --all-targets -- --nocapture
