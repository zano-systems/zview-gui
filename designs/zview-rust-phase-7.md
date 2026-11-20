# Phase 7: Rust headless comparison with Python ZView 0.28.1

On the two tracked recording/ELF pairs, **10/10 selected snapshot pairs
matched** and **2/3 negative cases were rejected as expected by both CLIs**.
The exception was a recording cut just before its final `end_batch`: Rust
rejected frame 20; Python returned it successfully. A separate three-frame
Rust capture on dedicated local QEMU matched **3/3** when replayed by both
CLIs. This is evidence for the tested headless paths, not a migration or
retirement recommendation.

## Inputs and reproduction

Run from the repository root. The run used macOS arm64, Rust/Cargo 1.99.0
(debug build), Python 3.12.16, uv 0.13.7 and the cached official `zview==0.28.1`
distribution (verified via `importlib.metadata` and invoked as
`python -I -m zview.cli`). No adapter, exporter, GUI session JSON or sibling
checkout supplied reference data. Cargo and uv were offline; prerequisites
must already be cached for these commands.

```sh
shasum -a 256 \
  test-data/zview-bench-60s/capture.ndjson.gz test-data/zview-bench-60s/zephyr.elf \
  test-data/tests/fixtures/recordings/sys_heap_v4.3.ndjson.gz test-data/tests/fixtures/zephyr.elf
cargo build --manifest-path crates/Cargo.toml --locked --offline --bin zview
uv run --offline --no-python-downloads --no-project --python 3.12 --with 'zview==0.28.1' \
  python -I scripts/compare_zview_replay.py
```

SHA-256 in the command's order:

```text
4d1f3e96ea7a526ff4492bef9c18cb826e5d4d6705c5a7150689496ef42c7b7e  benchmark capture (/4)
85932fc875cfa391635f8fa92b706ef82a08ec7824b473264ee73923aa9ed610  benchmark ELF
f67c6a09252c97dac50cda12b7b28d1bd5d683d9f56d90792d1adc0aac3a4bcf  legacy capture (/2)
570a09c439dfa82f86f8d36d0ccfded993dde83975c6ad33b8e98a52f53275f1  legacy ELF
```

The [comparison script](../scripts/compare_zview_replay.py) uses only the
standard library and prints the exact argv, exit status, stdout presence and
stderr for every case. Both executables receive
`dump -i RECORDING -e MATCHING_ELF --period 0.03 --json` plus the options
below. A nonzero polling floor reduces Python's replay event-queue drops
without replaying the original capture intervals. A preliminary invocation
with `--period 0.05` exceeded the enclosing terminal's 100-second budget
before finishing the matrix; it is not included in the counts. Python's
cache and logs are redirected to the script's temporary home directory.

For successful pairs, comparison ignores only JSON whitespace, object-key
order and spelling of equivalent decimal values. It preserves list order,
JSON types (including integer versus decimal), omitted keys, `null`, zero and
empty collections, with no tolerance, path stripping, or row reordering.
Names, target addresses, source locations, readings, `line` and `glance`
are all compared, including their nested lists. Negative cases compare exit
status, absence of a stdout payload, and the presence of a relevant
diagnostic, not its exact wording. The script exits 1 for any mismatch,
including the known truncation difference; it is not a green CI check.

For a separate three-frame comparison of a new recording, use
`--capture-pair RECORDING MATCHING_ELF`. This mode uses the same comparison
function, without running the tracked negative cases or opening a live
source. See the [Phase 8 qualification checklist](zview-rust-phase-8.md#qualification-checklist--do-not-execute-without-authorization).

## Tracked recordings

| Pair | Frame(s) and options | Matched |
| --- | --- | ---: |
| [Benchmark](../test-data/zview-bench-60s/README.md) | `--frame 1`, `--frame 289`, `--frame 578`; unfiltered | 3/3 |
| Benchmark | Omit `--frame`; `--match 'threads:idle,heaps:bench_heap'` | 1/1 |
| Benchmark | `--frame 2 --match '*,threads:*waiter*,threads:!gate_*,heaps:!*'` | 1/1 |
| [Legacy sys_heap](../test-data/tests/fixtures/README.md) | `--frame 1`, `--frame 10`, `--frame 20`; unfiltered | 3/3 |
| Legacy sys_heap | `--frame 10 --match 'threads:stress_?d,heaps:my_kernel_heap'` | 1/1 |
| Legacy sys_heap | `--frame 10 --match 'threads:no_such_thread,heaps:my_kernel_heap'` | 1/1 |

- **Identity and units:** Each unfiltered benchmark result had 18 threads,
  one heap, two semaphores, one mutex, one message queue, one slab and two
  work queues. Frame 1 identifies `bench_heap` at `0x20001898`: 528 bytes
  allocated, 1,336 free, peak 1,524 and `usage_percent=28.32618025751073`.
  Frames 289 and 578 show 720 and 1,188 allocated bytes respectively. The
  three chunk lists contain 46, 119 and 128 entries; chunk sizes are in
  bytes including allocator overhead, not payload totals. Frame 1 also
  matches semaphore token counts, mutex depth/owner/waiter identities,
  message counts and 4-byte message size, slab counts and 64-byte block
  size, and work-queue flags and pending labels.
- **CPU and filters:** On benchmark frame 1, `idle` (`0x20000e10`) has
  `cpu_normalized=0.0` without a baseline. The default filtered dump returns
  `idle` and `bench_heap` from frame 2: `idle` retains the same
  `cpu_normalized=99.97647109152746` (percent of measured cycles) as the
  unfiltered Rust frame 2, with `cpu=0.0` as the idle thread and a 48-byte
  stack watermark out of 256 bytes. The mixed filter omits `heaps` but keeps
  the other object groups, with just `lock_waiter_id` and `heap_waiter_id` in
  `threads`. The absent-row filter returns `threads: []`, keeps the selected
  heap, and warns on stderr in both CLIs.
- **Missing is not zero:** Legacy has only `threads` and `heaps` (three
  threads and one heap unfiltered). Frame 20 has `stress_id` at `0x200000b8`
  with a 744-byte watermark out of 1,024 bytes, but null thread metadata
  such as `priority` and `entry_site`, and null `is_cpu_coherent`. Its
  `my_kernel_heap` at `0x200008a8` has 1,056 allocated bytes and null
  `chunks` and `waiters`. This differs from an observed empty wait queue
  (`bench_heap.waiters: []` at benchmark frame 1) or an observed nonempty
  one (`["heap_waiter_id"]` there at frame 578).

| Negative case (legacy pair) | Rust | Python 0.28.1 | Matched expected rejection |
| --- | --- | --- | --- |
| `--frame 0` | Exit 2, no JSON, invalid argument | Exit 2, no JSON, invalid argument | Yes |
| `--frame 21` on intact input | Exit 1, no JSON, only 20 frames | Exit 1, no JSON, only 20 frames | Yes |
| `--frame 20` with final `end_batch` and `disconnect` removed | Exit 1, no JSON, unexpected end requesting `end_batch` | Exit 0, frame JSON, no truncation diagnostic | **No** |

The truncation case recompresses the unchanged NDJSON prefix as valid gzip in
scratch space. It leaves all of frame 20's read results intact. A separate
check with `python -I -m zview.cli dump -e test-data/tests/fixtures/zephyr.elf
--frame 20 --period 0.03 --json -i RECORDING` confirmed that Python returned
identical parsed snapshots on the intact and truncated inputs. In the
installed reference, `backend/replay.py` raises on the missing
operation, but `orchestrator.py:read_frame` suppresses exceptions from
`end_batch()`. This is a batch-boundary discrepancy, not evidence that Python
silently accepts every form of truncation or fails to match read operations.
Neither implementation was changed. Python also prints ELF load/cache
information on stderr and points to its log on runtime failure; Rust does not
emit those status lines.

## Dedicated local QEMU cross-replay

Re-ran the Phase 4 path with QEMU 11.1.3, the tracked benchmark ELF and a new
private stub bound to `127.0.0.1`. The run checked that port 12357 was free,
started the VM below, waited one second and checked that the process was alive.
Then Rust recorded three complete frames with no row filter:

```sh
bench=test-data/zview-bench-60s
rust=crates/target/debug/zview
work=$(mktemp -d)
port=12357
python3 - "$port" <<'PY'
import socket, sys
with socket.socket() as sock:
    sock.bind(("127.0.0.1", int(sys.argv[1])))
PY
qemu-system-arm -machine mps2-an385 -cpu cortex-m3 -kernel "$bench/zephyr.elf" \
  -display none -monitor none -serial "file:$work/serial.log" \
  -gdb "tcp:127.0.0.1:$port" -qmp "unix:$work/qmp.sock,server=on,wait=off" \
  >"$work/qemu.log" 2>&1 &
qemu_pid=$!
trap 'kill "$qemu_pid" 2>/dev/null || true; wait "$qemu_pid" 2>/dev/null || true' EXIT
sleep 1
if ! kill -0 "$qemu_pid"; then cat "$work/qemu.log"; exit 1; fi
"$rust" record -e "$bench/zephyr.elf" -r gdb -t "127.0.0.1:$port" \
  --period 0.1 --frames 3 -o "$work/cross-replay.ndjson.gz"
python3 - "$work/qmp.sock" <<'PY'
import json, socket, sys
with socket.socket(socket.AF_UNIX) as sock:
    sock.settimeout(5)
    sock.connect(sys.argv[1])
    with sock.makefile("rwb") as stream:
        assert "QMP" in json.loads(stream.readline())
        for command in ("qmp_capabilities", "query-status"):
            stream.write(json.dumps({"execute": command}).encode() + b"\n")
            stream.flush()
            while True:
                reply = json.loads(stream.readline())
                if "event" not in reply:
                    break
            print(command, reply)
            assert "return" in reply
        assert reply["return"]["running"] is True
PY
kill "$qemu_pid"
wait "$qemu_pid" || true
trap - EXIT
```

`record` exited 0 and reported three frames. On the private Unix QMP socket,
`{"execute":"qmp_capabilities"}` then `{"execute":"query-status"}` returned
`{"status":"running","running":true}` after the CLI detached. The run
terminated and waited for only the VM it started (`kill "$qemu_pid";
wait "$qemu_pid" || true`), then executed the following full comparison.
This was the invocation used for the reported matrix above; the optional
argument only adds the three cross-replay pairs.

```sh
uv run --offline --no-python-downloads --no-project --python 3.12 --with 'zview==0.28.1' \
  python -I scripts/compare_zview_replay.py --qemu-capture "$work/cross-replay.ndjson.gz"
```

**3/3 full parsed frames matched.** This compares replay of the *same* Rust
`/6` capture; Python did not connect to the live VM or make a separate
recording. No flash, reset, physical probe or external service was used.

## Other checks and limits

```sh
cargo fmt --manifest-path crates/Cargo.toml --all --check
cargo test --manifest-path crates/Cargo.toml --workspace --locked --offline
cargo clippy --manifest-path crates/Cargo.toml --workspace --all-targets --locked --offline -- -D warnings
```

Formatting, the 24 headless Rust tests (13 CLI, 6 live, 5 core), and Clippy
passed. The comparison does not check every frame or filter, bad CRCs,
mismatched ELFs, other real recording variants or firmware layouts, live
sampling parity, `-d`/West resolution, plain-text output, the TUI, either GUI,
the Python adapter protocol, packaging, or downstream CI. No fixture replay
timing or live acquisition timing was measured. Direct `jlink`, `pyocd` and
`nrfutil` remain [unqualified and deferred](zview-rust-port.md#phase-6-live-runner-readiness).
No production dependencies, defaults or CI workflows were changed.
