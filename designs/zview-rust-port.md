# Experimental Rust ZView port

**Phase 8: optional headless probe-rs transport, not hardware-qualified.**
See the [software boundary and manual qualification checklist](zview-rust-phase-8.md).
The native app can explicitly opt into
in-process Rust acquisition for Replay and trusted loopback GDB Live. The
experimental Rust command can dump a
recorded frame, dump a live frame, or record bounded live reads from a
trusted loopback GDB stub or (with a separate Cargo feature) the reviewed
USB probe path. Live and replay use the same ELF/DWARF discovery
and kernel decoders. This experiment does not change the default native Python
path, browser UI, or downstream CI. It does not propose retiring Python, adding a
West command, packaging or publishing this workspace, or changing release pins.

## Build boundary

[`crates/Cargo.toml`](../crates/Cargo.toml) is a standalone Cargo workspace:
`zview-core` is a library other Rust code can depend on by path, and `zview-cli`
provides the `zview` binary. The root Cargo package stays separate so even
dependency resolution for the headless build does not involve GPUI. The root
package links `zview-core` by path only for native targets, using the root
lockfile; the headless workspace retains its own lockfile. Rust replay needs
no Python runtime or external ELF tools. The CLI owns arguments, output and exit status.
In `zview-core/src/`, `threads.rs`, `objects.rs` and `heap.rs` request target
operations via `memory.rs`. `replay.rs` implements those operations without
exposing trace entries to the decoders. `firmware.rs`, `dwarf.rs` and
`inventory.rs` resolve layouts, instances, symbols and source locations.

From the repository root (for offline use, first cache the locked dependencies
with `cargo fetch --manifest-path crates/Cargo.toml --locked`):

```sh
cargo fmt --manifest-path crates/Cargo.toml --all --check
cargo test --manifest-path crates/Cargo.toml --workspace --locked --offline
cargo clippy --manifest-path crates/Cargo.toml --workspace --all-targets --locked --offline -- -D warnings
cargo run --manifest-path crates/Cargo.toml --locked --offline --bin zview -- --help
```

## Phase 3: dump a recorded frame

```sh
bench=test-data/zview-bench-60s
cargo run --manifest-path crates/Cargo.toml --locked --bin zview -- dump \
  --input "$bench/capture.ndjson.gz" --elf-file "$bench/zephyr.elf" --frame 1 --json
```

Exit 0 returns the selected frame as one JSON object and newline, or text
lines in group and row order without `--json`. Frame numbers start at 1;
omitting `--frame` selects frame 2. `--period 0` disables the minimum time
between polls (default 0.1 seconds). Supply the matching ELF with `-e` or use
`-d BUILD [--domain NAME]` to resolve it. `--match 'threads:idle,heaps:*'`
limits output, **not** the recorded requests or the CPU share denominator.
There is no prefix output or exit 3. Argument errors exit 2; runtime failures
exit 1 with a diagnostic on stderr instead of a snapshot.

After discovery, the library polls threads, heaps, semaphores, mutexes, message
queues, memory slabs and work queues in that order. The recorded features,
selection, DWARF and preceding read results determine which requests it makes.
For example, the first benchmark poll reads the thread entries once; later polls
reuse them and scan only the part of each stack not already known to be used.
No frame is counted until its `end_batch` matches. A legacy recording may lack
metadata or heap-map reads: probing their presence does not consume a different
operation. Malformed results and ordinary mismatches are errors, not zeros.

**Tested inputs and limits**

- On the [benchmark `/4` capture](../test-data/zview-bench-60s/README.md#sha-256),
  asking for frame 579 decodes all 578 frames, consumes the final `disconnect`,
  checks the gzip end, then reports out of range. Truncation or mismatch is a
  different error. A successful earlier dump does not validate the unread suffix.
- The [second `/2` pair](../test-data/tests/fixtures/README.md) has 20 frames,
  three threads, and one heap. Metadata, waiters and heap maps not recorded
  there are null, not measured zeros or empty arrays.
- The focused acceptance check used
  `uv tool run --python 3.12 --from 'zview==0.28.1' zview dump -i … -e …`.
  Complete parsed JSON matched for benchmark frames 1, 2 and 578, and frame 2
  of the second capture. Filtered default-frame text also matched. For Python
  frame 578, use a nonzero period (checked at `0.02` seconds); at zero its
  bounded event queue can drop frames. Rust decodes synchronously without drops.
- These checks exercise little-endian ARM ELF32, uncompressed unit-local DWARF,
  monitored thread lists and statically declared kernel objects. Static-only
  thread discovery, other architectures and arbitrary vendor layouts are not
  supported here. Strict read matching is not firmware authentication.
  NDJSON lines are bounded at 1 MiB; monitored discovery at 64 threads.
- The header reader recognizes `/2`, `/3`, `/4`, `/6`, not `/5`. A synthetic
  selected `/6` stream tests stamped usage and unfilled stacks on the benchmark
  ELF. There is no tracked real `/3` or `/6` capture; recognizing a
  header does not establish compatibility with every feature/layout combination.

## Phase 4: live dump and bounded recording

Use a dedicated stub you control, running the matching firmware, with no
other debugger attached. For the tracked benchmark see its [QEMU launch
command](../test-data/zview-bench-60s/README.md#capture-and-export).
The Rust CLI does not launch QEMU or select a device.

```sh
cargo build --manifest-path crates/Cargo.toml --locked --bin zview
zview=crates/target/debug/zview
elf=test-data/zview-bench-60s/zephyr.elf
target=127.0.0.1:12349
"$zview" dump -e "$elf" -r gdb -t "$target" --json
"$zview" record -e "$elf" -r gdb -t "$target" --match 'heaps:bench_heap' \
  --period 0.1 --frames 3 -o new-capture.ndjson.gz
"$zview" dump -i new-capture.ndjson.gz -e "$elf" --frame 3 --json
```

`-d BUILD [--domain NAME]` resolves the ELF as for replay. The runner and
target must be explicit. Only numeric loopback endpoints (and `localhost`
mapped to IPv4 loopback) are accepted; there is no DNS lookup or probe
frequency. The live selection shapes reads. Thread discovery still supplies
waiter names, and measured CPU shares use all thread cycle deltas. Every
selected heap gets its map. Data is sampled, not a complete event trace.

Use exactly one of `--frames N` (positive count of complete frames) and
`--duration SECONDS` (nonnegative; starts after discovery). A poll in
progress can finish after the duration. The period is a floor between poll
starts, not a deadline or an achieved sample rate. A `/6` gzip trace contains
source (including normalized endpoint and requested period), features,
selection and timed logical memory operations, not RSP packets. Output must
be a **new** file. A failed command may leave a partial capture. Dump emits
only the requested frame; record reports the count only on successful cleanup.

**Protocol and lifecycle limits**

- Little-endian ARM ELF32 with the monitored-thread layouts described above.
  A file-backed 64-byte span at the Thumb-normalized entry point must compare
  equal *before* using live pointers. This rejects some mismatches; it does
  not authenticate a whole image. The ELF, firmware and stub must be trusted.
- A single all-stop, ACK-mode session: stop reason (`?`), interrupt, bounded
  `m` reads, continue (`c`), detach (`D`). No arbitrary packets, register or
  memory writes, monitor commands, breakpoints, flash, reset, automatic
  reconnect, non-stop/multiprocess support, or general GDB console.
- Reads are split into at most 256 bytes per packet, with checksum and size
  checks, short-reply continuation, and bounded retransmission on NAK/invalid
  checksum. Logical reads over 128 KiB fail rather than emitting lines the
  replay reader cannot consume. I/O has a two-second timeout; new packet
  reads cannot start after the five-second batch budget.
- Discovery and each poll read while stopped; the target runs between polls.
  Cancellation (CLI Ctrl-C, also SIGTERM on Unix) finishes an in-flight batch
  and interrupts the inter-poll wait. Cleanup stops a pending continue,
  checks the detach reply and finalizes the recording. A rejected detach
  attempts a final continue but is still an error. A lost connection, failed
  stub, forced process kill or system crash cannot guarantee resumption.

**Validation.** The scoped tests include a deterministic ACK-mode loopback
server backed by benchmark discovery and heap bytes. Tests exercise checksums,
short replies, mismatch, malformed and rejected reads, timeout, detach
failure, cancellation while stopped, and selected record → offline dump.
On local QEMU **11.1.3**, `mps2-an385` / Cortex-M3 with the tracked benchmark
ELF, live dump returned all seven groups (18 threads). A three-frame Rust
capture replayed with identical complete parsed JSON in Rust and Python
ZView **0.28.1** for frames 1, 2 and 3. A duration-bounded heap capture and
Ctrl-C stop were also exercised. Read-only QMP `query-status` reported
`running: true` after dump, both successful records, and the interrupted
record. This is local emulation, **not hardware validation**.

## Phase 5: opt-in native GUI

Use `cargo run --locked --bin zview-gui-rs -- --rust-core`, then configure
Replay with a recording and matching ELF, or Live with an ELF and a trusted
loopback GDB address. The native app owns a single session; no HTTP/WebSocket
server, Python process or CLI subprocess is started. It uses the shared
session controls, protocol-v5 events, model and renderer. Native inputs are
user-selected local file paths, not adapter-served paths; this mode does not
apply the adapter's `--data-root` policy. Use only trusted files and firmware.

Replay uses [`ReplaySession`](../crates/zview-core/src/lib.rs), the same incremental
strict decoder as CLI dump. The GUI paces decoding at up to 10 frames/s and
retains the last 256 frames, not original capture timing. Live uses
[`zview_core::live::run`](../crates/zview-core/src/live.rs), with a 100 ms polling
floor and a fixed startup inventory. The rate shown is derived from successive
complete samples, not the requested period. The Phase 4 architecture, layout,
trust, protocol and cleanup restrictions also apply to the GUI.

The [native bridge](../src/native_core.rs) owns a dedicated worker and a bounded
event queue. A Live frame enters the queue only after the target resumes; queue
backpressure can slow sampling. Stop discards undelivered data from that worker
and `Stopping` remains active until the worker joins. A failed read or detach
is an error, not a successful stop. Reopen and explicit Live Reconnect allocate a
new session ID. Graceful Quit also cancels and joins; forced termination cannot
guarantee cleanup or target resumption.

There is no dynamic live view demand, heap capture, inventory refresh,
adjustable CPU averaging, recording output, build/runner detection, OS file
picker, or non-GDB runner in this mode. Charts and detail views consume the
fixed samples without pretending to request more target data. Unsupported live
heap-map readings and unknown readings (for example, unsupported wait queues or
absent runtime counters) remain unavailable. Replay can show already recorded
heap chunks. The existing Python and WASM paths retain their feature set.

To exercise Live acquisition through the native session code without opening a
desktop window, start a private benchmark QEMU GDB stub as described above, then
run this simulated-GPUI test with its loopback address:

```sh
ZVIEW_GUI_TEST_GDB=127.0.0.1:12349 cargo test --locked --lib \
  rust_live_benchmark_gdb_smoke -- --ignored
```

It checks acquisition, Disconnect, and explicit Reconnect against the real
emulated target, not visual interaction or physical hardware.

## Phase 6: live runner readiness

**`jlink`, `pyocd` and `nrfutil` are each unverified and deferred in Rust.**
At Phase 6 Rust Live accepted only explicit loopback GDB. The benchmark is a QEMU
GDB capture; neither tracked recording header identifies a physical runner.
No board/probe workflow was authorized for this phase. Replay, mock adapters
and a vendor GDB server are not tests of the direct probe backends.

| Runner | Python ZView 0.28.1 implementation | What a Rust candidate must establish |
| --- | --- | --- |
| `jlink` | `backend/jlink.py` keeps a `pylink-square` handle to SEGGER's native J-Link library. It opens without a serial selector, selects SWD and connects by MCU **device name**, not probe ID. Speed is `auto` or the requested Hz divided by 1000 (integer kHz). Disconnect closes the handle. | A compatible host library and binding for reads and close; an isolated probe so selection is unambiguous. No J-Link CLI or GDB server is used by the reference. |
| `pyocd` | `backend/pyocd.py` opens a persistent pyOCD session in `attach` mode. The target override is an MCU type, not a probe ID; `ConnectHelper` selects the probe. Default clock: 4 MHz. On a queued-read fault it drains pending callbacks and retries individual reads. | A supported probe driver and target definition (pack if needed), noninteractive selection and ordered reads that release pending transfers on failure. A Rust transport or Python bridge would need testing; the reference does not launch the `pyocd` CLI. |
| `nrfutil` | `backend/nrfutil.py` starts `nrfutil --json device read` with `--direct` for each read (20 s default timeout). Connect reads four bytes at address zero; disconnect only clears a flag. Selection is by probe serial, else `--traits jlink` requires a single J-Link. No clock option. | A compatible `nrfutil` **and** `device` subcommand, readable address zero, validation of JSON byte results and bounded child cleanup on error/timeout. Measure time per complete poll rather than assuming a process per read meets the requested period. |

**Next candidate: J-Link**, if a dedicated board becomes available. The
[adapter's build-selection test](../web-ui/test_build_context.py) models an
nRF52833 build with `nrfutil` for flash but `jlink` for debug reads. That is
selection evidence, not a hardware run. Local file/package inspection found
SEGGER's 9.90 library, `pylink-square` 1.7.0 and pyOCD 0.44.1 in a Python
environment, and nRF Util manifests for 8.0.0 and `device` 2.12.9. None of
these drivers or commands was exercised.

**Qualification gate, per runner.** Reserve one authorized board/probe pair
with matching firmware already running and a known observable heartbeat.
Retain board/core, probe model and firmware, host driver/tool versions,
target selector and clock, matching ELF hash and build/domain metadata. On
that pair, without flash or reset:

1. Dump a live frame and check known firmware objects and readings. Record a
   bounded number of complete frames to a new file; replay that same capture
   with the matching ELF in Rust and Python 0.28.1 and compare parsed frames.
2. After disconnect, verify the heartbeat advances and a fresh session can
   acquire the probe. Repeat with an approved, controlled read failure:
   require a diagnostic, no successful partial frame, bounded cleanup, and a
   successful new dump after the fault clears. Retain the capture and logs.

These remain gates for claiming direct runner support. Phase 8 adds a
separately selected experimental `probe-rs` transport, not an implementation
of the Python `jlink`, `pyocd` or `nrfutil` contracts and not a fallback.

The sections below describe the **Python reference and compatibility target**,
including commands not implemented by the Rust experiment.

## Reference: Python ZView 0.28.1

Based on the [PyPI 0.28.1 wheel](https://pypi.org/project/zview/0.28.1/#files)
(`zview-0.28.1-py3-none-any.whl`, SHA-256
`9f40ba7391fa97f87f93938a73fc51bb9d4ae85b3df9422f9eb4aa9a09f6cbec`).
See `zview/cli.py` and `snapshot.py` for process behavior,
`backend/base.py`, `kinds/` and `kernel/` for snapshot fields,
`backend/recording.py` and `backend/replay.py` for the read stream, and
`runner_config.py` and `target.py` for build resolution. West behavior comes
from the tagged [West command](https://github.com/wkhadgar/zview/blob/v0.28.1/scripts/west_commands/zview_tui_cmd.py).
The repo's browser protocol and its legacy benchmark JSON export are separate
contracts, not substitutes for `zview dump --json`.

For each headless workflow as it is implemented, preserve its invocation,
data meanings and success/failure behavior. Compare parsed JSON and decoded
recordings, not JSON whitespace, gzip bytes, timestamps from separate captures,
or literal diagnostic text.

### Headless commands

The Python entry points are `zview` (or `python -m zview.cli`) and, in a
Zephyr workspace with the ZView module installed, `west zview`.

```sh
# Read a recording with its matching firmware; no hardware needed.
zview dump -i capture.ndjson.gz -e zephyr.elf --frame 1 --json
# Sample a live GDB target already running the matching firmware.
zview dump -e zephyr.elf -r gdb -t 127.0.0.1:1234 --json
# Capture live reads to a new file.
zview record -e zephyr.elf -r gdb -t 127.0.0.1:1234 \
  --period 0.1 --duration 60 -o capture.ndjson.gz
```

| Flags | Python 0.28.1 behavior |
| --- | --- |
| `-e, --elf-file`; `-d, --build-dir`; `--domain` | An explicit ELF, or a Zephyr build from which to fill missing inputs. `--domain` selects the sysbuild image. Replay needs the matching ELF, not just an ELF for the same board. |
| `-r, --runner`; `-t, --runner-target`; `--frequency` | Live probe/source options; see build discovery below. A live `dump` requires both runner and target after resolution. |
| `--period SECONDS` | Minimum time between polling starts (default `0.10`, `0` allowed). This is not a sample-rate guarantee. Replay `dump` disables recorded-time pacing but still uses this polling floor. |
| `--match LINE` | Default `*`. Comma-separated case-sensitive globs, optionally scoped as `GROUP:GLOB`; exclude with `!GLOB` or `GROUP:!GLOB`. Group-specific includes replace unscoped includes for that group; excludes win. For example, `--match '*, heaps:!*'`. This shapes live reads; replay uses the captured selection for reads and applies a further filter to output, not to the read sequence. |
| `dump -i, --input FILE` | Replay source, mutually exclusive with runner/target; no probe frequency on replay. Without `-i`, dump reads live. |
| `dump --frame N` | **1-based** count of valid polling frames delivered to the command, not an NDJSON line, batch marker, or timestamp. The CLI defaults to **2** (the first frame that can calculate cycle shares from a prior poll); the Python helper itself defaults to 1. Dropped or broken frames do not count. |
| `dump --json` | Write one JSON object and a newline. Without it, write one human-readable line per returned row. `--usage-window` is not a `dump` or `record` flag. |
| `record -o, --output FILE`; `--duration SECONDS` or `--frames N` | Output path and exactly one bound are required. Duration bounds the polling loop, not startup; shutdown can wait for an in-flight poll. A frame bound must be at least 1 and counts data frames, not read entries. Recording captures maps for every selected heap. |

Successful commands exit 0. Dump stdout contains only the selected frame
(JSON or text); successful record prints `Recorded N frames to PATH` on
stdout, not the recording itself. Probe/ELF status and selection or read
notices can appear on stderr even on success. Do not mix stderr into data
pipelines. Standalone CLI argument errors exit 2 with usage and a diagnostic;
command failures exit 1 with a stderr diagnostic and a traceback in ZView's
log, not a success payload. West uses its own fatal diagnostics for discovery
and dispatch failures. Missing or unreadable input, unmatched firmware,
fatal reads, an out-of-range replay frame, and timeout are failure cases.
Unmatched row patterns warn but need not fail. Dump's default wait limit is
`max(5.0, N * max(period, 0.1) * 5.0)` seconds after starting polling, not a
wall-clock deadline for ELF loading or shutdown. A clean EOF before N reports
how many frames were received; at high rates, the reference's bounded event
queue may drop frames. Record writes directly to the output (not atomically),
so a failed command can leave a partial file.

### Snapshot JSON and units

The top-level object maps sampled group names to arrays of rows. It has no
frame number, timestamp, version envelope, or separate `groups` wrapper.
Unselected or unavailable groups are omitted, not filled with empty arrays.
A returned group can be empty after row filtering. Rows have `name` and
numeric `address`; addresses are target addresses, not ordinals. Every row
has a human-readable `line` and, where available, a `glance` object
(`fill` in percent, `text`, `level` of `quiet`, `busy`, or `blocked`).
Use structured fields for assertions, not presentation text. Source locations
(`entry_site`, `decl_site`) are `[path, line]` from the ELF or `null`.

| Group | Fields beyond the common row fields |
| --- | --- |
| `threads` | `stack_start` is an address; `stack_size` and `runtime.stack_watermark` are bytes (the latter estimates peak stack use); `runtime.stack_watermark_percent` is used stack as a percentage of size. `runtime.cpu_normalized` is the CPU percentage (share of measured thread cycle deltas including idle); `runtime.cpu` is load percentage (share of non-idle cycles, zero for idle). They are not fractions or time in seconds. Without a baseline, frame 1 seeds counts and shares are zero when usage is available; without runtime stats the shares are `null`. `runtime.is_active` means cycles advanced in that poll, not necessarily that the thread is currently scheduled; `runtime.is_cpu_coherent` is `null` when coherence is unknown. `priority` is numeric, `state` and `user_options` are bit fields, `entry_point` is an address, `entry_symbol` and `entry_site` identify the entry, and `is_idle` identifies an idle thread. |
| `heaps` | `free_bytes`, `allocated_bytes`, `max_allocated_bytes` (peak), `usage_percent` (`100 * allocated / (allocated + free)`, zero for a zero total), `chunks`, `waiters`, `decl_site`. A captured chunk is `{used, size}` with `size` in bytes including allocator overhead; chunk totals need not match payload statistics. `chunks: null` means no map, not an empty heap. |
| `semaphores` | `count` and `limit` are token counts (limit 0 is uninitialized; `4294967295` is the unbounded sentinel), plus `waiters`, `decl_site`. |
| `mutexes` | `lock_count` is nesting depth; `owner_address` is zero for no owner; `owner_name` can be `null`, plus `waiters`, `decl_site`. |
| `msgqs` | `used_msgs` and `max_msgs` are message counts, `msg_size` is bytes per message, plus `waiters`, `decl_site`. |
| `mem_slabs` | `num_blocks` and `num_used` count blocks; `block_size` is bytes; `max_used` is a peak block count or `null` if not traced, plus `waiters`, `decl_site`. |
| `workqs` | `flags` is a bit mask (started, busy, draining, plugged); `pending` lists queued item labels, not the currently executing item. `is_pending_truncated` marks an incomplete list; `thread_name`, `waiters` (drain waiters), `decl_site`. Empty pending does not imply idle. |

`waiters: null` means unavailable (for example, a queue layout not walked);
`waiters: []` means walked, with nobody waiting. Preserve unavailable values
and distinguish them from measured zeros. Do not synthesize unobserved
activity or a full execution trace from sampled values.

### Recording and strict replay

The `.ndjson.gz` is gzip-compressed UTF-8 NDJSON of **memory operations**,
not a stream of previously decoded snapshots. The writer emits
`schema: "zview-recording/6"`; the 0.28.1 reader accepts `/6`, `/4`, `/3`, and
`/2` (not `/5`). The header has `schema`, `zview`, `endianess` (wire spelling;
`"<"` for little endian), `created_at` (epoch seconds), `features`,
`selection` (`groups`, `skipped`, `include` and `exclude` scope/glob pairs),
and `source` (`runner`, `runner_target`, `frequency` in Hz if specified and
`period` in seconds). Legacy headers lacking features use threads and heaps;
those lacking selection default to all rows.

Subsequent entries have `t` (monotonic seconds, useful for relative pacing),
`op` and `args`, with `result` or `error` when applicable. `connect`,
`disconnect`, `begin_batch` and `end_batch` have empty arguments. Reads
(`read8`, `read32`, `read64`, `read_bytes`) take `at` (target address) and
`amount` (element count; bytes for `read_bytes`); results are numeric arrays
or a hex string for raw bytes. `read_many` expands to individual ordered
reads. A failed read records `error: {kind, message}` for replay to raise.
Discovery and other reads precede the first data frame; counting batch
markers is not enough to find the requested snapshot. The matching ELF
supplies symbols, DWARF field layouts and locations. Neither is embedded in
the recording.

Replay requires the next operation **and** arguments to match before
advancing, including when addresses are read repeatedly. An unmatched read
does not advance the cursor. Beginning another batch at the trailing
`disconnect` signals clean EOF, unlike a truncated stream. For legacy
recordings that lack chunk-map reads, the Python heap decoder can stop
requesting that optional map on mismatch without consuming the next entry;
this is not permission to search or reorder the read stream.
The Python frame orchestrator can suppress an `end_batch` error after all
reads succeed. See the [Phase 7 truncation check](zview-rust-phase-7.md#tracked-recordings)
for the observed difference from Rust.

### Zephyr build discovery and runners

- The standalone Python CLI fills missing arguments only if `-d` is supplied.
  West uses Zephyr's `run_common.get_build_dir` via `ZEPHYR_BASE` when inputs
  are missing and no `-d` was given (its help lists `build.dir-fmt`, the
  current directory, and `./build`). Fully explicit arguments need no West
  discovery. Bare `west zview` defaults to the TUI; bare standalone `zview`
  requires a command, though unprefixed live flags imply `live`.
- `domains.yaml` selects the requested `--domain` or the declared default.
  Paths are rebased when a sysbuild directory is moved. Without this file
  only `app` exists. For the chosen image, use explicit `-e`, then
  `zephyr/runners.yaml`'s `config.elf_file` (relative to the YAML file if not
  absolute), otherwise `zephyr/zephyr.elf`. A missing named ELF is an error,
  not a fallback. Replay does not resolve a runner and does not require
  `runners.yaml` if the ELF can be found without it.
- For live runner discovery, explicit flags win. ZView 0.28.1 uses
  **`flash-runner`** or `jlink` if absent, not the browser adapter's
  `debug-runner` preference. Unsupported discovered runners warn and fall
  back to PyOCD. The YAML supplies J-Link `--device` or PyOCD `--target`
  when present. A fully specified live runner and target bypass runner and
  clock discovery.
- `gdb` takes `host:port`; `jlink` takes a device name; `pyocd` takes a
  target override; `nrfutil` takes a probe serial. When a build is supplied,
  `record` can leave the target unset for PyOCD (automatic selection) or
  nrfutil (single J-Link), but not for GDB or J-Link; live `dump` still
  requires both `-r` and `-t` resolved. `--frequency` is for `pyocd` and
  `jlink` only, in Hz (also accepts suffixes such as `10M`). PyOCD defaults
  to 4 MHz; J-Link defaults to its own speed. YAML J-Link `--speed` is kHz
  if numeric (`auto`/`adaptive` leave it to the probe); PyOCD reads YAML
  `--frequency`.
- Live expects matching firmware already running. The reference checks
  a small entry-point span when readable; this is not a full image
  attestation. Connection does not flash firmware. GDB can halt and resume
  the target for reads.

### Representative CI assertions (Python reference)

These examples use the tracked [benchmark capture and matching
ELF](../test-data/zview-bench-60s/README.md). With Python ZView 0.28.1 and `jq`
installed, run from the repository root, writing the JSON to a scratch
directory of your choice (`$scratch`). Check the dump command before testing
its output:

```sh
bench=test-data/zview-bench-60s
zview dump -i "$bench/capture.ndjson.gz" -e "$bench/zephyr.elf" --frame 1 --json \
  >"$scratch/frame.json" || exit $?
# Pass (exit 0): the observed allocation in the first frame is 528 bytes.
jq -e 'any(.heaps[]; .name == "bench_heap" and .allocated_bytes == 528)' "$scratch/frame.json"
# Fail (exit 1): an example budget of 500 bytes is exceeded.
jq -e 'any(.heaps[]; .name == "bench_heap" and .allocated_bytes <= 500)' "$scratch/frame.json"
```

Both assertion outcomes were checked against the wheel. They are reference
examples, not Rust acquisition tests, evidence of real adopters, or changes
to downstream CI.

## Ordered follow-up phases

1. **Scaffold and contract** (implemented): independent headless Cargo build
   and truthful CLI boundary only.
2. **Real first-frame replay slice** (superseded): the initial thread prefix
   established strict request matching and the memory/decoder boundary.
3. **Full headless replay/dump** (implemented within the tested scope above):
   complete recorded batches, selection, runtime and source metadata, text/JSON,
   replay build resolution, frame selection, exhaustion and failure handling.
4. **Local QEMU GDB/live record** (implemented within the boundary above):
   sample matching firmware and replay bounded captures against the reference.
5. **Opt-in native GUI** (implemented within the boundary above): native
   Replay and trusted loopback GDB use `zview-core` when explicitly selected.
6. **Additional live runners** (readiness assessed; qualification deferred):
   see the [per-runner hardware checks](#phase-6-live-runner-readiness).
7. **Comparison report** (completed for the tested headless paths): see the
   [Phase 7 results and reproduction commands](zview-rust-phase-7.md).
   No migration or retirement commitment.
8. **Headless probe-rs** (software only): [scope, lifecycle review and
   outstanding hardware checks](zview-rust-phase-8.md). No GUI probe support.

The Python `live` and `replay` commands render a curses TUI. The Rust
experiment excludes that interface; reusable interactive live/replay session
behavior belongs in the library later, not in a replacement TUI or a CLI
subprocess driven by the GUI.
