# build both indexer and database rust binaries
build-all:
    cargo build --release --package indexer && cargo build --release --package storage

# build only indexer binary
build-indexer:
    cargo build --release --package indexer

# build only database binary
build-storage:
    cargo build --release --package storage

# formatting check
fmt:
    cargo fmt --all --check

# run linter
clippy:
    cargo clippy -- -D warnings