set positional-arguments

# Run the native UI with the official full-data test session, or another JSON.
ui file="test-data/zview-bench-60s/session-with-heap-maps.json":
    if [ -n "$1" ]; then cargo run -- "$1"; else cargo run; fi

# Open the browser UI with the official test session. Ctrl+C stops the server.
web-ui file="test-data/zview-bench-60s/session-with-heap-maps.json":
    python3 scripts/web_ui.py serve "$1"

# Generate a standalone static site in target/web (no user data included).
web-build:
    python3 scripts/web_ui.py build --release

# Build and pip-install a packaged development version into a virtual environment.
web-install-dev python="python3":
    "{{python}}" scripts/install_dev.py

# Install the browser build tools; wasm-bindgen stays inside target/tools.
web-setup:
    rustup toolchain install nightly --profile minimal --target wasm32-unknown-unknown
    cargo install wasm-bindgen-cli --version 0.2.129 --locked --root target/tools
