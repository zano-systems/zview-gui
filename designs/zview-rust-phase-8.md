# Phase 8: headless probe-rs

The optional `probe-rs` feature adds `-r probe-rs` to the standalone native
Rust `dump` and `record` commands. Building it or running the tests does not
look for probes. Default builds still accept only local GDB for live input.
There is no GUI transport, Python runner alias, service, automatic runner
selection, or change to browser or downstream CI.

```sh
cargo build --manifest-path crates/Cargo.toml --locked --features probe-rs --bin zview
crates/target/debug/zview dump --help
```

**Scope:** USB J-Link over SWD, an exact built-in single-core nRF52 chip
definition and core index `0` (the default). This limits the first path to
one reviewed driver and target sequence; it does not mean any combination
has passed on hardware. `--build-dir` resolves the ELF and looks for J-Link
`--device` in the selected build domain’s `zephyr/runners.yaml`. `--elf-file`
and `--chip` override those values. The runner must still be selected
explicitly. `--frequency` sets the SWD clock in Hz (a positive multiple of
1000), defaulting to 400 kHz. Normal attach uses the driver's default
clock; the requested clock is applied on active SWD before acquisition.
An unsupported request fails rather than silently falling back. The build’s
programming settings and vendor J-Link `--speed` are not used: a speed
accepted by the SEGGER library may be rejected by the `probe-rs` driver.

On the tested nRF52833 DK (J-Link OB-SAM3U128-V2-NordicSemi, firmware
July 8, 2025), both drivers reported a 10 MHz base frequency and a minimum
divider of 10 on active SWD, for a 1 MHz maximum. `probe-rs` rejected a
4 MHz request; the SEGGER library reported 1 MHz after that same request.
The clock was read back through the drivers, not measured on the SWCLK pin.

If `--probe` is omitted, the backend selects a J-Link only when exactly one
is visible and has a usable USB serial. With multiple J-Links, specify
`--probe VID:PID:SERIAL` (four hex digits per USB ID and the literal
nonempty serial). A missing or ambiguous probe is an error, not a fallback
to the first device. Enumeration does not attach to the target. No
user-facing reset, flash, erase, memory write, breakpoint
or debugger console.

With a dedicated, already-running board and an authorized debug session:

```sh
crates/target/debug/zview dump --runner probe-rs --build-dir "$build" --json
crates/target/debug/zview record --runner probe-rs --build-dir "$build" \
  --frames 3 --output "$capture"
```

Both runners use the same ELF resolution and 64-byte entry check, thread
discovery, stopped polling batches, logical `Memory` reads, decoders,
row selection and `/6` gzip operation recording. The probe source records
`runner: probe-rs`, `runner_target` (chip), `core`, `probe` (including serial),
requested `period` and configured `frequency` in Hz. Handle a recording as potentially
sensitive. The period is not a measured rate; the entry check is not image
attestation. Use only trusted ELF/target pairs.

## Lifecycle checked in probe-rs =0.32.0

These are source-level findings, **not observations of a device**:

- [`probe/jlink/mod.rs`](https://docs.rs/crate/probe-rs/0.32.0/source/src/probe/jlink/mod.rs)
  and [`probe/selector.rs`](https://docs.rs/crate/probe-rs/0.32.0/source/src/probe/selector.rs):
  the driver can pick one if multiple devices match; selectors can match
  serial aliases. ZView checks the filtered list for exactly one match and
  literal serial equality **before** `DebugProbeInfo::open`. The factory
  rechecks USB devices on open: exclusivity and stable USB topology between
  list and open are required. SWD avoids the driver's JTAG chain reset.
- [`Probe::attach_with_registry`](https://docs.rs/crate/probe-rs/0.32.0/source/src/probe.rs)
  passes `AttachMethod::Normal`; we pass a resolved target and default
  `Permissions` (no erase permission). In the
  [nRF52 unlock sequence](https://docs.rs/crate/probe-rs/0.32.0/source/src/vendor/nordicsemi/sequences/nrf52.rs)
  a locked chip requires erase permission **before** the reset/erase
  operations. No fallback on attach or permission failure.
- [`Session::new`, `clear_all_hw_breakpoints`, `halted_access`, `core` and `Drop`](https://docs.rs/crate/probe-rs/0.32.0/source/src/session.rs):
  even normal attach enables debug and clears hardware breakpoints;
  breakpoint cleanup can halt and resume cores. This is why multicore
  targets and shared debug sessions are excluded. `Session` owns the probe;
  `core(index)` lends a short-lived view. ZView reads the immutable entry
  code without requesting a halt, then checks core status before each batch.
  It marks running/sleeping cores for restoration **before** attempting
  halt, in case the halt succeeds but its reply fails. It never explicitly
  resumes a core found halted. An unknown/locked-up status is not treated as
  running. Normal end, error and cancellation paths attempt outstanding
  restoration; close reports errors, with a final best-effort drop fallback.
- Drop is not a fallible detach API: the library again clears breakpoints
  and calls target shutdown hooks, logging rather than returning failures.
  The default Cortex-M [`debug_core_stop`](https://docs.rs/crate/probe-rs/0.32.0/source/src/architecture/arm/sequences.rs)
  disables debug via DHCSR and DEMCR. ZView issues data reads, but
  attach/halt/resume/shutdown themselves change debug registers and execution.
  Neither pre-attach state nor post-drop state is guaranteed. In particular
  this is **not** a way to preserve someone else's halted breakpoint session.
- Logical reads are capped at 128 KiB, with deadline checks between at most
  1 KiB probe reads. Halt has a 500 ms library timeout. Cancellation and
  duration limits are cooperative; neither preempts a stuck USB/library
  call. Unplug, process kill, driver failure or attach failure before a
  session is returned can prevent restoration. A failed command may leave a
  partial recording, not a qualified capture.

## Word-read benchmark on nRF52833 DK

The probe transport uses aligned 32-bit blocks only for spans wholly within
RAM or nonvolatile memory in the target's memory map. Unaligned edges and
spans outside those regions keep byte access. It never reads past the
requested span or changes the logical operations stored in the recording.

With the same benchmark ELF, release builds, J-Link at 1 MHz, six-second
captures, and `--period 0`:

| Selection | Median interval, byte → word | Median stopped batch, byte → word |
| --- | ---: | ---: |
| Threads | 549 → 303 ms | 541 → 295 ms |
| All groups | 786 → 382 ms | 776 → 374 ms |

Frame intervals exclude startup. The stopped-batch figures exclude the
first frame and use the host's trace markers after halt and after resume.
They approximate target halt duration; they are not independent hardware
measurements. The all-groups word capture replayed to identical parsed JSON
in Rust and Python ZView 0.28.1 for the three frames checked.

### Fixed-read driver comparison

With the dedicated DK running the matching firmware and no other debugger or
ZView session connected, compare the drivers directly:

```sh
uv run --offline --no-python-downloads --no-project --python 3.12 --with 'zview==0.28.1' \
  python -I scripts/compare_zview_probe_reads.py "$capture" "$elf" \
  --probe "$selector" --output "$report"
```

This manual benchmark halts and resumes the core once per trial. It uses the
same ordered SRAM addresses and byte counts from the second captured frame
for both backends, at a configured 1 MHz. It reports read timings separately
from halt/resume and excludes decoding and recording compression. Its Python
side uses ZView's normal SEGGER memory-read methods, but explicitly halts
the core to match the Rust trials; this is not Python ZView's normal live
polling policy. The Rust hardware test is ignored in ordinary test runs.

On the benchmark DK, three trials per driver using 75 reads (10,042 bytes)
gave these median host timings in milliseconds:

| Operation | probe-rs | SEGGER |
| --- | ---: | ---: |
| All reads | 365.4 | 201.0 |
| `read32` (38 requests) | 190.2 | 132.7 |
| `read64` (18 requests) | 45.1 | 30.0 |
| `read8` (18 aligned five-byte requests) | 93.1 | 10.7 |
| `read_bytes` (one block) | 33.5 | 25.5 |

The per-operation numbers are independently computed medians. The
five-byte requests account for about half of the observed read-time gap.
The explicit halt and confirmation took 8.4 ms in probe-rs and 1,012 ms
via the SEGGER harness; resume took 12.3 ms and 2.2 ms, respectively.
Those are host call times, not an independent measurement of target halt
duration, and the forced SEGGER halt is not part of normal Python polling.

The `--five-byte-ab` option compares two probe-rs variants using the same
workload. Reading each aligned five-byte span in one byte-width call
instead of a word and a tail reduced the aggregate time for those 18 reads
from 94.2 to 56.6 ms. The full fixed workload went from 368.6 to 342.8 ms.
The transport now uses the single call for that shape; the logical request
is unchanged. In a separate six-second all-groups recording test at 1 MHz,
the median frame interval fell from 387.9 to 346.1 ms and the stopped-batch
proxy from 379.6 to 337.8 ms. Parsed JSON matched Python ZView 0.28.1 on
all three checked frames of the optimized recording. These are acquisition
measurements, not native UI delivery timings.

### Native replay delivery

A separate macOS debug-build experiment replayed the same 17-frame optimized
hardware capture and matching ELF in a hidden native GPUI window, three runs
per backend. The Python path used the checkout's owned loopback adapter with
ZView 0.28.1. Temporary instrumentation measured from decoded-frame creation
(before transport mapping) through application to the shared UI model. Each
run applied all 17 frames; excluding the first in each run gave 48 samples
per path:

| Path | Median | p95 | Maximum |
| --- | ---: | ---: | ---: |
| Python adapter / WebSocket | 6.5 ms | 8.3 ms | 9.3 ms |
| Direct Rust | 16.7 ms | 27.4 ms | 30.8 ms |

These measurements exclude acquisition, startup, and display refresh.
The direct path currently checks its worker queue on a 25 ms UI timer,
whereas WebSocket delivery is event-driven. Replacing the timer with a
wake-up on worker events is a candidate improvement, not a measured gain.
The temporary instrumentation was removed; no delivery behavior changed.

## Software checks without a probe

The feature-on tests use fabricated USB descriptors, the built-in target
registry and an in-memory core. They check ambiguous and missing selectors,
exact chip/core validation, read spans and deadlines, recorded source
metadata, initially halted versus running/sleeping cores, and cleanup when
a halt or resume reports failure. CLI tests use a missing ELF to check
argument handling without any possibility of attach. Shared acquisition,
cancellation, read-error recording and offline replay are exercised by the
existing loopback test stub, **not** a J-Link. The Phase 7 `--capture-pair`
workflow was exercised on the tracked benchmark pair: frames 1–3 matched
the pinned Python CLI. This is replay evidence only.

## Qualification checklist — do not execute without authorization

1. Obtain explicit approval for attach, temporary halts and breakpoint
   clearing on **one** dedicated board/probe with matching firmware already
   running. Confirm board revision, exact chip/core, literal USB selector,
   probe firmware, host/driver versions, ELF hash/build/domain and expected
   kernel objects. Verify an independent heartbeat/uptime or boot counter.
   No other debugger, duplicate serial, hotplug, critical workload or
   watchdog that could reset the board while halted. If these conditions
   cannot be confirmed, stop.
2. Only after approval, set `elf`, `chip`, `probe`, and a new `capture` path
   for that pair. From the repository root:

   ```sh
   zview=crates/target/debug/zview
   "$zview" dump -e "$elf" -r probe-rs --chip "$chip" --core 0 --probe "$probe" --frame 1 --json
   "$zview" record -e "$elf" -r probe-rs --chip "$chip" --core 0 --probe "$probe" \
     --frames 3 --period 0.1 -o "$capture"
   ```

   Check the dump's expected object identities, units and plausible values.
   Require exit 0, three complete recorded frames and a final `disconnect`.
   Save stderr, elapsed times and hashes of both files.
3. After **each** exit, independently check heartbeat progress, no reboot,
   and that a new authorized session can acquire the probe. Repeat after
   Ctrl-C during a longer bounded capture, and an approved non-destructive
   failure (for example an ELF with different entry bytes). Require a
   diagnostic and no successful partial dump. Stop on unexpected state,
   reset, timeout or uncertain cleanup; do not retry under reset.
4. Offline, compare the **same recording and matching ELF**:

   ```sh
   uv run --offline --no-python-downloads --no-project --python 3.12 --with 'zview==0.28.1' \
     python -I scripts/compare_zview_replay.py --capture-pair "$capture" "$elf"
   ```

   This dedicated mode checks frames 1–3 with the unchanged [Phase 7
   comparison rules](zview-rust-phase-7.md#inputs-and-reproduction). A
   difference or missing frame fails the command. Do not substitute a
   second live run or loosen numeric, missing-field or ordering checks.

The nRF52833 DK run establishes USB access, bounded recording, and replay
agreement on that setup. Independent heartbeat and recovery checks after
cancellation or a read fault remain open. GUI integration requires a
separate decision.
