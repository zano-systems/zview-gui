# SPDX-License-Identifier: Apache-2.0

"""``west zview-gui``: open the browser UI connected Live to the current build."""

import argparse
import os
import re
import sys
from pathlib import Path

from west.commands import WestCommand

ROOT = Path(__file__).resolve().parents[2]
SCRIPTS = ROOT / "scripts"
# What `west packages pip --install` installs for this module revision.
REQUIREMENTS = ROOT / "web-ui" / "requirements-west.txt"

EPILOG = """\
Auto-detected when omitted:
  -d/--build-dir      build.dir-fmt, the current directory, or ./build
  -e/--elf-file       from the build directory's runners.yaml
  -r/--runner         the build's debug-runner, then flash-runner
  -t/--runner-target  from the build directory's runners.yaml
  --data-root         the west workspace, if it holds the build and its sources

It runs the zview-gui package from PyPI, which ships the browser site. One-time
setup: `west packages pip --install`. --rebuild compiles the site from this
module's sources instead; that needs `just web-setup` in the module directory
(nightly Rust + wasm-bindgen).
"""


def required_version(requirements=REQUIREMENTS):
    """The zview-gui release this module revision installs."""
    match = re.search(r"^zview-gui\s*==\s*(\S+)\s*$", Path(requirements).read_text(), re.MULTILINE)
    if match is None:
        raise ValueError(f"No pinned zview-gui release in {requirements}.")
    return match.group(1)


class ZViewGuiCommand(WestCommand):
    def __init__(self):
        super().__init__(
            "zview-gui",
            "open the ZView browser UI connected to the current build",
            "Start the ZView GUI's local adapter, connect Live to the target "
            "described by the build directory, and open the browser.",
        )

    def do_add_parser(self, parser_adder):
        parser = parser_adder.add_parser(
            self.name,
            help=self.help,
            description=self.description,
            epilog=EPILOG,
            formatter_class=argparse.RawDescriptionHelpFormatter,
        )
        # Keep in step with zview_gui.cli.add_serve_options (checked by the
        # tests); the package may not be installed when west builds its help.
        parser.add_argument("-d", "--build-dir",
                            help="Zephyr build directory. Relative to --data-root when supplied, "
                                 "otherwise the current directory.")
        parser.add_argument("--domain", help="Firmware image of a sysbuild build.")
        parser.add_argument("-e", "--elf-file", help="ELF override.")
        parser.add_argument("-r", "--runner", help="Probe runner override (gdb, jlink, nrfutil, pyocd).")
        parser.add_argument("-t", "--runner-target", help="MCU/target override.")
        parser.add_argument("--frequency", type=int, help="Probe clock override, in Hz.")
        parser.add_argument("--period", type=float, default=0.1,
                            help="Live polling period in seconds; 0 polls back-to-back (default: 0.1).")
        parser.add_argument("--heap-period", type=float,
                            help="Minimum period between heap map captures, in seconds "
                                 "(default: --period, so fragmentation follows heap polling).")
        parser.add_argument("--heap-cooldown", type=float, default=0.0,
                            help="Minimum delay after a completed heap map, in seconds (default: 0).")
        parser.add_argument("--port", type=int,
                            help="Local port; 0 picks a free one (default: 8765, "
                                 "or a free port when 8765 is taken).")
        parser.add_argument("--data-root", help="Trusted directory the UI may read paths from.")
        parser.add_argument("--no-open", action="store_true",
                            help="Do not open the browser.")
        parser.add_argument("--rebuild", action="store_true",
                            help="Build the browser site from this module's sources and serve "
                                 "it instead of the packaged one (needs `just web-setup`).")
        return parser

    def do_run(self, args, unknown):
        del unknown
        build_dir = self._build_dir(args)
        cli = self._package()
        site = self._build_site() if args.rebuild else None
        try:
            cli.run(args, build_dir=build_dir, topdir=self.topdir, site=site,
                    color=self.color_ui)
        except cli.LaunchError as error:
            self.die(str(error))

    def _build_dir(self, args):
        zephyr_base = os.environ.get("ZEPHYR_BASE")
        if zephyr_base is None:
            self.die("ZEPHYR_BASE is not set. Run inside a Zephyr west workspace.")
        sys.path.append(str(Path(zephyr_base) / "scripts" / "west_commands"))
        from build_helpers import find_build_dir, is_zephyr_build

        if args.build_dir is None:
            build_dir = find_build_dir(None)
        else:
            build_dir = (Path(args.data_root or ".") / args.build_dir).resolve()
        if not is_zephyr_build(build_dir):
            self.die(f"'{build_dir}' is not a Zephyr build directory. "
                     "Run `west build` or pass -d.")
        return Path(build_dir).resolve()

    def _package(self):
        """The installed zview-gui CLI; warns when it is not this module's release."""
        try:
            import zview_gui
            from zview_gui import cli
        except ImportError as error:
            self.die(f"The zview-gui package is not installed ({error}). "
                     "Run `west packages pip --install`.")
        wanted = required_version()
        if zview_gui.__version__ != wanted:
            self.wrn(f"zview-gui {zview_gui.__version__} is installed, but this module "
                     f"revision expects {wanted}. Run `west packages pip --install`.")
        return cli

    def _build_site(self):
        sys.path.insert(0, str(SCRIPTS))
        import subprocess

        import web_ui

        try:
            web_ui.build()
        except ValueError as error:
            self.die(f"{error} (run it in {ROOT})")
        except (OSError, subprocess.CalledProcessError) as error:
            self.die(f"WASM build failed: {error}")
        return web_ui.SITE
