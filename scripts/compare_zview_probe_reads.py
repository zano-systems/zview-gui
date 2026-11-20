"""Time one recorded SRAM read sequence on a dedicated nRF52833 DK at 1 MHz."""

import argparse
from collections import defaultdict
import gzip
import json
import os
from pathlib import Path
import statistics
import subprocess
import tempfile
import time

from elftools.elf.elffile import ELFFile
from pylink import JLinkInterfaces
from zview.backend.jlink import JLinkScraper

ROOT = Path(__file__).resolve().parents[1]
WIDTHS = {"read8": 1, "read32": 4, "read64": 8, "read_bytes": 1}


def requests_from(recording):
    batches = []
    batch = None
    with gzip.open(recording, "rt") as stream:
        for line in stream:
            row = json.loads(line)
            op = row.get("op")
            if op == "begin_batch":
                if batch is not None:
                    raise ValueError("nested batch in recording")
                batch = []
            elif op == "end_batch":
                if batch is None:
                    raise ValueError("end_batch without begin_batch")
                batches.append(batch)
                batch = None
                if len(batches) == 3:
                    break
            elif op in WIDTHS and batch is not None:
                batch.append((op, row["args"]["at"], row["args"]["amount"]))
    if len(batches) != 3:
        raise ValueError("expected discovery and at least two complete capture frames")
    requests = batches[2]  # discovery, first frame, then steady-state frame
    if not 1 <= len(requests) <= 128 or any(
        not (1 <= length <= 8192 and 0x20000000 <= addr <= 0x20020000 - length)
        for op, addr, count in requests
        for length in (WIDTHS[op] * count,)
    ):
        raise ValueError("expected a bounded nRF52833 SRAM workload")
    return requests


def entry_bytes(path):
    with path.open("rb") as stream:
        elf = ELFFile(stream)
        entry = elf.header.e_entry & ~1
        for segment in elf.iter_segments(type="PT_LOAD"):
            offset = entry - segment["p_vaddr"]
            if 0 <= offset <= segment["p_filesz"] - 64:
                return entry, segment.data()[offset : offset + 64]
    raise ValueError("ELF entry has no 64-byte file-backed read")


def segger_trial(serial, entry, expected, requests):
    scraper = JLinkScraper("nRF52833_xxAA", 1_000_000)
    probe = scraper.probe
    probe.open(serial_no=serial)
    restore = False
    try:
        probe.set_tif(JLinkInterfaces.SWD)
        probe.connect("nRF52833_xxAA", speed=1000)
        if probe.speed != 1000:
            raise RuntimeError(f"SEGGER reports {probe.speed} kHz, expected 1000")
        if bytes(probe.memory_read8(entry, len(expected))) != expected:
            raise RuntimeError("target entry does not match ELF")
        if probe.halted():
            raise RuntimeError("target is already halted; refusing to time it")

        methods = {op: getattr(scraper, op) for op in WIDTHS}
        samples = []
        start = time.perf_counter()
        restore = True
        probe.halt()
        if not probe.halted():
            raise RuntimeError("could not confirm halt")
        halt = time.perf_counter() - start
        start = time.perf_counter()
        for op, addr, count in requests:
            tick = time.perf_counter()
            if tick - start >= 2:
                raise TimeoutError("read batch exceeded two seconds")
            value = methods[op](addr, count)
            elapsed = time.perf_counter() - tick
            if len(value) != count:
                raise RuntimeError(f"short {op} read")
            samples.append((op, WIDTHS[op] * count, elapsed * 1000))
        reads = time.perf_counter() - start
        start = time.perf_counter()
        probe.restart()
        if probe.halted():
            raise RuntimeError("could not confirm resume")
        restore = False
        return {
            "halt_ms": halt * 1000,
            "reads_ms": reads * 1000,
            "resume_ms": (time.perf_counter() - start) * 1000,
            "requests": samples,
        }
    finally:
        try:
            if restore and probe.halted():
                probe.restart()
        finally:
            probe.close()


def probe_rs_trial(elf, request_path, selector, split_five_byte=False):
    env = os.environ | {
        "ZVIEW_TIMING_ELF": str(elf),
        "ZVIEW_TIMING_REQUESTS": str(request_path),
        "ZVIEW_TIMING_PROBE": selector,
    }
    env.pop("ZVIEW_TIMING_SPLIT_FIVE_BYTE", None)
    if split_five_byte:
        env["ZVIEW_TIMING_SPLIT_FIVE_BYTE"] = "1"
    result = subprocess.run(
        [
            "cargo", "test", "-q", "--manifest-path", "crates/Cargo.toml", "--locked",
            "--offline", "--release", "-p", "zview-core", "--features", "probe-rs",
            "hardware_read_timing", "--", "--ignored", "--nocapture",
        ],
        cwd=ROOT,
        env=env,
        text=True,
        stdout=subprocess.PIPE,
    )
    if result.returncode:
        raise RuntimeError(result.stdout)
    return next(
        json.loads(line.removeprefix("TIMING "))
        for line in result.stdout.splitlines()
        if line.startswith("TIMING ")
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("recording", type=Path)
    parser.add_argument("elf", type=Path)
    parser.add_argument(
        "--probe", required=True, metavar="VID:PID:SERIAL",
        help="dedicated J-Link with no other debugger attached; the test temporarily halts the core",
    )
    parser.add_argument("--output", required=True, type=Path, help="new JSON report")
    parser.add_argument(
        "--five-byte-ab",
        action="store_true",
        help="compare only probe-rs, splitting or keeping aligned five-byte reads",
    )
    args = parser.parse_args()
    serial = int(args.probe.rsplit(":", 1)[1])
    requests = requests_from(args.recording)
    elf = args.elf.resolve()
    entry, expected = entry_bytes(elf)
    trials = defaultdict(list)
    with args.output.open("x") as output, tempfile.TemporaryDirectory() as tmp:
        request_path = Path(tmp) / "requests.json"
        request_path.write_text(json.dumps(requests), encoding="utf-8")
        drivers = ("probe-rs", "probe-rs-split-five-byte" if args.five_byte_ab else "segger")
        for order in (drivers, drivers[::-1], drivers):
            for driver in order:
                result = (
                    probe_rs_trial(elf, request_path, args.probe, driver == "probe-rs-split-five-byte")
                    if driver != "segger"
                    else segger_trial(serial, entry, expected, requests)
                )
                trials[driver].append(result)
                print(driver, {k: round(v, 2) for k, v in result.items() if k != "requests"}, flush=True)
                time.sleep(0.4)
        json.dump({"requests": requests, "trials": trials}, output, indent=2)
        output.write("\n")
    for driver, samples in trials.items():
        by_kind = defaultdict(list)
        for sample in samples:
            totals = defaultdict(float)
            for op, _, ms in sample["requests"]:
                totals[op] += ms
            for op, ms in totals.items():
                by_kind[op].append(ms)
        print(driver, "median milliseconds by operation:", {
            op: round(statistics.median(ms), 2) for op, ms in by_kind.items()
        })


if __name__ == "__main__":
    main()
