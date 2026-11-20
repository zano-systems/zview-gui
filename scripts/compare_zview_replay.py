"""Opt-in Phase 7 comparison with the official Python ZView CLI."""

import argparse
from decimal import Decimal
import gzip
from importlib.metadata import version
import json
import os
from pathlib import Path
import shlex
import subprocess
import sys
from tempfile import TemporaryDirectory


ROOT = Path(__file__).resolve().parents[1]
BENCH = Path("test-data/zview-bench-60s")
LEGACY = Path("test-data/tests/fixtures")
BENCH_INPUT = BENCH / "capture.ndjson.gz"
LEGACY_INPUT = LEGACY / "recordings/sys_heap_v4.3.ndjson.gz"
REFERENCE = "0.28.1"


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate JSON key {key!r}")
        result[key] = value
    return result


def invalid_number(token):
    raise ValueError(f"non-JSON number {token}")


def snapshot(stdout):
    result = json.loads(
        stdout, object_pairs_hook=unique_object, parse_float=Decimal,
        parse_constant=invalid_number,
    )
    if not isinstance(result, dict):
        raise ValueError("expected a snapshot object")
    return result


def differences(rust, python, path="$"):
    if type(rust) is not type(python):
        yield f"{path}: Rust {rust!r} != Python {python!r} (type)"
    elif isinstance(rust, dict):
        for key in sorted(rust.keys() | python.keys()):
            child = f"{path}[{key!r}]"
            if key not in rust or key not in python:
                yield f"{child}: missing from {'Rust' if key not in rust else 'Python'}"
            else:
                yield from differences(rust[key], python[key], child)
    elif isinstance(rust, list):
        if len(rust) != len(python):
            yield f"{path}: Rust length {len(rust)} != Python length {len(python)}"
        for index, (left, right) in enumerate(zip(rust, python)):
            yield from differences(left, right, f"{path}[{index}]")
    elif rust != python:
        yield f"{path}: Rust {rust!r} != Python {python!r}"


def compare(label, rust, env, recording, elf, options, *, exit_code=0, diagnostic=None):
    print(f"\n{label}", flush=True)
    args = ["dump", "-i", str(recording), "-e", str(elf), "--period", "0.03", "--json", *options]
    snapshots = {}
    passed = True
    for name, command in [
        ("Rust", [str(rust)]),
        ("Python", [sys.executable, "-I", "-m", "zview.cli"]),
    ]:
        command += args
        print(f"  {name}: {shlex.join(command)}", flush=True)
        try:
            result = subprocess.run(
                command, cwd=ROOT, env=env, capture_output=True, text=True, timeout=90,
            )
        except (OSError, subprocess.TimeoutExpired) as error:
            print(f"    {error}")
            passed = False
            continue
        print(f"    exit {result.returncode}; stdout {'present' if result.stdout else 'empty'}")
        if result.stderr:
            print(f"    stderr:\n      " + "\n      ".join(result.stderr.rstrip().splitlines()))
        if result.returncode != exit_code:
            print(f"    expected exit {exit_code}")
            passed = False
        if exit_code:
            if result.stdout or not result.stderr.strip():
                print("    expected no stdout and a diagnostic")
                passed = False
            if diagnostic and diagnostic not in result.stderr:
                print(f"    expected diagnostic containing {diagnostic!r}")
                passed = False
        elif result.returncode == 0:
            try:
                snapshots[name] = snapshot(result.stdout)
            except ValueError as error:
                print(f"    invalid snapshot: {error}")
                passed = False
    if len(snapshots) == 2:
        mismatches = list(differences(snapshots["Rust"], snapshots["Python"]))
        for mismatch in mismatches[:12]:
            print(f"    {mismatch}")
        if mismatches:
            print(f"    {len(mismatches)} JSON differences (first 12 shown)")
            passed = False
        else:
            print("    identical parsed JSON")
    print(f"  {'PASS' if passed else 'FAIL'}", flush=True)
    return passed


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rust", type=Path, default=ROOT / "crates/target/debug/zview")
    inputs = parser.add_mutually_exclusive_group()
    inputs.add_argument(
        "--qemu-capture", type=Path,
        help="also replay frames 1–3 of a Rust recording from the benchmark ELF on dedicated QEMU",
    )
    inputs.add_argument(
        "--capture-pair", nargs=2, type=Path, metavar=("RECORDING", "MATCHING_ELF"),
        help="compare only frames 1–3 of this local pair; never connects to the source",
    )
    args = parser.parse_args()
    if version("zview") != REFERENCE:
        parser.error(f"run with the official zview=={REFERENCE} distribution")
    rust = args.rust.resolve()
    if not rust.is_file():
        parser.error(f"build the experimental CLI first: no binary at {rust}")

    with TemporaryDirectory(prefix="zview-comparison-") as scratch:
        # Python ZView writes ELF caches and logs under the home directory.
        env = dict(os.environ, HOME=scratch, USERPROFILE=scratch, LOCALAPPDATA=scratch)
        if args.capture_pair:
            recording, elf = (path.resolve() for path in args.capture_pair)
            results = [
                compare(f"Capture pair frame {frame}", rust, env, recording, elf,
                        ["--frame", str(frame)]) for frame in (1, 2, 3)
            ]
            print(f"\nCapture pair: {sum(results)}/3 snapshot pairs match")
            return 0 if all(results) else 1

        truncated = Path(scratch) / "missing-final-end-batch.ndjson.gz"
        with gzip.open(ROOT / LEGACY_INPUT, "rb") as source:
            lines = source.readlines()
        if [json.loads(line)["op"] for line in lines[-2:]] != ["end_batch", "disconnect"]:
            raise ValueError("legacy fixture no longer ends with end_batch, disconnect")
        # Keep gzip valid; remove the last frame's end marker and disconnect.
        truncated.write_bytes(gzip.compress(b"".join(lines[:-2]), mtime=0))

        replay = []
        for label, recording, elf, frames in [
            ("benchmark", BENCH_INPUT, BENCH / "zephyr.elf", (1, 289, 578)),
            ("legacy", LEGACY_INPUT, LEGACY / "zephyr.elf", (1, 10, 20)),
        ]:
            replay.extend((f"{label} frame {frame}", recording, elf, ["--frame", str(frame)])
                          for frame in frames)
        replay += [
            ("benchmark default frame, scoped includes", BENCH_INPUT, BENCH / "zephyr.elf",
             ["--match", "threads:idle,heaps:bench_heap"]),
            ("benchmark frame 2, include/override/exclude", BENCH_INPUT, BENCH / "zephyr.elf",
             ["--frame", "2", "--match", "*,threads:*waiter*,threads:!gate_*,heaps:!*"]),
            ("legacy frame 10, wildcard", LEGACY_INPUT, LEGACY / "zephyr.elf",
             ["--frame", "10", "--match", "threads:stress_?d,heaps:my_kernel_heap"]),
            ("legacy frame 10, absent row", LEGACY_INPUT, LEGACY / "zephyr.elf",
             ["--frame", "10", "--match", "threads:no_such_thread,heaps:my_kernel_heap"]),
        ]
        good = [compare(label, rust, env, recording, elf, options)
                for label, recording, elf, options in replay]
        bad = [
            compare("legacy invalid frame", rust, env, LEGACY_INPUT, LEGACY / "zephyr.elf",
                    ["--frame", "0"], exit_code=2, diagnostic="--frame"),
            compare("legacy clean EOF", rust, env, LEGACY_INPUT, LEGACY / "zephyr.elf",
                    ["--frame", "21"], exit_code=1, diagnostic="only 20 frames"),
            compare("legacy truncated before end_batch", rust, env, truncated, LEGACY / "zephyr.elf",
                    ["--frame", "20"], exit_code=1, diagnostic="zview:"),
        ]
        print(f"\nTracked replay: {sum(good)}/{len(good)} snapshot pairs match")
        print(f"Failures: {sum(bad)}/{len(bad)} cases rejected as expected by both")
        qemu = []
        if args.qemu_capture:
            qemu = [
                compare(f"QEMU cross-replay frame {frame}", rust, env,
                        args.qemu_capture.resolve(), BENCH / "zephyr.elf", ["--frame", str(frame)])
                for frame in (1, 2, 3)
            ]
            print(f"\nQEMU cross-replay: {sum(qemu)}/{len(qemu)} snapshot pairs match")
        return 0 if all(good + bad + qemu) else 1


if __name__ == "__main__":
    sys.exit(main())
