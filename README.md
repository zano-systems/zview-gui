# ZView GUI

An experimental native and browser application for exploring Zephyr RTOS thread
activity, kernel objects, and memory usage, built in Rust with
[GPUI Kit](https://gpui-kit.com/).

ZView GUI is a companion frontend to [ZView](https://github.com/rodrigopex/zview),
which provides data acquisition and decoding. Native and browser sessions both
require the Python adapter for Live acquisition and Replay.

## Status

**Under construction.** The application and its documentation are experimental
and may change. The project is not open for contributions at this stage.
The local Python web adapter supports Live connections and Replay. Live hardware
support requires the appropriate probe tools/drivers and hardware validation;
MCP integration is not implemented.

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

## Quick start

Install Rust, [just](https://just.systems/), and the
[GPUI Kit native prerequisites](https://gpui-kit.com/docs/installation).
Install the Python adapter as described in [Native Live and Replay](#native-live-and-replay).
From the repository root, start the adapter-backed native application:

```sh
just gui
```

For the browser, use the same Python 3.12 adapter environment and set up
the WASM build tools once. Then build and launch this checkout through the adapter:

```sh
just web-setup
just web-gui --python .venv-web/bin/python
```

Both applications start with an empty adapter session. Use Configure session
to connect Live or replay a ZView recording with its matching ELF/build.
The [benchmark guide](test-data/zview-bench-60s/README.md) provides a real
recording and matching ELF for Replay, with provenance and checksums.

`just gui` and bare `cargo run` require an adapter-backed native session.
`just gui --python /path/to/python` selects the adapter interpreter explicitly.
Native typography is unchanged; the browser bundles IBM Plex Sans and JetBrains Mono.

### Native Live and Replay

The native executable is named `zview-gui-rs`. `just gui` and `cargo run`
launch it; to select it explicitly, use `cargo run --bin zview-gui-rs`.
The Python/browser command remains `zview-gui`.

Native and web share the same Rust/GPUI UI foundation, Python adapter, session
configuration, Live/Replay controls and streaming protocol. Install the adapter
from this checkout into a separate Python 3.12 environment; no WASM build or
browser assets are needed for native:

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
Closing the main window quits the application, waits for
adapter startup if necessary, and requests graceful session cleanup. An owned
adapter that does not exit within five seconds is terminated and reaped.
System Quit also waits for owned-process cleanup.
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

Native and web use the same adapter-backed Live and Replay workflows. Replay
reads recordings generated by ZView with their matching ELF or build.

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

One-time setup, and again after `west update` moves the module to a new
release:

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

Outside west, the same package provides a `zview-gui` command with the same
options. It takes the build directory explicitly, and without `-d` it serves
Replay only, from `--data-root`:

```sh
pip install zview-gui
zview-gui -d path/to/build
zview-gui --data-root path/to/recordings
```

To install the **current checkout** like a release, but labelled as a development
version, use a dedicated Python 3.12 virtual environment:

```sh
just web-setup                          # once, for the WASM build tools
python3.12 -m venv .venv-web
just web-install-dev .venv-web/bin/python
.venv-web/bin/zview-gui --version         # zview-gui 0.4.1.dev0
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

### Browser startup and hosting

`just web-gui` builds this checkout's assets and launches the installed adapter
with `--site target/web`. Select its interpreter with `--python` or
`ZVIEW_GUI_PYTHON`; otherwise it uses the launcher's Python. Adapter options
such as `--data-root`, `--port` and `--no-open` are forwarded. Use `--skip-build`
when the current browser build is already complete.

The adapter prints a local URL and opens the browser. Keep the terminal running;
**Ctrl+C** stops the adapter. Recordings and ELF/build paths are local to the
adapter and constrained by its data root. A missing/disconnected adapter is
reported in the UI and retried; the session controls remain accessible.

Use a desktop-sized viewport of at least **1000 × 680**. GPUI prefers WebGPU
and falls back to WebGL2; browser/driver support and background-tab playback
timing can differ from native. This is not a mobile layout.

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
