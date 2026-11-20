//! Optional native transport. In particular, do not use Session::auto_attach
//! or Session::halted_access (the latter operates on *all* cores).
use super::Transport;
use anyhow::{Context, Result, bail, ensure};
use probe_rs::{
    CoreStatus, MemoryInterface, Permissions, Session, Target,
    config::Registry,
    probe::{DebugProbeInfo, DebugProbeSelector, ProbeFactory, WireProtocol, jlink::JLinkFactory},
};
use std::{
    fmt,
    ops::Range,
    str::FromStr,
    time::{Duration, Instant},
};

#[derive(Clone, Debug)]
pub struct ProbeSelector(DebugProbeSelector);

impl FromStr for ProbeSelector {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        let mut parts = s.splitn(3, ':');
        for id in [parts.next(), parts.next()] {
            ensure!(
                id.is_some_and(|s| s.len() == 4 && s.bytes().all(|c| c.is_ascii_hexdigit())),
                "expected four-digit hex IDs in VID:PID:SERIAL"
            );
        }
        ensure!(
            parts
                .next()
                .is_some_and(|s| !s.trim().is_empty() && !s.contains(char::is_control)),
            "probe requires a nonempty serial in VID:PID:SERIAL"
        );
        Ok(Self(s.parse()?))
    }
}

impl fmt::Display for ProbeSelector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl ProbeSelector {
    fn unique<'a>(&self, listed: &'a [DebugProbeInfo]) -> Result<&'a DebugProbeInfo> {
        let mut matches = listed.iter().filter(|info| self.0.matches_probe(info));
        let info = matches
            .next()
            .with_context(|| format!("no probe matches {self}"))?;
        ensure!(matches.next().is_none(), "ambiguous probe selector {self}");
        // probe-rs also accepts hex-encoded aliases; insist on the actual serial.
        ensure!(
            info.serial_number == self.0.serial_number,
            "expected the literal probe serial"
        );
        Ok(info)
    }
}

fn resolve(registry: &Registry, chip: &str, core: usize) -> Result<Target> {
    let target = registry.get_target_by_name(chip)?;
    ensure!(
        target.name.eq_ignore_ascii_case(chip),
        "specify the exact chip name, not a prefix"
    );
    ensure!(
        target.cores.len() == 1 && core == 0,
        "requires one single-core target and --core 0"
    );
    // Normal attach calls vendor-specific setup/unlock sequences. Do not opt
    // other families into this path without reviewing those sequences.
    ensure!(
        target.name.starts_with("nRF52") && target.cores[core].core_type.is_cortex_m(),
        "only the nRF52 attach sequence has been reviewed"
    );
    Ok(target)
}

fn select_single(listed: &[DebugProbeInfo]) -> Result<ProbeSelector> {
    let info = match listed {
        [] => bail!("no supported USB J-Link found"),
        [info] => info,
        _ => bail!("multiple USB J-Links found; specify --probe VID:PID:SERIAL"),
    };
    let serial = info
        .serial_number
        .as_deref()
        .context("the detected USB J-Link has no serial; cannot select it safely")?;
    format!("{:04x}:{:04x}:{serial}", info.vendor_id, info.product_id).parse()
}

pub(super) fn discover(chip: &str, core: usize) -> Result<ProbeSelector> {
    // Reject unsupported targets before enumerating USB. Do not open a probe
    // during discovery; open() rechecks the exact selector before attaching.
    resolve(&Registry::from_builtin_families(), chip, core)?;
    let listed = JLinkFactory
        .list_probes()
        .into_iter()
        .map(|p| p.info)
        .collect::<Vec<_>>();
    select_single(&listed)
}

pub(super) fn frequency_khz(hz: u32) -> Result<u32> {
    let khz = hz / 1000;
    ensure!(
        (1..0xffff).contains(&khz) && hz.is_multiple_of(1000),
        "probe-rs --frequency must be a positive whole number of kHz expressed in Hz (1000–65534000)"
    );
    Ok(khz)
}

pub(super) fn open(
    chip: &str,
    core: usize,
    probe: ProbeSelector,
    frequency_hz: u32,
) -> Result<Box<dyn Transport>> {
    Ok(Box::new(open_jlink(chip, core, probe, frequency_hz)?))
}

fn open_jlink(
    chip: &str,
    core: usize,
    probe: ProbeSelector,
    frequency_hz: u32,
) -> Result<Batches<SelectedCore>> {
    let khz = frequency_khz(frequency_hz)?;
    let registry = Registry::from_builtin_families();
    let target = resolve(&registry, chip, core)?;
    let word_regions = target
        .memory_map
        .iter()
        .filter(|region| region.is_ram() || region.is_nvm())
        .map(|region| region.address_range())
        .collect();
    // Use only the reviewed local USB driver, not the all-driver lister.
    let listed = JLinkFactory
        .list_probes_filtered(Some(&probe.0))
        .into_iter()
        .map(|p| p.info)
        .collect::<Vec<_>>();
    let mut handle = probe
        .unique(&listed)?
        .open()
        .with_context(|| format!("open probe {probe}"))?;
    handle.select_protocol(WireProtocol::Swd)?;
    let mut session = handle
        .attach_with_registry(target, Permissions::default(), &registry)
        .with_context(|| format!("normal attach to {chip} on {probe}"))?;
    // select_protocol only stages the protocol. J-Link switches its physical
    // interface on attach, and its advertised speed limit is interface-specific.
    // Attach at the driver's default; set the acquisition clock on active SWD
    // before ZView reads firmware or starts polling.
    let dap = session
        .get_arm_interface()?
        .try_dap_probe_mut()
        .context("J-Link session did not expose its ARM debug probe")?;
    let selected = dap
        .set_speed(khz)
        .with_context(|| format!("set active SWD clock to {khz} kHz on {probe}"))?;
    ensure!(
        selected == khz,
        "probe selected {selected} kHz instead of requested {khz} kHz"
    );
    Ok(Batches {
        core: SelectedCore {
            session,
            index: core,
        },
        resume_due: false,
        word_regions,
    })
}

// Keep Session (the owner), not Core (a short-lived view). This small
// interface also lets tests exercise failure *after* a halt took effect.
trait Control {
    fn status(&mut self) -> Result<CoreStatus>;
    fn halt(&mut self) -> Result<()>;
    fn run(&mut self) -> Result<()>;
    fn read_bytes(&mut self, addr: u32, data: &mut [u8]) -> Result<()>;
    fn read_words(&mut self, addr: u32, data: &mut [u32]) -> Result<()>;
}
struct SelectedCore {
    session: Session,
    index: usize,
}
impl Control for SelectedCore {
    fn status(&mut self) -> Result<CoreStatus> {
        Ok(self.session.core(self.index)?.status()?)
    }
    fn halt(&mut self) -> Result<()> {
        self.session
            .core(self.index)?
            .halt(Duration::from_millis(500))?;
        Ok(())
    }
    fn run(&mut self) -> Result<()> {
        Ok(self.session.core(self.index)?.run()?)
    }
    fn read_bytes(&mut self, addr: u32, data: &mut [u8]) -> Result<()> {
        Ok(self.session.core(self.index)?.read_8(addr.into(), data)?)
    }
    fn read_words(&mut self, addr: u32, data: &mut [u32]) -> Result<()> {
        Ok(self.session.core(self.index)?.read_32(addr.into(), data)?)
    }
}

struct Batches<C: Control> {
    core: C,
    resume_due: bool,
    word_regions: Vec<Range<u64>>,
}

const TRANSFER_BYTES: usize = 1024;

fn is_aligned_five_byte_read(addr: u32, len: u32) -> bool {
    len == 5 && addr.is_multiple_of(4)
}

fn read_chunk<T>(addr: u32, deadline: Instant, read: impl FnOnce() -> Result<T>) -> Result<T> {
    ensure!(Instant::now() < deadline, "probe batch timed out");
    read().map_err(|e| crate::ReadFault(format!("probe read at {addr:#x}: {e:#}")).into())
}

impl<C: Control> Transport for Batches<C> {
    fn halt(&mut self) -> Result<()> {
        match self.core.status()? {
            CoreStatus::Running | CoreStatus::Sleeping => {
                self.resume_due = true; // halt can fail after stopping the core
                self.core.halt().context("halt selected core")
            }
            CoreStatus::Halted(_) => Ok(()), // not our halt
            state => bail!("refusing to halt core in state {state:?}"),
        }
    }
    fn resume(&mut self) -> Result<()> {
        if self.resume_due {
            match self
                .core
                .status()
                .context("check selected core before resume")?
            {
                CoreStatus::Halted(_) => self.core.run().context("resume selected core")?,
                CoreStatus::Running | CoreStatus::Sleeping => {}
                state => bail!("cannot confirm restoration in core state {state:?}"),
            }
            self.resume_due = false;
        }
        Ok(())
    }
    fn memory(&mut self, at: u32, len: u32, deadline: Instant) -> Result<Vec<u8>> {
        // Caller has checked length and address overflow. Check the batch
        // deadline between calls; the library has no cancellable USB read.
        let mut data = vec![0; len as usize];
        let start = u64::from(at);
        let normal_memory = self
            .word_regions
            .iter()
            .any(|r| start >= r.start && start + u64::from(len) <= r.end);
        let prefix = ((4 - (at as usize % 4)) % 4).min(data.len());
        let aligned_len = (data.len() - prefix) / 4 * 4;
        // Never widen a read, and keep the old access width outside known
        // RAM/NVM, where changing width could alter a device access.
        // For the common five-byte metadata span, one byte-width transfer
        // is faster than a separate word transfer and byte tail.
        if normal_memory && aligned_len != 0 && !is_aligned_five_byte_read(at, len) {
            let (head, rest) = data.split_at_mut(prefix);
            let (body, tail) = rest.split_at_mut(aligned_len);
            if !head.is_empty() {
                read_chunk(at, deadline, || self.core.read_bytes(at, head))?;
            }
            let mut words = [0; TRANSFER_BYTES / 4];
            for (i, chunk) in body.chunks_mut(TRANSFER_BYTES).enumerate() {
                let addr = at + (prefix + i * TRANSFER_BYTES) as u32;
                let words = &mut words[..chunk.len() / 4];
                read_chunk(addr, deadline, || self.core.read_words(addr, words))?;
                for (bytes, word) in chunk.as_chunks_mut::<4>().0.iter_mut().zip(words) {
                    *bytes = word.to_le_bytes();
                }
            }
            if !tail.is_empty() {
                let addr = at + (prefix + aligned_len) as u32;
                read_chunk(addr, deadline, || self.core.read_bytes(addr, tail))?;
            }
        } else {
            for (i, chunk) in data.chunks_mut(TRANSFER_BYTES).enumerate() {
                let addr = at + (i * TRANSFER_BYTES) as u32;
                read_chunk(addr, deadline, || self.core.read_bytes(addr, chunk))?;
            }
        }
        Ok(data)
    }
    fn close(&mut self) -> Result<()> {
        self.resume()
    }
}
impl<C: Control> Drop for Batches<C> {
    fn drop(&mut self) {
        // Fallback on unwind or a transient failure during explicit close.
        let _ = self.resume();
        // Session::drop then performs its own breakpoint/debug shutdown.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use probe_rs::HaltReason;

    #[test]
    #[ignore = "manual timing on a dedicated nRF52833 DK; temporarily halts the core"]
    fn hardware_read_timing() -> Result<()> {
        use std::{env, fs};

        let elf = fs::read(env::var("ZVIEW_TIMING_ELF")?)?;
        let (_, _, entry, expected) = crate::firmware::Firmware::live(&elf)?;
        let requests: Vec<(String, u32, u32)> =
            serde_json::from_slice(&fs::read(env::var("ZVIEW_TIMING_REQUESTS")?)?)?;
        ensure!(!requests.is_empty() && requests.len() <= 128);
        let requests = requests
            .iter()
            .map(|(op, at, amount)| {
                let width = match op.as_str() {
                    "read8" | "read_bytes" => 1,
                    "read32" => 4,
                    "read64" => 8,
                    _ => bail!("unexpected read operation {op}"),
                };
                let len = amount.checked_mul(width).context("read size overflow")?;
                // Accept only small reads in this board's SRAM.
                ensure!(
                    *at >= 0x2000_0000
                        && (1..=8192).contains(&len)
                        && at.checked_add(len).is_some_and(|end| end <= 0x2002_0000)
                );
                Ok((op, *at, len))
            })
            .collect::<Result<Vec<_>>>()?;
        let selector = env::var("ZVIEW_TIMING_PROBE")?.parse()?;
        let split_five_byte = env::var_os("ZVIEW_TIMING_SPLIT_FIVE_BYTE").is_some();
        let mut probe = open_jlink("nRF52833_xxAA", 0, selector, 1_000_000)?;
        let result = (|| {
            ensure!(
                probe.memory(
                    entry,
                    expected.len().try_into()?,
                    Instant::now() + Duration::from_secs(2)
                )? == expected,
                "target entry does not match ELF"
            );
            let mut timings = Vec::with_capacity(requests.len());
            let start = Instant::now();
            probe.halt()?;
            let halt = start.elapsed();
            let start = Instant::now();
            for (op, addr, len) in requests {
                let now = Instant::now();
                let deadline = start + Duration::from_secs(2);
                let data = if split_five_byte && is_aligned_five_byte_read(addr, len) {
                    let mut word = [0];
                    let mut bytes = [0; 5];
                    read_chunk(addr, deadline, || probe.core.read_words(addr, &mut word))?;
                    bytes[..4].copy_from_slice(&word[0].to_le_bytes());
                    read_chunk(addr + 4, deadline, || {
                        probe.core.read_bytes(addr + 4, &mut bytes[4..])
                    })?;
                    bytes.to_vec()
                } else {
                    probe.memory(addr, len, deadline)?
                };
                std::hint::black_box(data);
                timings.push((op, len, now.elapsed().as_secs_f64() * 1000.0));
            }
            let reads = start.elapsed();
            let start = Instant::now();
            probe.resume()?;
            println!(
                "TIMING {}",
                serde_json::json!({
                    "halt_ms": halt.as_secs_f64() * 1000.0,
                    "reads_ms": reads.as_secs_f64() * 1000.0,
                    "resume_ms": start.elapsed().as_secs_f64() * 1000.0,
                    "requests": timings,
                })
            );
            Ok(())
        })();
        let cleanup = probe.close();
        cleanup.context("restore target after hardware timing")?;
        result
    }

    fn info(serial: Option<&str>) -> DebugProbeInfo {
        DebugProbeInfo::new(
            "mock",
            0x1234,
            0x5678,
            serial.map(String::from),
            &JLinkFactory,
            None,
            false,
        )
    }

    #[test]
    fn only_one_literal_serial_is_accepted() {
        for s in [
            "",
            "auto",
            "1234:5678",
            "1234:5678:",
            "1234:5678: ",
            "xx:5678:A",
        ] {
            assert!(s.parse::<ProbeSelector>().is_err(), "{s}");
        }
        let probe: ProbeSelector = "1234:5678:A".parse().unwrap();
        assert!(probe.unique(&[info(None), info(Some("a"))]).is_err());
        assert!(probe.unique(&[info(Some("A")), info(Some("A"))]).is_err());
        let probes = [info(Some("B")), info(Some("A"))];
        assert_eq!(probe.unique(&probes).unwrap(), &probes[1]);
    }

    #[test]
    fn automatic_selection_never_guesses_between_devices() {
        assert!(
            select_single(&[])
                .unwrap_err()
                .to_string()
                .contains("no supported USB J-Link")
        );
        assert!(select_single(&[info(None)]).is_err());
        assert_eq!(
            select_single(&[info(Some("0001"))]).unwrap().to_string(),
            "1234:5678:0001"
        );
        for probes in [
            [info(Some("A")), info(Some("B"))],
            [info(Some("A")), info(Some("A"))],
            [info(Some("A")), info(None)],
        ] {
            assert!(
                select_single(&probes)
                    .unwrap_err()
                    .to_string()
                    .contains("specify --probe")
            );
        }
    }

    #[test]
    fn validate_chip_and_core_without_a_probe() {
        let registry = Registry::from_builtin_families();
        let chip = "nRF52833_xxAA";
        assert_eq!(resolve(&registry, chip, 0).unwrap().name, chip);
        assert!(resolve(&registry, "nRF52833", 0).is_err());
        assert!(resolve(&registry, chip, 1).is_err());
        assert!(resolve(&registry, "auto", 0).is_err());
    }

    #[test]
    fn swd_speed_conversion_requires_exact_khz() {
        assert_eq!(frequency_khz(400_000).unwrap(), 400);
        assert_eq!(frequency_khz(4_000_000).unwrap(), 4000);
        for bad in [0, 999, 4_000_001, 65_535_000, u32::MAX] {
            assert!(frequency_khz(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn trace_source_identifies_the_requested_connection() {
        let target = crate::live::Target::ProbeRs {
            chip: "nRF52833_xxAA".into(),
            core: 0,
            probe: Some(select_single(&[info(Some("mock"))]).unwrap()),
            frequency_hz: 4_000_000,
        };
        assert_eq!(
            target.source(Duration::from_millis(100)),
            serde_json::json!({
                "runner":"probe-rs","runner_target":"nRF52833_xxAA","core":0,
                "probe":"1234:5678:mock","frequency":4000000,"period":0.1
            })
        );
    }

    struct Mock {
        state: CoreStatus,
        runs: usize,
        halts: usize,
        halt_error: bool,
        run_error: bool,
        reads: Vec<(u32, usize, usize)>,
        fail_at: Option<u32>,
    }
    const HALTED: CoreStatus = CoreStatus::Halted(HaltReason::Request);
    fn byte_at(addr: u32) -> u8 {
        addr as u8
    }

    impl Control for Mock {
        fn status(&mut self) -> Result<CoreStatus> {
            Ok(self.state)
        }
        fn halt(&mut self) -> Result<()> {
            self.halts += 1;
            self.state = HALTED;
            ensure!(!self.halt_error, "halt reply lost");
            Ok(())
        }
        fn run(&mut self) -> Result<()> {
            self.runs += 1;
            ensure!(!self.run_error, "resume failed");
            self.state = CoreStatus::Running;
            Ok(())
        }
        fn read_bytes(&mut self, addr: u32, data: &mut [u8]) -> Result<()> {
            self.reads.push((addr, data.len(), 1));
            ensure!(self.fail_at != Some(addr), "simulated byte read fault");
            for (i, byte) in data.iter_mut().enumerate() {
                *byte = byte_at(addr + i as u32);
            }
            Ok(())
        }
        fn read_words(&mut self, addr: u32, data: &mut [u32]) -> Result<()> {
            self.reads.push((addr, data.len() * 4, 4));
            ensure!(self.fail_at != Some(addr), "simulated word read fault");
            for (i, word) in data.iter_mut().enumerate() {
                let start = addr + i as u32 * 4;
                *word = u32::from_le_bytes([0, 1, 2, 3].map(|offset| byte_at(start + offset)));
            }
            Ok(())
        }
    }
    fn batches(state: CoreStatus) -> Batches<Mock> {
        Batches {
            core: Mock {
                state,
                runs: 0,
                halts: 0,
                halt_error: false,
                run_error: false,
                reads: vec![],
                fail_at: None,
            },
            resume_due: false,
            word_regions: std::iter::once(0..2048).collect(),
        }
    }

    #[test]
    fn batches_restore_only_their_own_halts() {
        for initial in [HALTED, CoreStatus::Running, CoreStatus::Sleeping] {
            let mut batch = batches(initial);
            batch.halt().unwrap();
            assert_eq!(batch.core.state, HALTED);
            batch.resume().unwrap();
            batch.close().unwrap();
            let count = usize::from(initial != HALTED);
            assert_eq!((batch.core.halts, batch.core.runs), (count, count));
        }
        let mut batch = batches(CoreStatus::LockedUp);
        assert!(batch.halt().is_err());
        batch.close().unwrap();
        assert_eq!((batch.core.halts, batch.core.runs), (0, 0));
    }

    #[test]
    fn uncertain_halt_and_failed_resume_keep_cleanup_armed() {
        let mut batch = batches(CoreStatus::Running);
        batch.core.halt_error = true;
        assert!(batch.halt().is_err());
        batch.core.run_error = true;
        assert!(batch.resume().is_err());
        batch.core.run_error = false;
        batch.close().unwrap();
        assert_eq!(batch.core.state, CoreStatus::Running);
        assert_eq!(batch.core.runs, 2);
    }

    #[test]
    fn reads_keep_exact_unaligned_span_and_check_deadline() {
        let mut batch = batches(HALTED);
        let deadline = Instant::now() + Duration::from_secs(1);
        let data = batch.memory(7, 1027, deadline).unwrap();
        assert_eq!(data, (7..1034).map(byte_at).collect::<Vec<_>>());
        assert_eq!(batch.core.reads, [(7, 1, 1), (8, 1024, 4), (1032, 2, 1)]);
        batch.core.reads.clear();
        assert!(batch.memory(8, 4, Instant::now()).is_err());
        assert!(batch.core.reads.is_empty());
    }

    #[test]
    fn word_reads_are_bounded_and_only_use_normal_memory() {
        let mut batch = batches(HALTED);
        let deadline = Instant::now() + Duration::from_secs(1);
        assert!(batch.memory(8, 0, deadline).unwrap().is_empty());
        let data = batch.memory(8, 1032, deadline).unwrap();
        assert_eq!(data, (8..1040).map(byte_at).collect::<Vec<_>>());
        assert_eq!(batch.core.reads, [(8, 1024, 4), (1032, 8, 4)]);
        batch.core.reads.clear();
        // Crossing a memory-map boundary is treated like unknown memory.
        batch.memory(2044, 8, deadline).unwrap();
        batch.memory(8, 3, deadline).unwrap();
        assert_eq!(batch.core.reads, [(2044, 8, 1), (8, 3, 1)]);
    }

    #[test]
    fn aligned_five_byte_metadata_uses_one_byte_width_read() {
        let mut batch = batches(HALTED);
        let data = batch
            .memory(8, 5, Instant::now() + Duration::from_secs(1))
            .unwrap();
        assert_eq!(data, [8, 9, 10, 11, 12]);
        assert_eq!(batch.core.reads, [(8, 5, 1)]);
    }

    #[test]
    fn failed_word_read_does_not_retry_or_read_tail() {
        let mut batch = batches(HALTED);
        batch.core.fail_at = Some(8);
        let error = batch
            .memory(7, 7, Instant::now() + Duration::from_secs(1))
            .unwrap_err();
        assert!(error.is::<crate::ReadFault>());
        assert_eq!(batch.core.reads, [(7, 1, 1), (8, 4, 4)]);
    }
}
