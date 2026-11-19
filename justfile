set positional-arguments

# Run the native Python-adapter UI; forward explicit native CLI options.
gui *args:
    cargo run -- "$@"

# Build and open the adapter-backed browser UI. Ctrl+C stops the adapter.
web-gui *args:
    python3 scripts/web_ui.py serve "$@"

# Generate browser assets in target/web for the Python adapter.
web-build:
    python3 scripts/web_ui.py build --release

# Build and pip-install a packaged development version into a virtual environment.
web-install-dev python="python3":
    "{{python}}" scripts/install_dev.py

# Install the browser build tools; wasm-bindgen stays inside target/tools.
web-setup:
    rustup toolchain install nightly --profile minimal --target wasm32-unknown-unknown
    cargo install wasm-bindgen-cli --version 0.2.129 --locked --root target/tools
