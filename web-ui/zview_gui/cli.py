"""``zview-gui``: serve the browser UI, optionally connected Live to a Zephyr build.

``west zview-gui`` finds the build directory and calls :func:`run` with the same
options; this command takes the build directory explicitly.
"""

import argparse
import asyncio
import sys
from pathlib import Path

from . import __version__
from .console import Console, wants_color
from .launch import LIVE_OVERRIDES, data_root_for, heap_period, live_request

EPILOG = """\
Without -d, it serves Replay only, reading files under --data-root (default:
the current directory). With -d, it connects Live to that build; the ELF,
runner, target and probe clock come from its runners.yaml unless overridden,
and the data root defaults to the build and its application sources.
"""


class LaunchError(Exception):
    """A setup problem to report as a one-line error, without a traceback."""


def add_serve_options(parser):
    """The options ``zview-gui`` and ``west zview-gui`` share (same dests)."""
    parser.add_argument("-d", "--build-dir", help="Zephyr build directory to connect Live to.")
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
    parser.add_argument("--no-open", action="store_true", help="Do not open the browser.")


def build_parser():
    parser = argparse.ArgumentParser(
        prog="zview-gui", description=__doc__.split("\n\n")[0], epilog=EPILOG,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    add_serve_options(parser)
    parser.add_argument("--site", type=Path,
                        help="Serve this browser site instead of the packaged one.")
    parser.add_argument("--version", action="version", version=f"%(prog)s {__version__}")
    return parser


# The option dest that sets each Live override.
OVERRIDE_DESTS = {"domain": "domain", "elf": "elf_file", "runner": "runner",
                  "target": "runner_target", "frequency": "frequency"}


def _overrides(args):
    return {key: getattr(args, OVERRIDE_DESTS[key]) for key in LIVE_OVERRIDES}


def run(args, *, build_dir=None, topdir=None, site=None, color=True):
    """Serve until Ctrl+C. ``build_dir`` (resolved) enables Live; raises LaunchError."""
    from zview.logging_setup import configure as configure_logging

    from . import server

    overrides = _overrides(args)
    # Like `west zview`: ZView's warnings (e.g. resynced broken frames) go to
    # its rotating log file instead of flooding the terminal.
    log = configure_logging()
    console = Console(color=color and wants_color(sys.stdout))
    console.field("ZView log", log)

    live = None
    if build_dir is None:
        if any(value is not None for value in overrides.values()):
            raise LaunchError("-e, -r, -t, --domain and --frequency need a build directory (-d).")
        root = Path(args.data_root or ".").resolve()
    else:
        extra = [args.elf_file] if args.elf_file else []
        try:
            root = (Path(args.data_root).resolve() if args.data_root
                    else data_root_for(build_dir, topdir, extra))
            live = live_request(root, build_dir, **overrides)
        except ValueError as error:
            raise LaunchError(f"{error} Pass --data-root containing the build and ELF.") from None
        console.field("Build directory", build_dir)
    console.field("Data root", root)

    site = Path(site).resolve() if site is not None else server.default_site()
    if not server.site_ready(site):
        raise LaunchError(f"No browser site in {site}. Reinstall zview-gui, or build the "
                          "site from a source checkout with `just web-setup && just web-build`.")
    if site != server.PACKAGED_SITE:
        console.field("Browser site", site)

    try:
        asyncio.run(server.serve(
            root, port=args.port, period=args.period,
            heap_period=heap_period(args.period, args.heap_period),
            heap_cooldown=args.heap_cooldown, live=live, open_browser=not args.no_open,
            console=console, site=site,
        ))
    except KeyboardInterrupt:
        pass
    except (OSError, ValueError) as error:
        raise LaunchError(str(error)) from None


def main(argv=None):
    parser = build_parser()
    args = parser.parse_args(argv)
    build_dir = None
    if args.build_dir is not None:
        build_dir = Path(args.build_dir).resolve()
        if not (build_dir / "CMakeCache.txt").is_file():
            parser.error(f"'{args.build_dir}' is not a build directory (no CMakeCache.txt).")
    try:
        run(args, build_dir=build_dir, site=args.site)
    except LaunchError as error:
        print(f"zview-gui: error: {error}", file=sys.stderr)
        return 1
    return 0
