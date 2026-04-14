# tldr — Mattermost chat summarisation

# Start daemon with auto-reload using debug build (requires: cargo install systemfd cargo-watch)
dev:
    RUST_LOG=debug systemfd --no-pid -s http::8765 -- cargo watch -x 'run --bin tldr-daemon'

# Start frontend dev server with HMR
dev-frontend:
    cd frontend && npm run dev

# Stop the dev server (kills systemfd, cargo-watch and tldr-daemon on port 8765)
stop:
    -pkill -f 'systemfd.*8765'
    -pkill -f 'cargo-watch'
    -fuser -k 8765/tcp

# Run both daemon and frontend dev servers
dev-all:
    just dev &
    just dev-frontend

# Build debug binaries and frontend
build:
    cargo build
    cd frontend && npm run build

# Build release (optimised) binaries and frontend
build-release:
    cargo build --release
    cd frontend && npm run build

# Run tests
test:
    cargo test

# Quick type-check (backend + frontend)
check:
    cargo check
    cd frontend && npx svelte-check --output human

# Run all tests and builds (CI equivalent)
test-all:
    cargo test
    cd frontend && npm run build

# Lint
lint:
    cargo clippy -- -D warnings
    cargo fmt --check
    cd frontend && npx svelte-check --output human

# Format code
fmt:
    cargo fmt

# Clean build artifacts
clean:
    cargo clean
    rm -rf frontend/dist frontend/node_modules

# Install frontend dependencies
frontend-install:
    cd frontend && npm install

# Run the daemon directly (no auto-reload)
daemon *ARGS:
    RUST_LOG=debug cargo run --bin tldr-daemon -- {{ARGS}}

# Run the CLI
cli *ARGS:
    cargo run --bin tldr-cli -- {{ARGS}}

# Summarise via CLI
summarise *ARGS:
    cargo run --bin tldr-cli -- summarise {{ARGS}}
