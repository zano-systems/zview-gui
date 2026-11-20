//! Thread discovery, cached watermark scans and per-address cycle baselines.
use crate::{
    Selection,
    dwarf::Field,
    firmware::{Firmware, ThreadLayout, address},
    memory::Memory,
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

#[derive(Debug)]
pub(crate) struct Regression;
impl std::fmt::Display for Regression {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("thread cycle counter regressed")
    }
}
impl std::error::Error for Regression {}

pub(crate) struct Thread {
    pub name: String,
    pub ptr: u32,
    stack: Option<(u32, u32)>,
    watermark: u32,
    cycles: Option<u64>,
    entry: Option<u32>,
    legacy_meta: Option<bool>,
}
impl Thread {
    pub fn reset(&mut self) {
        self.watermark = 0;
        self.cycles = None;
    }
    fn delta(&mut self, cycles: u64) -> Result<u64> {
        Ok(cycles
            .checked_sub(self.cycles.replace(cycles).unwrap_or(cycles))
            .ok_or(Regression)?)
    }
    fn scan(&mut self, mem: &mut impl Memory, reserved: u32) -> Result<Option<u32>> {
        let Some((start, size)) = self.stack else {
            return Ok(None);
        };
        if size == 0 {
            return Ok(Some(0));
        }
        let data = size
            .checked_sub(reserved)
            .context("stack smaller than sentinel")?;
        let words = mem.read32(address(start, reserved)?, data / 4 - self.watermark / 4)?;
        self.watermark = data - 4 * words.iter().take_while(|&&w| w == 0xaaaa_aaaa).count() as u32;
        Ok(Some(self.watermark + reserved))
    }
}

pub(crate) fn discover(
    mem: &mut impl Memory,
    t: &ThreadLayout,
    distinct: bool,
) -> Result<Vec<Thread>> {
    mem.begin_batch()?;
    let mut ptr = mem.word(t.head)?;
    let mut threads: Vec<Thread> = Vec::new();
    let mut seen = HashSet::new();
    while ptr != 0 {
        ensure!(
            seen.insert(ptr) && seen.len() <= 64,
            "cyclic or oversized thread list"
        );
        let raw: Vec<_> = mem
            .read32(ptr, t.size / 4)?
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect();
        let slice = |f: Field| &raw[f.offset as usize..(f.offset + f.size) as usize];
        let word = |f| -> Result<u32> { Ok(u32::from_le_bytes(slice(f).try_into()?)) };
        let mut name = t
            .name
            .map(|f| {
                String::from_utf8_lossy(slice(f).split(|&c| c == 0).next().unwrap()).into_owned()
            })
            .unwrap_or_else(|| format!("thread @ 0x{ptr:X}"));
        if distinct && name.is_empty() {
            name = format!("thread @ 0x{ptr:X}")
        }
        let mut duplicate = threads.iter().position(|d| d.name == name);
        if distinct && duplicate.is_some() {
            name = format!("{name} @ 0x{ptr:X}");
            duplicate = None;
        }
        let thread = Thread {
            name,
            ptr,
            stack: t
                .stack
                .map(|(a, b)| -> Result<_> { Ok((word(a)?, word(b)?)) })
                .transpose()?,
            entry: None,
            legacy_meta: None,
            cycles: None,
            watermark: 0,
        };
        if let Some(i) = duplicate {
            threads[i] = thread;
        } else {
            threads.push(thread);
        }
        ptr = word(t.next)?;
    }
    mem.end_batch()?;
    Ok(threads)
}

fn metadata(
    mem: &mut impl Memory,
    t: &ThreadLayout,
    d: &mut Thread,
    merged: bool,
) -> Result<[Option<u32>; 3]> {
    let mut values = [None; 3];
    if merged {
        if let Some(base) = t.meta.iter().flatten().map(|f| f.offset).min() {
            let end = t
                .meta
                .iter()
                .flatten()
                .map(|f| f.offset + f.size)
                .max()
                .unwrap();
            let raw = mem.read8(address(d.ptr, base)?, end - base)?;
            for (v, f) in values.iter_mut().zip(t.meta) {
                if let Some(f) = f {
                    let i = (f.offset - base) as usize;
                    *v = Some(if f.size == 2 {
                        u16::from_le_bytes([raw[i], raw[i + 1]]).into()
                    } else {
                        raw[i].into()
                    });
                }
            }
        }
    } else {
        // Pre-merged recordings may predate metadata entirely. Only probe
        // the first field: once present the rest are strict requests.
        if d.legacy_meta == Some(false) {
            return Ok(values);
        }
        for (v, f) in values.iter_mut().zip(t.meta) {
            if let Some(f) = f {
                let at = address(d.ptr, f.offset)?;
                let byte = if d.legacy_meta.is_none() {
                    let byte = mem.optional_byte(at)?;
                    d.legacy_meta = Some(byte.is_some());
                    let Some(b) = byte else {
                        return Ok(values);
                    };
                    b
                } else {
                    mem.read8(at, 1)?[0]
                };
                *v = Some(byte.into());
            }
        }
    }
    if let Some(f) = t.entry
        && (!merged || d.entry.is_none())
    {
        let at = address(d.ptr, f.offset)?;
        d.entry = if !merged && d.legacy_meta.is_none() {
            let entry = mem.optional_word(at)?;
            d.legacy_meta = Some(entry.is_some());
            entry
        } else {
            Some(mem.word(at)?)
        };
    }
    Ok(values)
}

pub(crate) fn poll(
    mem: &mut impl Memory,
    fw: &Firmware,
    threads: &mut [Thread],
    features: &[String],
    sel: &Selection,
) -> Result<Vec<Value>> {
    let t = fw.thread.as_ref().context("missing thread layout")?;
    let has = |name: &str| features.iter().any(|f| f == name);
    let usage = t
        .cycles
        .filter(|_| t.kernel_usage.is_some() || has("thread_usage_alone"));
    let mut deltas = HashMap::new();
    let mut coherent = None;
    if let Some(counter) = usage {
        if has("usage_snapshot") {
            let stamps = |m: &mut _| -> Result<Vec<u32>> {
                t.stamps.iter().map(|&a| Memory::word(m, a)).collect()
            };
            for attempt in 1..=2 {
                let before = stamps(mem)?;
                let counters = threads
                    .iter()
                    .map(|d| mem.read64(address(d.ptr, counter.offset)?))
                    .collect::<Result<Vec<_>>>()?;
                let same = before == stamps(mem)?;
                if same || has("single_usage_pass") || attempt == 2 {
                    coherent = (!before.is_empty()).then_some(same);
                    for (d, c) in threads.iter_mut().zip(counters) {
                        deltas.insert(d.ptr, d.delta(c)?);
                    }
                    break;
                }
            }
        } else {
            mem.word(
                t.kernel_usage
                    .context("missing legacy kernel usage counter")?,
            )?;
        }
    }
    let mut rows = Vec::new();
    for d in threads.iter_mut().filter(|d| sel.row("threads", &d.name)) {
        if let Some(f) = usage
            && !has("usage_snapshot")
        {
            deltas.insert(d.ptr, d.delta(mem.read64(address(d.ptr, f.offset)?)?)?);
        }
        let watermark = if has("unfilled_stacks") {
            None
        } else {
            d.scan(mem, 4 * u32::from(has("stack_sentinel")))?
        };
        let size = d.stack.map(|(_, s)| s);
        let pct = watermark
            .zip(size)
            .map(|(w, s)| percent(w.into(), s.into()));
        let [priority, state, options] = metadata(mem, t, d, has("merged_thread_meta"))?;
        let active = deltas.get(&d.ptr).is_some_and(|&x| x > 0);
        let display = |v: Option<u32>| v.map_or_else(|| "-".into(), |n| n.to_string());
        let row = json!({"name":d.name,"address":d.ptr,"stack_start":d.stack.map(|(a,_)|a),"stack_size":size,
            "priority":priority.map(|p|p as i8),"state":state,"user_options":options,"entry_point":d.entry,
            "entry_symbol":d.entry.filter(|&a|a!=0).and_then(|a|fw.symbol(a & !1)),
            "entry_site":d.entry.and_then(|a|fw.inventory.functions.get(&(a & !1))),
            "is_idle":idle(t,d.ptr),
            "runtime":{"is_active":active,"is_cpu_coherent":coherent,"stack_watermark":watermark,"stack_watermark_percent":pct,"cpu":null,"cpu_normalized":null},
            "line":format!("thread {:30} stack={:>6}  watermark={}",d.name,display(size),display(watermark))});
        rows.push((d.ptr, row));
    }
    if usage.is_some() {
        let total = deltas.values().map(|&v| u128::from(v)).sum::<u128>();
        let busy = deltas
            .iter()
            .filter(|(a, _)| !idle(t, **a))
            .map(|(_, v)| u128::from(*v))
            .sum::<u128>();
        for (ptr, row) in &mut rows {
            let d = deltas[ptr];
            let share = percent(d.into(), total);
            row["runtime"]["cpu_normalized"] = json!(share);
            row["runtime"]["cpu"] = json!(if idle(t, *ptr) {
                0.0
            } else {
                percent(d.into(), busy)
            });
            row["glance"] = json!({"fill":share,"text":if d>0 {"running"}else{"waiting"},"level":if d>0{"busy"}else{"quiet"}});
        }
    }
    Ok(rows.into_iter().map(|(_, row)| row).collect())
}
fn idle(t: &ThreadLayout, a: u32) -> bool {
    t.idle.contains(&a) && (a - t.idle.start).is_multiple_of(t.size)
}
pub(crate) fn percent(n: u128, total: u128) -> f64 {
    if total == 0 {
        0.0
    } else {
        n as f64 / total as f64 * 100.0
    }
}
