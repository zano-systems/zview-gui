//! Synchronous acquisition of matching, already-running firmware.
#[cfg(all(feature = "probe-rs", not(target_family = "wasm")))]
mod probe;
use crate::{
    Firmware, Memory, Selection, Session, Snapshot,
    recording::Trace,
    rsp::{Rsp, with_cleanup},
};
use anyhow::{Context, Result, bail, ensure};
#[cfg(all(feature = "probe-rs", not(target_family = "wasm")))]
pub use probe::ProbeSelector;
use serde_json::{Value, json};
use std::{
    fmt::Write,
    fs,
    net::SocketAddr,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Default, Clone)]
pub struct Cancellation(Arc<AtomicBool>);
impl Cancellation {
    /// Thread-safe. An in-flight batch gets to finish and resume the target.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

#[derive(Clone, Copy)]
pub enum Bound {
    Frames(u32),
    Duration(Duration),
    /// Explicit cancellation is a successful stop only if cleanup succeeds.
    UntilCancelled,
}

pub enum Target {
    Gdb(SocketAddr),
    #[cfg(all(feature = "probe-rs", not(target_family = "wasm")))]
    ProbeRs {
        chip: String,
        core: usize,
        probe: Option<ProbeSelector>,
        frequency_hz: u32,
    },
}

#[cfg(all(feature = "probe-rs", not(target_family = "wasm")))]
pub const DEFAULT_PROBE_FREQUENCY_HZ: u32 = 400_000;

impl Target {
    fn resolve_probe(&mut self) -> Result<()> {
        #[cfg(all(feature = "probe-rs", not(target_family = "wasm")))]
        if let Self::ProbeRs {
            chip,
            core,
            probe,
            frequency_hz,
        } = self
        {
            probe::frequency_khz(*frequency_hz)?;
            if probe.is_none() {
                *probe = Some(probe::discover(chip, *core)?);
            }
        }
        Ok(())
    }

    fn source(&self, period: Duration) -> Value {
        let mut source = match self {
            Self::Gdb(addr) => {
                json!({"runner":"gdb","runner_target":addr.to_string(),"frequency":null})
            }
            #[cfg(all(feature = "probe-rs", not(target_family = "wasm")))]
            Self::ProbeRs {
                chip,
                core,
                probe,
                frequency_hz,
            } => {
                json!({"runner":"probe-rs","runner_target":chip,"core":core,"probe":probe.as_ref().map(ToString::to_string),"frequency":frequency_hz})
            }
        };
        source["period"] = json!(period.as_secs_f64());
        source
    }

    fn open(self) -> Result<Box<dyn Transport>> {
        Ok(match self {
            Self::Gdb(addr) => Box::new(Rsp::open(addr)?),
            #[cfg(all(feature = "probe-rs", not(target_family = "wasm")))]
            Self::ProbeRs {
                chip,
                core,
                probe,
                frequency_hz,
            } => probe::open(
                &chip,
                core,
                probe.context("no resolved probe selector")?,
                frequency_hz,
            )?,
        })
    }
}

pub struct Config<'a> {
    pub elf: &'a Path,
    pub target: Target,
    pub period: Duration,
    pub selection: Selection,
    pub heap_maps: bool,
    /// Create a new gzip operation trace; never truncate an existing file.
    pub output: Option<&'a Path>,
}

// Physical operations only. Logical widths, little-endian conversion, batch
// markers and trace errors stay identical across transports.
pub(crate) trait Transport {
    fn halt(&mut self) -> Result<()>;
    fn resume(&mut self) -> Result<()>;
    fn memory(&mut self, at: u32, len: u32, deadline: Instant) -> Result<Vec<u8>>;
    fn close(&mut self) -> Result<()>;
}

struct Acquisition {
    transport: Box<dyn Transport>,
    trace: Option<Trace>,
    batch_deadline: Instant,
}
const BATCH_LIMIT: Duration = Duration::from_secs(5);
impl Acquisition {
    fn marker(&mut self, op: &str) -> Result<()> {
        if let Some(t) = &mut self.trace {
            t.marker(op)?;
        }
        Ok(())
    }
    fn bytes(&mut self, at: u32, amount: u32, width: u32) -> Result<Vec<u8>> {
        let len = amount
            .checked_mul(width)
            .context("live read size overflow")?;
        ensure!(len <= 128 * 1024, "logical read exceeds 128 KiB");
        ensure!(
            len == 0 || at.checked_add(len - 1).is_some(),
            "read crosses address limit"
        );
        self.transport.memory(at, len, self.batch_deadline)
    }
    fn observed<T>(
        &mut self,
        op: &str,
        at: u32,
        amount: u32,
        result: Result<T>,
        encode: impl FnOnce(&T) -> Value,
    ) -> Result<T> {
        if let Some(t) = &mut self.trace {
            t.read(op, at, amount, &result, encode)?;
        }
        result
    }
}
impl Memory for Acquisition {
    fn connect(&mut self) -> Result<()> {
        self.marker("connect")
    }
    fn begin_batch(&mut self) -> Result<()> {
        self.transport.halt()?;
        self.batch_deadline = Instant::now() + BATCH_LIMIT;
        self.marker("begin_batch")
    }
    fn end_batch(&mut self) -> Result<()> {
        self.transport.resume()?;
        self.marker("end_batch")
    }
    fn read8(&mut self, at: u32, amount: u32) -> Result<Vec<u8>> {
        let r = self.bytes(at, amount, 1);
        self.observed("read8", at, amount, r, |v| json!(v))
    }
    fn read32(&mut self, at: u32, amount: u32) -> Result<Vec<u32>> {
        let r = self.bytes(at, amount, 4).map(|b| {
            b.as_chunks::<4>()
                .0
                .iter()
                .copied()
                .map(u32::from_le_bytes)
                .collect()
        });
        self.observed("read32", at, amount, r, |v| json!(v))
    }
    fn read64(&mut self, at: u32) -> Result<u64> {
        let r = self
            .bytes(at, 1, 8)
            .map(|b| u64::from_le_bytes(b.try_into().unwrap()));
        self.observed("read64", at, 1, r, |v| json!([v]))
    }
    fn read_bytes(&mut self, at: u32, amount: u32) -> Result<Vec<u8>> {
        let r = self.bytes(at, amount, 1);
        self.observed("read_bytes", at, amount, r, |v| {
            let mut hex = String::with_capacity(v.len() * 2);
            for b in v {
                write!(hex, "{b:02x}").unwrap();
            }
            Value::String(hex)
        })
    }
}

/// Run on a worker thread. Callbacks run synchronously
/// outside stopped batches. Only complete frames reach `publish`; errors (including cancel)
/// are returned after attempting detach and closing the recording.
pub fn run(
    config: Config<'_>,
    bound: Bound,
    cancel: &Cancellation,
    mut publish: impl FnMut(Snapshot) -> Result<()>,
    mut notice: impl FnMut(&str),
) -> Result<u64> {
    ensure!(
        !matches!(config.target, Target::Gdb(addr) if !addr.ip().is_loopback()),
        "GDB requires a loopback address"
    );
    ensure!(
        !matches!(bound, Bound::Frames(0)),
        "frame bound must be >= 1"
    );
    ensure!(
        config.output.is_none() || config.heap_maps,
        "live recording requires heap-map capture"
    );
    let stop = || {
        let cancelled = cancel.is_cancelled();
        ensure!(
            !cancelled || matches!(bound, Bound::UntilCancelled),
            "live acquisition cancelled"
        );
        Ok(cancelled)
    };
    if stop()? {
        return Ok(0);
    }
    let bytes = fs::read(config.elf).with_context(|| format!("read {}", config.elf.display()))?;
    let (fw, features, entry, expected) =
        Firmware::live(&bytes).with_context(|| format!("resolve {}", config.elf.display()))?;
    if stop()? {
        return Ok(0);
    }
    let mut target = config.target;
    target.resolve_probe()?;
    if stop()? {
        return Ok(0);
    }
    let trace = config
        .output
        .map(|p| {
            Trace::create(
                p,
                target.source(config.period),
                &config.selection,
                &features,
            )
        })
        .transpose()?;
    let mut mem = Acquisition {
        transport: target.open()?,
        trace,
        batch_deadline: Instant::now() + BATCH_LIMIT,
    };
    let result = (|| {
        if stop()? {
            return Ok(0);
        }
        ensure!(
            mem.bytes(entry, expected.len() as u32, 1)? == expected,
            "target entry code does not match ELF at {entry:#x}"
        );
        if stop()? {
            return Ok(0);
        }
        let mut session = Session::discover(&mut mem, fw, features, config.selection)?;
        if !config.heap_maps {
            session.maps = Some(false);
        }
        for msg in session.notices(&session.selection) {
            notice(&msg);
        }
        let limit = match bound {
            Bound::Frames(n) => config
                .period
                .max(Duration::from_millis(100))
                .saturating_mul(n)
                .saturating_mul(5)
                .max(BATCH_LIMIT),
            Bound::Duration(d) => d,
            Bound::UntilCancelled => Duration::MAX,
        };
        let start = Instant::now();
        let mut seen = 0;
        loop {
            if stop()? {
                return Ok(seen);
            }
            if start.elapsed() >= limit {
                if matches!(bound, Bound::Duration(_)) {
                    return Ok(seen);
                }
                bail!("live frame bound not reached within {limit:?}");
            }
            let poll = Instant::now();
            match session.frame(&mut mem) {
                Ok(frame) => {
                    publish(frame)?;
                    seen += 1;
                }
                Err(e) if e.is::<crate::threads::Regression>() => {
                    for t in &mut session.threads {
                        t.reset();
                    }
                    notice("broken polling frame; resetting usage and watermark baselines");
                }
                Err(e) => return Err(e),
            }
            if stop()? {
                return Ok(seen);
            }
            if matches!(bound, Bound::Frames(n) if seen == u64::from(n)) {
                return Ok(seen);
            }
            loop {
                let left = config
                    .period
                    .saturating_sub(poll.elapsed())
                    .min(limit.saturating_sub(start.elapsed()));
                if left.is_zero() {
                    break;
                }
                std::thread::sleep(left.min(Duration::from_millis(25)));
                if stop()? {
                    return Ok(seen);
                }
            }
        }
    })();
    let cleanup = mem.transport.close();
    drop(mem.transport);
    let cleanup = cleanup.and_then(|()| match &mut mem.trace {
        Some(t) => t.marker("disconnect"),
        None => Ok(()),
    });
    let combine = |r: Result<u64>, cleanup: Result<()>| match r {
        Ok(n) => cleanup.map(|()| n),
        Err(e) => Err(with_cleanup(e, cleanup)),
    };
    let result = combine(result, cleanup);
    if let Some(t) = mem.trace {
        return combine(result, t.finish().context("finish recording"));
    }
    result
}
