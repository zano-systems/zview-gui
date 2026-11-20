"""Adds the built browser site to zview-gui wheels as ``zview_gui/site``.

Standard wheels need the site in target/web (``just web-build``, or the release
workflow's optimized build); building one without it fails. Editable installs
skip it: they serve target/web from the checkout directly.
"""

import os
import runpy
from pathlib import Path

from hatchling.builders.hooks.plugin.interface import BuildHookInterface

# What the browser loads; target/web also holds Cargo's "web" profile output.
SITE_FILES = ("index.html", "main.js", "startup.mjs")
SITE_DIRS = ("pkg", "fonts")


class SiteHook(BuildHookInterface):
    PLUGIN_NAME = "zview-gui-site"

    def initialize(self, version, build_data):
        if self.target_name != "wheel" or version == "editable":
            return
        site = Path(self.root, os.environ.get("ZVIEW_GUI_SITE", "target/web"))
        manifest = Path(self.root, "web-ui/zview_gui/site_assets.py")
        missing = runpy.run_path(str(manifest))["missing_assets"](site)
        if missing:
            raise RuntimeError(
                f"The browser site in {site} is incomplete (missing {', '.join(missing)}). "
                "Build it first: `just web-setup && just web-build`.")
        for name in (*SITE_FILES, *SITE_DIRS):
            if (site / name).exists():
                build_data["force_include"][str(site / name)] = f"zview_gui/site/{name}"
