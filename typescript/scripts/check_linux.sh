#!/usr/bin/env bash
# Runs inside the check-linux container: /flyrail is the read-only checkout, /cache the writable cache.
set -euo pipefail

work=/cache/work
mkdir -p "$HOME" "$TMPDIR" "$work"
rm -rf "$work/typescript" "$work/rust" "$work/spec" "$work/rust-toolchain.toml"
tar -C /flyrail \
    --exclude=typescript/node_modules --exclude=typescript/build --exclude=typescript/dist \
    --exclude=typescript/native/target --exclude=typescript/src/native/binding.d.ts --exclude=rust/target \
    -cf - typescript rust spec rust-toolchain.toml | tar -C "$work" -xf -

export PATH="$CARGO_HOME/bin:$PATH"
if [[ ! -x "$CARGO_HOME/bin/rustup" ]]; then
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs |
        sh -s -- -y --no-modify-path --profile minimal --default-toolchain none
fi
channel="$(sed -n 's/^channel = "\(.*\)"$/\1/p' "$work/rust-toolchain.toml")"
rustup toolchain install "$channel" --profile minimal --component rustfmt,clippy

cd "$work/typescript"
echo "node $(node --version), npm $(npm --version), $(cargo --version), $(ldd --version | head -n 1)"
npm ci --no-audit --no-fund
python3 -I -B scripts/check_cargo_lock.py
npm run build:native
npm run format:check
cargo fmt --check --manifest-path native/Cargo.toml
npm run lint
cargo clippy --manifest-path native/Cargo.toml --all-targets --all-features -- -D warnings
npm run typecheck
npm test
npm run build:native:release
npm run build
python3 -I -B scripts/package_qa.py
