# Contributing

ZView GUI is experimental and under construction. The project is not open for
contributions at this stage. The conventions below are retained for maintainers.

## Versions

Use [Semantic Versioning](https://semver.org/). One version covers the
application and its Python package. Change it in all of these together (the
tests check they agree):

- `Cargo.toml`, and its package entry in `Cargo.lock`;
- `pyproject.toml`;
- the `zview-gui==` pin in `web-ui/requirements-west.txt`, which is what
  `west packages pip --install` installs for a module revision.

The initial experimental version is `0.1.0`. During `0.x` development:

- Patch versions contain backward-compatible fixes.
- Minor versions contain features or breaking changes. Document any breaking
  change and migration steps explicitly.
- Move to `1.0.0` when the supported interfaces and compatibility promises are
  stable. After that, breaking changes require a major version.

Supported interfaces include document formats, command-line arguments, and
documented backend integration. A UI layout adjustment alone is not necessarily
a breaking change.

Do not bump the version on every commit. Update it deliberately for a release;
use `vX.Y.Z` release tags only when publishing that release.

## Releases

Pushing a `vX.Y.Z` tag that matches `pyproject.toml` runs
`.github/workflows/release.yml`. It builds the size-optimized browser site
(`web-release` profile plus a pinned `wasm-opt`), runs the adapter tests and
the browser smoke test, packs the site into the `zview-gui` wheel, checks the
installed wheel serves it, and publishes the wheel to PyPI through trusted
publishing from the `pypi` environment. Running the workflow by hand does
everything except publishing and uploads the wheel as an artifact.

PyPI never accepts the same version twice, so a broken release is fixed with
a new patch version. Until the release is on PyPI, a module revision that pins
it cannot be installed with `west packages pip --install`: tag and publish
before pointing users at that revision.

To build a wheel locally, build the site first (`just web-build`), then run
`python -m build --wheel` and `python3 scripts/check_wheel.py contents dist/*.whl`.
A wheel build fails if the site in `target/web` is incomplete.

For a local packaged development install, run
`just web-install-dev .venv-web/bin/python` after `just web-setup`.
It builds the optimized site and installs a regular wheel with version
`<release version>.dev0` into the selected virtual environment. Only a temporary
packaging tree receives that version; release files and publishing are unchanged.
Use a dedicated environment: repeated runs replace the development package and
reinstall its dependencies. See the main README for setup and launch commands.

Native adapter integration can be checked without hardware or a browser build.
Install this checkout's adapter in the separate Python 3.12 environment described
in `web-ui/README.md`, then run:

```sh
ZVIEW_GUI_PYTHON="$PWD/.venv-web/bin/python" cargo test --locked --lib \
  native_transport_replays_through_owned_python_adapter -- --ignored
```

This exercises owned-process readiness, the native HTTP/WebSocket transport,
real recording replay and reconnect snapshot recovery. The normal Python suite
also checks parent-EOF shutdown; Rust UI tests cover shared native session
controls. These do not replace native/browser visual checks or Live hardware
validation. Do not point `ZVIEW_PYTHON` at this adapter environment: the legacy
recording exporter has a different backend contract.

## Commits

Use [Conventional Commits](https://www.conventionalcommits.org/):

```text
type(optional-scope): concise description
```

Use `feat` for features, `fix` for bug fixes, and `docs`, `test`, `refactor`,
`build`, `ci`, or `chore` as appropriate. Mark breaking changes with `!` and
explain them in a `BREAKING CHANGE:` footer.

Examples:

```text
feat(heap): add fragmentation map inspection
fix(recording): retain the current session after failed imports
docs: explain west module setup
```

Maintainers should work on a separate branch and open a pull request rather than
pushing directly to `main`. Before requesting review, run `cargo fmt --check`,
`cargo test`, and `cargo clippy --all-targets -- -D warnings`. Report any manual
visual or platform checks that were not performed.
