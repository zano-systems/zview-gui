# ZView GUI

A browser UI for [ZView](https://pypi.org/project/zview/): live and recorded
views of Zephyr RTOS threads, stacks, CPU load, heaps and other kernel objects.
The package holds a local-only adapter and the prebuilt WebAssembly UI, so no
Rust or Node toolchain is needed.

## Install and run

In a Python 3.10 or newer virtual environment:

```sh
python -m pip install zview-gui
zview-gui
```

This opens the UI in your browser. Use a desktop browser with WebGPU or WebGL2.
To replay a recording, select it and its matching ELF or build in **Configure
session**. Files must be under the working directory by default; pass
`--data-root path/to/recordings` to choose a different root.

To connect Live at startup, run `zview-gui -d path/to/build`. Use matching
firmware already running on the target, built with ZView's configuration (for
example, the `zview` snippet from
[ZView's west module](https://github.com/wkhadgar/zview)), and the appropriate
probe tools and drivers.

The server listens on `127.0.0.1` only (port 8765, or a free one when it is
taken). Press Ctrl+C to disconnect the probe and stop the server. For
virtual environment setup and more details, see the
[Quick start](https://github.com/zano-systems/zview-gui#quick-start).

## In a west workspace

Add the `zview-gui` module and ZView's own module (at the release this package
pins; it provides the `zview` build snippet) to your west manifest, as shown in
the [project README](https://github.com/zano-systems/zview-gui#west-integration-west-zview-gui).
Then:

```sh
west update zview zview-gui && west packages pip --install
west build -b <board> app -S zview
west zview-gui
```

`west zview-gui` finds the build, connects Live through the build's debug
runner and opens the browser.

See the [project repository](https://github.com/zano-systems/zview-gui) for
source and documentation. Share feedback about the browser UI in the
[issue tracker](https://github.com/zano-systems/zview-gui/issues). Backed by
[Zano Systems](https://github.com/zano-systems). Apache License 2.0; the
bundled fonts are under the SIL Open Font License (`zview_gui/site/fonts/OFL.txt`).
