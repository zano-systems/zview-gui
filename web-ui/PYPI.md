# ZView GUI

A browser UI for [ZView](https://pypi.org/project/zview/): live and recorded
views of Zephyr RTOS threads, stacks, CPU load, heaps and other kernel objects.
The package holds a local-only adapter and the prebuilt WebAssembly UI, so no
Rust or Node toolchain is needed.

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

## Standalone

```sh
pip install zview-gui
zview-gui -d path/to/build        # Live, using the build's runners.yaml
zview-gui --data-root recordings  # Replay only
```

Live needs firmware built with ZView's configuration, such as the `zview`
snippet from [ZView's west module](https://github.com/wkhadgar/zview).

The server listens on `127.0.0.1` only (port 8765, or a free one when it is
taken) and reads files under its data root only. Press Ctrl+C to disconnect
the probe and stop it.

Source, issues and documentation:
<https://github.com/zano-systems/zview-gui>. Backed by
[Zano Systems](https://github.com/zano-systems). Apache License 2.0; the
bundled fonts are under the SIL Open Font License (`zview_gui/site/fonts/OFL.txt`).
