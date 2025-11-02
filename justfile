[doc('Run formatters')]
fmt:
    cargo fmt
    treefmt --allow-missing-formatter

[doc('clean')]
clean:
    cargo clean

[doc('build')]
build:
    cargo build --workspace --target x86_64-unknown-linux-musl --profile dev

[doc('test')]
test:
    cargo test --workspace --target x86_64-unknown-linux-musl --profile dev

[doc('run-client')]
run-client:
    rm -f /tmp/nixbtm.sock
    cargo run --bin nix-btm --target x86_64-unknown-linux-musl --profile dev -- client -n /tmp/nixbtm.sock

[doc('run-daemon')]
run-daemon:
    rm -f /tmp/nixbtm.sock
    cargo run --bin nix-btm --target x86_64-unknown-linux-musl --profile dev -- client -n /tmp/nixbtm.sock

[doc('lint')]
lint: fmt
    cargo clippy --workspace --target x86_64-unknown-linux-musl --release

[doc('lint-fix')]
lint-fix: fmt
    cargo clippy --fix --workspace --target x86_64-unknown-linux-musl --profile dev --allow-dirty
