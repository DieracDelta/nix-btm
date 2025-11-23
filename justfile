[doc('Run formatters')]
fmt:
    cargo fmt
    treefmt --allow-missing-formatter

[doc('clean')]
clean:
    cargo clean

[doc('build')]
build:
    cargo build --workspace --profile dev

[doc('doc')]
doc:
    cargo doc --workspace --profile dev --open

[doc('test')]
test:
    cargo test --workspace --profile dev -- --nocapture

[doc('run-client')]
run-client:
    -pkill -9 -f "nix-btm.*client" 2>/dev/null || true
    rm -f /tmp/nixbtm-client-*.log
    cargo run --bin nix-btm --profile dev -- client -d /tmp/nix-daemon.sock

[doc('run-daemon')]
run-daemon:
    -pkill -9 -f "nix-btm.*daemon" 2>/dev/null || true
    rm -f /tmp/nixbtm.sock /tmp/nix-daemon.sock /tmp/nixbtm-daemon-*.log
    sleep 1
    cargo run --bin nix-btm --profile dev -- daemon -n /tmp/nixbtm.sock -d /tmp/nix-daemon.sock

[doc('lint')]
lint: fmt
    cargo clippy --workspace --release

[doc('lint-fix')]
lint-fix: fmt
    cargo clippy --fix --workspace --profile dev --allow-dirty
