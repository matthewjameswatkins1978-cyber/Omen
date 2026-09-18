default:
    cargo xtask --help

verify:
    cargo xtask verify

check:
    cargo check --workspace --all-targets

test:
    cargo test --workspace

lint:
    cargo fmt --check
    cargo clippy --workspace --all-targets --all-features -- -D warnings

schema-gen:
    cargo xtask generate-schemas

schema-verify:
    cargo xtask verify-schemas
