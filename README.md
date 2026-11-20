# ZView GUI

An experimental native and browser application for exploring Zephyr RTOS thread
activity, kernel objects, and memory usage, built in Rust with
[GPUI Kit](https://gpui-kit.com/).

ZView GUI is a companion frontend to [ZView](https://github.com/rodrigopex/zview),
which provides data acquisition and decoding. The browser UI is distributed as
[`zview-gui` on PyPI](https://pypi.org/project/zview-gui/). The native UI is
available from source for development; no native binaries are distributed.

## Status

**Under construction.** The application and its documentation are experimental
and may change. Share feedback about the browser UI or report bugs in the
[issue tracker](https://github.com/zano-systems/zview-gui/issues). The project is
not open for code contributions at this stage.
The local Python web adapter supports Live connections and Replay. Live hardware
support requires the appropriate probe tools/drivers and hardware validation;
MCP integration is not implemented.

## Quick start

Install `zview-gui` from PyPI to run the browser UI. The package includes the
local Python adapter and prebuilt browser UI, so no checkout, Rust toolchain,
WASM build, or west workspace is required. Use Python 3.10 or newer; Python 3.12
is tested in release CI.

```sh
python3 -m venv .venv-zview
.venv-zview/bin/python -m pip install zview-gui
.venv-zview/bin/zview-gui
```

On Windows, create the environment with `py -m venv .venv-zview` and use
`.venv-zview\Scripts\python.exe` and `.venv-zview\Scripts\zview-gui.exe` in the
last two commands.

The command prints a local URL and opens the browser. Keep the terminal running;
press **Ctrl+C** to stop the adapter. Use a desktop browser with WebGPU or WebGL2
and a viewport of at least **1000 × 680**.

- **Replay:** Open **Configure session** and select a recording and its matching
  ELF or build. The adapter reads files under the working directory by default;
  pass `--data-root /path/to/recordings` to choose another directory. The
  [benchmark guide](test-data/zview-bench-60s/README.md) provides a recording
  and matching ELF to try.
- **Live:** Configure the connection in the sidebar, or launch with
  `zview-gui -d path/to/build` to connect at startup. Use matching firmware
  already running on the target and the appropriate probe tools and drivers.
  Connecting does not flash firmware.

If you already use a Zephyr workspace, see the optional
[west integration](#west-integration-west-zview-gui).

## Current capabilities

- Adapter-backed Live acquisition and recorded-session playback in native and web.
- Threads overview and per-thread runtime and stack details.
- Thread lists default to descending Load %. Select a column heading to sort;
  select it again to reverse direction. Name sorting keeps rows stable while
  live usage changes. The choice is retained across frames and screen navigation
  for the current application instance. Missing readings sort last.
- Kernel objects overview with search and type filters. The sidebar hides empty
  types using live discovery when available, otherwise the selected snapshot.
  If the selected type becomes empty, the filter returns to All types.
- Detail views for semaphores, message queues, mutexes, memory slabs,
  work queues, and heaps. Use the Kernel objects/type breadcrumbs to navigate
  to a parent list, or Esc to return while preserving the list's search/filter.
  Clickable breadcrumbs have rounded hover backgrounds and keyboard-focus
  indicators; current-page labels and separators remain noninteractive.
- Info icons beside table and detail headings explain sampled measurements and
  state legends on hover or keyboard focus. Esc dismisses a focused explanation.
  Current warnings and missing-data notices remain visible.
- Time-history charts, work queue activity, and captured heap fragmentation maps.
- One shared Rust UI for native and WebAssembly builds. The required
  [Python session adapter](web-ui/README.md) streams live target measurements or
  decoded ZView recordings through HTTP commands and WebSocket events.
- The adapter UI has persistent sidebar session controls, with editable
  Live/Replay settings, explicit Connect/Disconnect/Reconnect, and observed live
  samples/s (successful frames received by the adapter). Build directories
  supply board/image/ELF context; Git details are labelled as checkout metadata,
  not proof of the firmware's build revision.
- Adapter sessions show status, icon-only actions, source and Live history cache
  in the sidebar's bottom panel, rather than occupying the main toolbar.
  The collapsed session-status button expands the sidebar to show those controls.
  Navigation scrolls separately and long session panels scroll on short windows.
- Adapter sessions show source information in the sidebar rather than repeating
  it in the footer. Live sessions also show a current-view history cache meter:
  estimated memory usage/budget and retained duration/time limit. The cache resets
  on screen changes and is not total application memory. Playback controls
  remain in the footer.
- Session actions use icons with tooltips and accessible names. A green circle means fresh frames are
  arriving; busy, waiting/warning, failed and disconnected states have distinct
  indicators and text, not color alone.
- Opening Live heap details requests one heap's fragmentation map on demand.
  One controlling tab selects only the groups and rows shown by its screen.
  Leaving heap details releases map demand. Acquisition periods are configurable;
  default map capture includes a one-second cooldown.
- Live charts retain up to five minutes of change-aware history, independent of
  polling rate, starting fresh at zero on each screen visit. The visible range
  grows up to a 20-second rolling window, with per-chart wheel zoom, accurate
  visible-duration labels, and explicit reset/fit actions. Percentage
  charts connect real readings; count/state charts retain steps. Observation
  gaps remain gaps; dense views preserve extrema using pixel envelopes.
  A chart-memory safeguard explicitly reports any shortened retention.

Native and web share a 32-point application header, separate from navigation.
Native integrates the OS window controls and dragging; web does not draw them.
The sidebar toggle stays fixed at the left, with About and Help & shortcuts at
the right of the leading section. The separator aligns with the sidebar edge
when expanded. When collapsed, the leading section fits those controls, followed
by the separator and breadcrumbs; the sidebar itself becomes a narrow icon rail.
Header sizing lives in `src/header.rs`. Native window-control spacing does not
widen the sidebar.

Kernel objects navigation starts expanded on both hosts and can be collapsed
manually. Fixed "Backed by Zano" attribution sits above the session controls in
the shared sidebar footer and links to https://zano.systems/ (the logo is the
link when collapsed).
All session actions use the shared sidebar controls, with the collapsed session
icon centered on the navigation icons. Activating it by mouse or keyboard expands
the sidebar and focuses the session controls.

## West integration (`west zview-gui`)

In a Zephyr workspace, ZView GUI is a west module whose `west zview-gui` command
starts the [Python session adapter](web-ui/README.md), connects Live to the
target described by the build directory, and opens the browser. The adapter
and the prebuilt browser UI ship together as the
[`zview-gui` package on PyPI](https://pypi.org/project/zview-gui/), which pins
the ZView release it is tested with (`zview==0.28.1`). The firmware side still
needs ZView's own west module: it provides the `zview` snippet that configures
the build (`-S zview`) and the `west zview` command. Keep it at the same
release; the adapter's Python code always comes from the PyPI pin, not from
this checkout. Add both modules to your manifest:

```yaml
manifest:
  remotes:
    - name: wkhadgar
      url-base: https://github.com/wkhadgar
    - name: zano-systems
      url-base: https://github.com/zano-systems
  projects:
    - name: zview
      remote: wkhadgar
      revision: v0.28.1  # the release zview-gui pins
      path: modules/tools/zview
      west-commands: scripts/west-commands.yml
    - name: zview-gui
      remote: zano-systems
      revision: main
      path: modules/tools/zview-gui
      west-commands: scripts/west-commands.yml
```

Use a GUI module revision whose pinned package is published on PyPI. If `main`
pins an unreleased version, select a published release tag instead. Run this
setup once, and again after `west update` moves the module to a new release:

```sh
west update zview zview-gui
west packages pip --install   # installs zview-gui==<module version> and ZView
```

No Rust or WASM toolchain is needed. `west zview-gui` runs the installed
package and warns when it is not the release the module revision expects.
Contributors who change the UI build the site from the module's sources
instead, with the WASM tools installed once:

```sh
(cd modules/tools/zview-gui && just web-setup)
west zview-gui --rebuild
```

Build with the ZView snippet and launch:

```sh
west build -b <board> app -S zview
west zview-gui                  # build dir from build.dir-fmt, cwd, or ./build
west zview-gui -d other-build --no-open --port 0
west zview-gui -r gdb -t 127.0.0.1:1234   # e.g. QEMU's gdbstub (no runners.yaml)
```

Runner, target and probe clock come from the build's `runners.yaml`
(`debug-runner` before `flash-runner`). Override them with `-r/-t/--frequency`,
the ELF with `-e`, or the sysbuild image with `--domain`. The adapter may only
read paths under its data root. That root is the west workspace when it holds
the build and the application sources, otherwise their nearest common directory;
`--data-root` overrides it. If auto-connect fails, the error is printed and the
server keeps running so you can fix the settings in **Configure…**. `--rebuild`
recompiles the WASM site incrementally into the module's `target/web` and serves
that instead of the packaged site.
It listens on `127.0.0.1:8765`, or on a free port (printed) when 8765 is taken;
an explicit `--port` that is taken is an error.
**Ctrl+C** disconnects the probe and stops the server.
While it runs, the terminal reports Live status changes (reading, lost, failed,
stopped) and read notices, coloured by level, with the page URL highlighted.
Set `NO_COLOR=1`, or `west config color.ui false`, for plain output.
Heap fragmentation maps are captured only while a heap's details are open; by
default they follow the polling period (`--period`, 0.1 s) with no cooldown, so
the fragmentation ratio updates as often as the heap statistics. Each map walks
every heap chunk over the probe, so it lowers the overall frame rate while that
view is open; `--heap-period`/`--heap-cooldown` throttle it. ZView's warnings,
such as resynced broken frames, go to its log file (printed at startup).

## Upgrading from 0.4.x

In 0.5, both UIs require the Python adapter for Live and Replay. Direct JSON
imports and startup with a bundled sample are no longer supported. To replay a
ZView recording, select it and its matching ELF or build in **Configure session**.
For source builds, use `just web-gui` or `just gui` instead of `just web-ui` or
`just ui`. The native executable is now named `zview-gui-rs`.

## Development

### Browser builds

To install the **current checkout** like a release, but labelled as a development
version, use a dedicated Python 3.12 virtual environment:

```sh
just web-setup                          # once, for the WASM build tools
python3.12 -m venv .venv-web
just web-install-dev .venv-web/bin/python
.venv-web/bin/zview-gui --version         # zview-gui 0.5.0.dev0
.venv-web/bin/zview-gui -d path/to/build
```

This rebuilds the optimized browser UI and pip-installs a regular wheel with
the same dependencies, entry point and bundled site as a release. The version
is the checkout's release version plus `.dev0` (pip's spelling of `-dev`).
It does not publish to PyPI or change tracked version files. Re-running replaces
the previous development install and reinstalls its dependencies; use a dedicated
environment. The wheel is retained in `target/dev-dist/`.
With a virtual environment activated, `just web-install-dev` uses its `python3`.
On Windows, pass `.venv-web/Scripts/python.exe`.
The west module still expects its release pin and may warn about the dev version;
`west packages pip --install` restores the pinned release.

#### Browser startup and hosting

`just web-gui` builds this checkout's assets and launches the installed adapter
with `--site target/web`. Select its interpreter with `--python` or
`ZVIEW_GUI_PYTHON`; otherwise it uses the launcher's Python. Adapter options
such as `--data-root`, `--port` and `--no-open` are forwarded. Use `--skip-build`
when the current browser build is already complete.

GPUI prefers WebGPU and falls back to WebGL2. Browser/driver support and
background-tab playback timing can differ from native. A missing or disconnected
adapter is reported in the UI and retried; the session controls remain accessible.

GPUI Kit 0.7.3's web dependencies require nightly Rust. `web-setup` installs
the WASM target and matching wasm-bindgen CLI under `target/tools`; native
builds remain on stable Rust. Set `ZVIEW_WEB_TOOLCHAIN` to override the web
toolchain. The first build downloads pinned, checksum-verified fonts and Rust
dependencies. Node/npm is not needed to launch, but the Python adapter is required.

```sh
just web-build
```

Serve `target/web/`, including `pkg/`, `startup.mjs` and `fonts/`, through the
Python adapter using `--site target/web`. Keep `fonts/OFL.txt` with the
distribution. Browser assets do not contain a session or sample dataset.

### Native Live and Replay

Native is a development workflow, not a distributed install. From the
repository root, use `just gui` or `cargo run` to launch `zview-gui-rs`.
Install Rust and the
[GPUI Kit native prerequisites](https://gpui-kit.com/docs/installation),
plus [just](https://just.systems/) if using `just gui`.

Native and web share the same Rust/GPUI UI, Python adapter and session controls.
Install the adapter from this checkout into a separate Python 3.12 environment;
no WASM build or browser assets are needed for native:

```sh
python3.12 -m venv .venv-web
.venv-web/bin/python -m pip install -r web-ui/requirements.txt
.venv-web/bin/python -m pip install --no-deps -e .
just gui --python .venv-web/bin/python
```

Use **Configure session** in the sidebar to choose Live or Replay. Replay needs
the recording's matching ELF or build; Live needs matching firmware already
running. Connecting does not flash firmware.

To connect Live automatically at startup, pass `--build-dir` (or `-d`):

```sh
just gui --python .venv-web/bin/python \
  --data-root "/path/to/project workspace" \
  --build-dir "build"
```

Relative build paths are resolved under `--data-root`; absolute paths must be
inside it. The default data root remains the working directory. The adapter
resolves the build's ELF, runner and target using the same rules as Configure
session. Auto-connect runs once after the first valid adapter snapshot; stream
reconnects do not restart acquisition. Build/probe errors remain visible in the
UI so settings can be corrected manually. No build, flash or reset is performed.
This startup option is for the native-owned adapter and cannot be combined with
`--adapter`; attached adapters remain controlled through the session UI.

Native starts its own loopback adapter on a free port and shuts it down when
the window closes. Its terminal retains readiness and session diagnostics
without browser URL or browser-server instructions.
Closing the main window or choosing **Quit ZView** (Command+Q on macOS,
Ctrl+Q on Windows and Linux) cancels pending adapter startup and keeps the UI
responsive while the owned adapter shuts down. An owned adapter that does not
exit within five seconds is terminated and reaped before ZView quits.
If the OS initiates termination without going through ZView's Quit command,
GPUI cannot defer it, so the fallback waits synchronously for cleanup.
`--python` overrides `ZVIEW_GUI_PYTHON`; otherwise it uses
`python3`. Missing dependencies or startup failures are reported in the UI.

The adapter's data root defaults to the working directory. Pass
`--data-root /path/to/workspace` when the recording, build or ELF is elsewhere.
The browser adapter's path and symlink restrictions also apply to native.
To share an explicitly selected running adapter instead:

```sh
cargo run -- --adapter http://127.0.0.1:8765
```

Closing native never stops an adapter it attached to. Native and browser clients
share the existing controlling-client rule: one client controls Live view
selection at a time. Remote adapter connections are not supported.

## Validation

```sh
cargo fmt --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
node --test web/startup.test.mjs
python3 -m unittest discover -s scripts -p 'test_*.py' -v
cargo +nightly clippy --locked --lib --target wasm32-unknown-unknown -- -D warnings
just web-build
```

The full-data heap-map test runs in the normal Rust suite without Python.
For native adapter integration tests, set `ZVIEW_GUI_PYTHON` to an interpreter
with this checkout's adapter installed before running
`cargo test --locked -- --include-ignored`. CI separately regenerates the
official fixture using its pinned exporter environment and compares it with
the committed JSON.

Builds and headless tests do not replace browser/native visual checks. Inspect
adapter failure/recovery, navigation, playback, heap maps, zoom, and resizing.
Firefox/Safari and hosted CI still require validation; the browser CI job uses
a rolling nightly toolchain. See [CONTRIBUTING.md](CONTRIBUTING.md) for maintainer
conventions.
