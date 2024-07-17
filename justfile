[doc('Run formatters')]
fmt:
  cargo fmt
  treefmt --allow-missing-formatter

[doc('build')]
build:
  cargo build --workspace --target x86_64-unknown-linux-musl --release

[doc('lint')]
lint: fmt
  cargo clippy --workspace --target x86_64-unknown-linux-musl --release

