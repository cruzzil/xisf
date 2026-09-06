#!/usr/bin/env bash
# Run what CI runs, with the settings CI uses.
#
# This exists because `-D warnings` and the feature matrix are exactly the
# conditions a local `cargo test` does not reproduce, and both have caught
# things that a green local run did not. Run this before pushing.
set -uo pipefail

export RUSTFLAGS="-D warnings"
status=0
run() {
    printf '%-46s ' "$1"; shift
    if out=$("$@" 2>&1); then echo "OK"; else echo "FAILED"; echo "$out" | grep -E '^error' -A4 | head -12; status=1; fi
}

run "fmt"                    cargo fmt --all --check
run "clippy"                 cargo clippy --workspace --all-targets
run "test (default features)" cargo test --workspace
run "no default features"    cargo test -p xisf-core --no-default-features
for feature in zlib lz4 zstd checksums; do
    run "feature: $feature"  cargo test -p xisf-core --no-default-features --features "$feature"
done
RUSTDOCFLAGS="-D warnings" run "docs" cargo doc --workspace --no-deps

if [ -n "${XISF_VERIFY:-}" ]; then
    run "round-trip through libXISF" cargo test -p xisf-core --test round_trip_libxisf
else
    printf '%-46s %s\n' "round-trip through libXISF" "SKIPPED (set XISF_VERIFY)"
fi

exit $status
