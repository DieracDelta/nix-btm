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

[doc('test')]
test:
    cargo test --workspace --profile dev -- --nocapture

[doc('run-client')]
run-client:
    rm -f /tmp/nixbtm.sock
    cargo run --bin nix-btm --profile dev -- client -n /tmp/nixbtm.sock

[doc('run-daemon')]
run-daemon:
    rm -f /tmp/nixbtm.sock
    cargo run --bin nix-btm --profile dev -- client -n /tmp/nixbtm.sock

[doc('lint')]
lint: fmt
    cargo clippy --workspace --release

[doc('lint-fix')]
lint-fix: fmt
    cargo clippy --fix --workspace --profile dev --allow-dirty
