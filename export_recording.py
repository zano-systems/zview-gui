"""Regenerate chart test fixtures from a ZView recording and its matching ELF.

Run from the repository root in CI's pinned exporter environment.
"""

import argparse
import json
import math
import queue
import threading
from pathlib import Path

from zview.backend.replay import ReplayScraper
from zview.orchestrator import ZScraper
from zview.snapshot import serialize_frame


class TimedReplay(ReplayScraper):
    """Observe the replay clock without waiting for wall-clock playback."""

    recording_time = None

    def _pace_to(self, recording_t):
        self.recording_time = recording_t


class TimestampQueue(queue.Queue):
    """Attach time on the producer thread, before replay advances to another frame."""

    def __init__(self, backend):
        super().__init__()
        self.backend = backend

    def put(self, item, block=True, timeout=None):
        if "threads" in item:
            item = dict(item, recording_time=self.backend.recording_time)
        super().put(item, block=block, timeout=timeout)


def export(recording: Path, elf: Path, duration: float) -> list[dict]:
    frames = []
    with TimedReplay(recording, honor_timing=True) as backend:
        scraper = ZScraper(backend, elf)
        scraper.update_available_threads()
        scraper.reset_thread_pool()
        messages = TimestampQueue(backend)
        stop = threading.Event()
        scraper.start_polling_thread(messages, stop, 0)
        start = None
        try:
            while True:
                message = messages.get(timeout=30)
                if "fatal_error" in message:
                    raise RuntimeError(message["fatal_error"])
                if "replay_complete" in message:
                    break
                if "threads" not in message:
                    continue
                timestamp = message["recording_time"]
                if timestamp is None:
                    raise ValueError("Recording frame has no timestamp.")
                if start is None:
                    start = timestamp
                elapsed = timestamp - start
                if elapsed > duration:
                    break
                frame = serialize_frame(message)
                frame["elapsed_seconds"] = elapsed
                frames.append(frame)
        finally:
            stop.set()
            scraper.finish_polling_thread()
    if not frames:
        raise ValueError("Recording contained no usable frames.")
    return frames


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--recording", type=Path, required=True)
    parser.add_argument("--elf", type=Path, required=True)
    parser.add_argument("--duration", type=float, default=15)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if not math.isfinite(args.duration) or args.duration <= 0:
        parser.error("--duration must be finite and greater than zero")
    frames = export(args.recording, args.elf, args.duration)
    args.output.write_text(json.dumps(frames, allow_nan=False), encoding="utf-8")
    print(f"Exported {len(frames)} frames, 0–{frames[-1]['elapsed_seconds']:.3f}s")


if __name__ == "__main__":
    main()
