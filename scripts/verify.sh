#!/usr/bin/env bash
# Bounded local checkpoint verification. Reuses Cargo's selected target/cache.
set -euo pipefail

if ! command -v cargo >/dev/null 2>&1; then
    echo "Rust is required. Install Rust 1.85 or newer with rustfmt and Clippy." >&2
    exit 1
fi
repository_root=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
cd "$repository_root"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"

rustc --version
cargo --version
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets --all-features
cargo test --locked --doc
RUSTDOCFLAGS="${RUSTDOCFLAGS:-} -D warnings" cargo doc --locked --no-deps
cargo build --locked --release
./scripts/smoke.sh

echo "Checkpoint checks passed for the selected toolchain and platform."
echo "This does not run comparative benchmarks, fuzz campaigns, or release qualification."
