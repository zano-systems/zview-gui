"""Owned native-app adapter: the same server, without requiring browser assets.

Stdout carries one readiness record; diagnostics go to stderr. Closing stdin
requests graceful shutdown, including when the native parent exits unexpectedly.
This module is internal to the native launcher, not a second public CLI.
"""

import argparse
import asyncio
import contextlib
import json
import signal
import sys
import threading

from . import __version__
from .console import Console
from .session import PROTOCOL_VERSION


async def run(data_root, ready_stream):
    from .server import serve

    stop = asyncio.Event()
    loop = asyncio.get_running_loop()

    def parent_closed():
        sys.stdin.buffer.readline()
        with contextlib.suppress(RuntimeError):
            loop.call_soon_threadsafe(stop.set)

    threading.Thread(target=parent_closed, daemon=True).start()
    for signum in (signal.SIGINT, signal.SIGTERM):
        with contextlib.suppress(NotImplementedError):
            loop.add_signal_handler(signum, stop.set)

    def ready(url):
        print(json.dumps({
            "url": url, "protocol": PROTOCOL_VERSION, "package_version": __version__,
        }), file=ready_stream, flush=True)

    await serve(
        data_root, port=0, period=0.1, heap_period=0.1, heap_cooldown=0.0,
        ready=ready, stop=stop, console=Console(stream=sys.stderr, color=False),
        announce_browser=False,
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--data-root", required=True)
    args = parser.parse_args()
    from zview.logging_setup import configure

    ready_stream = sys.stdout
    # ZView and probe libraries may print directly, including from worker
    # threads. Reserve the real stdout pipe exclusively for readiness.
    with contextlib.redirect_stdout(sys.stderr):
        configure()
        try:
            asyncio.run(run(args.data_root, ready_stream))
        except KeyboardInterrupt:
            pass


if __name__ == "__main__":
    main()
