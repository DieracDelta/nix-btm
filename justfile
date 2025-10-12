[doc('Run formatters')]
fmt:
    cargo fmt
    treefmt --allow-missing-formatter

[doc('clean')]
clean:
    cargo clean

[doc('build')]
build:
    cargo build --workspace --target x86_64-unknown-linux-musl --release

[doc('run')]
run:
    cargo run --bin nix-btm --target x86_64-unknown-linux-musl --release -- -s /tmp/nixbtm.sock

[doc('lint')]
lint: fmt
    cargo clippy --workspace --target x86_64-unknown-linux-musl --release

[doc('lint-fix')]
lint-fix: fmt
    cargo clippy --fix --workspace --target x86_64-unknown-linux-musl --release --allow-dirty
