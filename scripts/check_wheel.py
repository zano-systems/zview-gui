"""Check a built zview-gui wheel before it is published.

    python3 scripts/check_wheel.py contents dist/zview_gui-X.Y.Z-py3-none-any.whl
    <venv>/bin/python -I scripts/check_wheel.py serve   # with the wheel installed

``contents`` needs only the standard library. ``serve`` must run with the
installed package (``-I`` keeps a source checkout off ``sys.path``): it serves
the packaged site and fetches its page and WASM module.
"""

import asyncio
import re
import sys
import tempfile
import zipfile
from pathlib import Path

SITE = "zview_gui/site/"
REQUIRED = ("index.html", "main.js", "startup.mjs", "pkg/zview_desktop.js",
            "pkg/zview_desktop_bg.wasm", "fonts/OFL.txt")
# Nothing but the package, its site and the metadata belongs in the wheel.
ALLOWED = re.compile(r"zview_gui/(?:[a-z_]+\.py|site/.+)|zview_gui-[^/]+\.dist-info/.+")


def check_contents(wheel):
    with zipfile.ZipFile(wheel) as archive:
        names = archive.namelist()
        metadata = next(n for n in names if n.endswith(".dist-info/METADATA"))
        text = archive.read(metadata).decode()
        entry_points = archive.read(metadata.replace("METADATA", "entry_points.txt")).decode()
    problems = [f"missing {SITE}{name}" for name in REQUIRED if SITE + name not in names]
    problems += [f"retired browser loader asset {name}" for name in names
                 if name in {SITE + "documents.mjs", SITE + "initial.json", SITE + "session.json"}]
    problems += [f"unexpected {name}" for name in names if not ALLOWED.fullmatch(name)]
    if "zview-gui = zview_gui.cli:main" not in entry_points:
        problems.append("no zview-gui console script")
    if not re.search(r"^Requires-Dist: zview==\d", text, re.MULTILINE):
        problems.append("zview is not pinned")
    version = re.search(r"^Version: (\S+)$", text, re.MULTILINE).group(1)
    if not Path(wheel).name.startswith(f"zview_gui-{version}-py3-none-any"):
        problems.append(f"wheel name does not match version {version}")
    if problems:
        raise SystemExit(f"{wheel}: " + "; ".join(problems))
    print(f"{Path(wheel).name}: {len(names)} files, site complete, version {version}")


async def check_serving():
    from aiohttp import ClientSession
    from zview_gui import __version__, server
    from zview_gui.console import Console

    site = server.default_site()
    if site != server.PACKAGED_SITE:
        raise SystemExit(f"not the packaged site: {site}")
    stop, ready = asyncio.Event(), asyncio.get_running_loop().create_future()
    with tempfile.TemporaryDirectory() as root:
        task = asyncio.create_task(server.serve(
            root, port=0, ready=ready.set_result, stop=stop, console=Console(color=False)))
        url = await asyncio.wait_for(ready, 10)
        try:
            async with ClientSession() as client:
                for path, kind in (("", "text/html"), ("pkg/zview_desktop_bg.wasm", "application/wasm")):
                    async with client.get(url + path) as response:
                        body = await response.read()
                        if response.status != 200 or response.content_type != kind or not body:
                            raise SystemExit(f"{url}{path}: {response.status} {response.content_type}")
        finally:
            stop.set()
            await asyncio.wait_for(task, 10)
    print(f"zview-gui {__version__} serves its packaged site from {site}")


def main(argv):
    if len(argv) == 2 and argv[0] == "contents":
        check_contents(argv[1])
    elif argv == ["serve"]:
        asyncio.run(check_serving())
    else:
        raise SystemExit(__doc__)


if __name__ == "__main__":
    main(sys.argv[1:])
