"""Build and install the checkout as a development wheel, not an editable package.

Run with the destination virtual environment's Python. Release metadata stays
unchanged: only a temporary packaging tree gets the PEP 440 ``.dev0`` suffix.
"""

import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile

from check_wheel import check_contents
from web_ui import ROOT, SITE, build


def stage_project(root, destination):
    """Copy packaging inputs without fixtures, generated output or local config."""
    metadata = (root / "pyproject.toml").read_text(encoding="utf-8")
    pattern = r'(?m)^version = "(\d+\.\d+\.\d+)"$'
    match = re.search(pattern, metadata)
    if match is None:
        raise ValueError("Expected a stable X.Y.Z version in pyproject.toml.")
    version = match[1] + ".dev0"
    (destination / "pyproject.toml").write_text(
        re.sub(pattern, f'version = "{version}"', metadata), encoding="utf-8")
    for name in ("hatch_build.py", "LICENSE", "NOTICE", "web-ui/PYPI.md"):
        target = destination / name
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(root / name, target)
    shutil.copytree(
        root / "web-ui" / "zview_gui", destination / "web-ui" / "zview_gui",
        ignore=shutil.ignore_patterns("__pycache__", "*.pyc", "site"))
    return version


def build_wheel(root, site, output):
    """Use the normal build backend and site hook, with only the version changed."""
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="zview-gui-dev-") as directory:
        stage = Path(directory)
        version = stage_project(root, stage)
        subprocess.run([
            sys.executable, "-m", "pip", "wheel", "--no-deps", "--no-cache-dir",
            "--wheel-dir", str(output), str(stage),
        ], env={**os.environ, "ZVIEW_GUI_SITE": str(site.resolve())}, check=True)
    wheel = output / f"zview_gui-{version}-py3-none-any.whl"
    check_contents(wheel)
    return wheel


def main():
    if sys.prefix == sys.base_prefix:
        raise SystemExit(
            "Use a virtual environment: just web-install-dev .venv-web/bin/python")
    build(release=True)
    wheel = build_wheel(ROOT, SITE, ROOT / "target" / "dev-dist")
    # Repeated installs must replace the same .dev0, including the embedded site.
    # Normal dependency resolution keeps the release package's backend contract.
    subprocess.run([
        sys.executable, "-m", "pip", "install", "--force-reinstall", str(wheel),
    ], check=True)
    subprocess.run([sys.executable, "-m", "pip", "check"], check=True)
    subprocess.run([sys.executable, "-I", "-m", "zview_gui", "--version"], check=True)


if __name__ == "__main__":
    main()
