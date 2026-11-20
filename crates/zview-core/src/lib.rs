//! Headless snapshot decoding. The replay cursor owns operation matching;
//! the session owns discovery, polling order and cross-poll state.
#![forbid(unsafe_code)]
mod dwarf;
mod firmware;
mod heap;
mod inventory;
pub mod live;
mod memory;
mod objects;
mod presentation;
mod recording;
mod replay;
mod rsp;
mod selection;
mod threads;

use anyhow::{Context, Result, bail, ensure};
use firmware::Firmware;
use flate2::read::MultiGzDecoder;
use memory::Memory;
pub use selection::{GROUPS, Selection};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashMap},
    fs::{self, File},
    io::BufReader,
    path::Path,
    time::{Duration, Instant},
};
pub const REFERENCE_VERSION: &str = "0.28.1";
pub type Snapshot = BTreeMap<&'static str, Vec<Value>>;

#[derive(Debug)]
struct ReadFault(String);
impl std::fmt::Display for ReadFault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for ReadFault {}

struct Session {
    fw: Firmware,
    features: Vec<String>,
    selection: Selection,
    threads: Vec<threads::Thread>,
    names: HashMap<u32, String>,
    maps: Option<bool>,
}
impl Session {
    fn discover(
        mem: &mut impl Memory,
        fw: Firmware,
        features: Vec<String>,
        selection: Selection,
    ) -> Result<Self> {
        mem.connect()?;
        let threads = if let Some(t) = &fw.thread {
            threads::discover(mem, t, features.iter().any(|f| f == "distinct_names"))?
        } else {
            vec![]
        };
        let names = threads.iter().map(|d| (d.ptr, d.name.clone())).collect();
        Ok(Self {
            fw,
            features,
            selection,
            threads,
            names,
            maps: None,
        })
    }
    fn notices(&self, shown: &Selection) -> Vec<String> {
        let mut inventory = vec![];
        if !self.threads.is_empty() {
            inventory.push((
                "threads",
                self.threads.iter().map(|d| d.name.as_str()).collect(),
            ));
        }
        for group in &self.fw.objects {
            if let Some(objs) = self
                .fw
                .inventory
                .objects
                .get(group.ty)
                .filter(|o| !o.is_empty())
            {
                inventory.push((group.key, objs.iter().map(|(n, _)| n.as_str()).collect()))
            }
        }
        shown.notices(&inventory)
    }
    fn groups(&mut self, mem: &mut impl Memory) -> Result<Snapshot> {
        let mut frame = Snapshot::new();
        if !self.threads.is_empty() && self.selection.group("threads") {
            frame.insert(
                "threads",
                threads::poll(
                    mem,
                    &self.fw,
                    &mut self.threads,
                    &self.features,
                    &self.selection,
                )?,
            );
        }
        for group in &self.fw.objects {
            if !self.selection.group(group.key) {
                continue;
            }
            let Some(objects) = self
                .fw
                .inventory
                .objects
                .get(group.ty)
                .filter(|o| !o.is_empty())
            else {
                continue;
            };
            let mut rows = Vec::new();
            for (name, addr) in objects
                .iter()
                .filter(|(n, _)| self.selection.row(group.key, n))
            {
                rows.push(
                    group
                        .read(mem, &self.fw, &self.names, &mut self.maps, name, *addr)
                        .with_context(|| format!("poll {} {name}", group.key))?,
                );
            }
            frame.insert(group.key, rows);
        }
        Ok(frame)
    }
    fn frame(&mut self, mem: &mut impl Memory) -> Result<Snapshot> {
        mem.begin_batch()?;
        let result = self.groups(mem);
        // A captured fault or regression can end a partial batch. Never hunt
        // for end_batch after a mismatch, or publish after a failed marker.
        if result
            .as_ref()
            .err()
            .is_none_or(|e| e.is::<ReadFault>() || e.is::<threads::Regression>())
        {
            mem.end_batch()?;
        }
        result
    }
}

/// The result of reading the next complete batch (or the checked end marker).
pub enum ReplayPoll {
    Frame(Snapshot),
    Notice(String),
    End,
}

/// One strict replay cursor and its decoder state. Drive from a worker, and
/// stop between polls for cancellation. The caller controls delivery pacing.
pub struct ReplaySession {
    replay: replay::Replay<BufReader<MultiGzDecoder<File>>>,
    session: Session,
    faults: u32,
    ended: bool,
}

impl ReplaySession {
    pub fn open(input: &Path, elf: &Path) -> Result<Self> {
        let f = File::open(input).with_context(|| format!("open {}", input.display()))?;
        let mut replay = replay::Replay::new(BufReader::new(MultiGzDecoder::new(f)))?;
        let bytes = fs::read(elf).with_context(|| format!("read {}", elf.display()))?;
        let fw = Firmware::parse(&bytes, &replay.features)
            .with_context(|| format!("resolve {}", elf.display()))?;
        let features = replay.features.clone();
        let selection = replay.selection.clone();
        let session = Session::discover(&mut replay, fw, features, selection)?;
        Ok(Self {
            replay,
            session,
            faults: 0,
            ended: false,
        })
    }

    /// A notice consumes a broken batch, not a frame. On error, stop using
    /// this cursor; no attempt is made to resynchronize a corrupt recording.
    pub fn poll(&mut self) -> Result<ReplayPoll> {
        if self.ended || self.replay.finished()? {
            self.ended = true;
            return Ok(ReplayPoll::End);
        }
        match self.session.frame(&mut self.replay) {
            Ok(snapshot) => {
                self.faults = 0;
                Ok(ReplayPoll::Frame(snapshot))
            }
            Err(e) if e.is::<threads::Regression>() => {
                for t in &mut self.session.threads {
                    t.reset();
                }
                Ok(ReplayPoll::Notice(
                    "broken polling frame; resetting usage and watermark baselines".into(),
                ))
            }
            Err(e) if e.is::<ReadFault>() => {
                self.faults += 1;
                if self.faults >= 3 {
                    return Err(e.context("three successive recorded read faults"));
                }
                Ok(ReplayPoll::Notice(format!("transient read fault: {e:#}")))
            }
            Err(e) => Err(e),
        }
    }
}

/// Decode complete polling frames synchronously, with no recorded-time pacing
/// or event queue drops. Stop at the requested frame, not at a read prefix.
pub fn dump(
    input: &Path,
    elf: &Path,
    frame: u32,
    period: Duration,
    shown: &Selection,
    mut notice: impl FnMut(&str),
) -> Result<Snapshot> {
    ensure!(frame >= 1, "frame must be >= 1");
    let mut replay = ReplaySession::open(input, elf)?;
    for msg in replay.session.notices(shown) {
        notice(&msg);
    }
    let start = Instant::now();
    let timeout = period
        .max(Duration::from_millis(100))
        .saturating_mul(frame)
        .saturating_mul(5)
        .max(Duration::from_secs(5));
    let mut seen = 0;
    loop {
        ensure!(
            start.elapsed() <= timeout,
            "no frame {frame} within {timeout:?}"
        );
        let poll = Instant::now();
        match replay.poll()? {
            ReplayPoll::End => {
                bail!("recording has only {seen} frames; --frame {frame} out of range")
            }
            ReplayPoll::Notice(message) => notice(&message),
            ReplayPoll::Frame(mut snapshot) => {
                seen += 1;
                if seen == frame {
                    snapshot.retain(|g, rows| {
                        rows.retain(|row| shown.row(g, row["name"].as_str().unwrap()));
                        shown.group(g)
                    });
                    return Ok(snapshot);
                }
            }
        }
        std::thread::sleep(period.saturating_sub(poll.elapsed()));
    }
}
