"""Required browser assets for the server, launcher and wheel tools.

Keep this module dependency-free: build tools load it before installing the
adapter, including in isolated Python.
"""

from pathlib import Path

REQUIRED_ASSETS = (
    "index.html", "main.js", "startup.mjs",
    "pkg/zview_desktop.js", "pkg/zview_desktop_bg.wasm",
    "fonts/OFL.txt",
    "fonts/IBMPlexSans-Regular.source.ttf",
    "fonts/JetBrainsMono-Regular.source.ttf",
)


def missing_assets(site):
    site = Path(site)
    return [name for name in REQUIRED_ASSETS if not (site / name).is_file()]
