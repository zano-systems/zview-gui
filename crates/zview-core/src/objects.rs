//! Resolve polling geometry once; no recording entries or fixture addresses
//! participate in instance discovery, pointer traversal or span selection.
use crate::{
    dwarf::{MissingLayout, Reader, with_struct},
    firmware::{Firmware, address},
    heap,
    memory::Memory,
};
use anyhow::{Context, Result, ensure};
use gimli::Dwarf;
use serde_json::{Value, json};
use std::collections::HashMap;

pub(crate) struct ObjectLayout {
    pub key: &'static str,
    pub ty: &'static str,
    pub size: u32,
    offsets: HashMap<&'static str, u32>,
    base: u32,
    words: u32,
    wait: Option<u32>,
    qnode: Option<u32>,
}
impl ObjectLayout {
    pub fn resolve(
        d: &Dwarf<Reader<'_>>,
        features: &[String],
        simple: bool,
        discover: bool,
    ) -> Result<Vec<Self>> {
        let has = |key: &str| features.iter().any(|f| f == key);
        let mut out = Vec::new();
        for (key, ty, fields) in [
            ("heaps", "k_heap", &[][..]),
            ("semaphores", "k_sem", &["count", "limit"]),
            ("mutexes", "k_mutex", &["owner", "lock_count"]),
            ("msgqs", "k_msgq", &["used_msgs", "max_msgs", "msg_size"]),
            (
                "mem_slabs",
                "k_mem_slab",
                &["num_blocks", "block_size", "num_used"],
            ),
            ("workqs", "k_work_q", &["pending", "flags"]),
        ] {
            if !has(key) {
                continue;
            }
            match Self::layout(d, key, ty, fields, has("heap_waiters"), simple) {
                Err(e) if discover && e.is::<MissingLayout>() => continue,
                result => out.push(result?),
            }
        }
        Ok(out)
    }

    fn layout(
        d: &Dwarf<Reader<'_>>,
        key: &'static str,
        ty: &'static str,
        fields: &[&'static str],
        heap_waiters: bool,
        simple: bool,
    ) -> Result<Self> {
        let mut obj = with_struct(d, ty, |t, r| {
            let size = t.size(r)?;
            let mut offsets = HashMap::new();
            for &name in fields {
                let nested = if key == "mem_slabs" {
                    t.optional(r, &format!("info.{name}"))?
                } else {
                    None
                };
                let f = match nested {
                    Some(f) => f,
                    None => t.field(r, name)?,
                };
                ensure!(
                    f.offset % 4 == 0 && (f.size == 4 || (name == "pending" && f.size >= 4)),
                    "invalid {ty}.{name}"
                );
                offsets.insert(name, f.offset);
            }
            let mut base = 0;
            let mut words = size.div_ceil(4);
            let wait_field = if key == "workqs" { "drainq" } else { "wait_q" };
            let wait = if key == "heaps" && !heap_waiters {
                None
            } else {
                t.optional(r, wait_field)?.map(|f| f.offset)
            };
            if key == "mem_slabs"
                && let Some(f) = t.optional(r, "info.max_used")?
            {
                ensure!(f.size == 4 && f.offset % 4 == 0, "invalid max_used");
                offsets.insert("max_used", f.offset);
            }
            if key == "workqs" {
                if let Some(f) = t.optional(r, "thread_id")? {
                    offsets.insert("thread_id", f.offset);
                }
                base = *offsets.values().chain(wait.iter()).min().unwrap();
                words = (*offsets.values().chain(wait.iter()).max().unwrap() - base) / 4 + 1;
                if let Some(f) = t.optional(r, "thread")? {
                    offsets.insert("thread", f.offset);
                }
            }
            Ok(Self {
                key,
                ty,
                size,
                offsets,
                wait,
                qnode: None,
                base,
                words,
            })
        })?;
        let extra = match key {
            "heaps" => Some((
                "z_heap",
                &[
                    "free_bytes",
                    "allocated_bytes",
                    "max_allocated_bytes",
                    "end_chunk",
                ][..],
            )),
            "workqs" => Some(("k_work", &["node", "handler"][..])),
            _ => None,
        };
        if let Some((ty, fields)) = extra {
            with_struct(d, ty, |t, r| {
                for &f in fields {
                    obj.offsets.insert(f, t.field(r, f)?.offset);
                }
                Ok(())
            })?;
        }
        if simple && obj.wait.is_some() {
            obj.qnode = with_struct(d, "k_thread", |t, r| {
                Ok(t.optional(r, "base.qnode_dlist")?.map(|f| f.offset))
            })?;
        }
        Ok(obj)
    }

    fn waiters(
        &self,
        mem: &mut impl Memory,
        at: u32,
        head: Option<u32>,
        names: &HashMap<u32, String>,
    ) -> Result<Option<Vec<String>>> {
        let Some((off, qnode)) = self.wait.zip(self.qnode) else {
            return Ok(None);
        };
        let list = address(at, off)?;
        let mut node = match head {
            Some(h) => h,
            None => mem.word(list)?,
        };
        let mut seen = Vec::new();
        let mut labels = Vec::new();
        while node != 0 && node != list && seen.len() < 32 && !seen.contains(&node) {
            seen.push(node);
            let ptr = node
                .checked_sub(qnode)
                .context("wait node before enclosing thread")?;
            labels.push(
                names
                    .get(&ptr)
                    .cloned()
                    .unwrap_or_else(|| format!("thread @ 0x{ptr:X}")),
            );
            node = mem.word(node)?;
        }
        Ok(Some(labels))
    }

    pub fn read(
        &self,
        mem: &mut impl Memory,
        fw: &Firmware,
        names: &HashMap<u32, String>,
        maps: &mut Option<bool>,
        name: &str,
        at: u32,
    ) -> Result<Value> {
        let o = &self.offsets;
        let mut ptr = at;
        let (mut row, waiters) = if self.key == "heaps" {
            ptr = mem.word(at)?;
            let free = mem.word(address(ptr, o["free_bytes"])?)?;
            let alloc = mem.word(address(ptr, o["allocated_bytes"])?)?;
            let peak = mem.word(address(ptr, o["max_allocated_bytes"])?)?;
            let waiters = self.waiters(mem, at, None, names)?;
            let chunks = heap::chunks(mem, ptr, o["end_chunk"], maps)?;
            (
                json!({"free_bytes":free,"allocated_bytes":alloc,"max_allocated_bytes":peak,"chunks":chunks}),
                waiters,
            )
        } else {
            let raw = mem.read32(address(at, self.base)?, self.words)?;
            let word_at = |off: u32| -> Result<u32> {
                let index = off.checked_sub(self.base).context("member before span")?;
                ensure!(index % 4 == 0, "unaligned word field");
                raw.get((index / 4) as usize)
                    .copied()
                    .context("member outside span")
            };
            let word = |field| word_at(o[field]);
            let row = match self.key {
                "semaphores" => json!({"count":word("count")?,"limit":word("limit")?}),
                "mutexes" => {
                    let owner = word("owner")?;
                    json!({"lock_count":word("lock_count")?,"owner_address":owner,"owner_name":names.get(&owner)})
                }
                "msgqs" => {
                    json!({"used_msgs":word("used_msgs")?,"max_msgs":word("max_msgs")?,"msg_size":word("msg_size")?})
                }
                "mem_slabs" => {
                    json!({"num_blocks":word("num_blocks")?,"block_size":word("block_size")?,"num_used":word("num_used")?,"max_used":o.get("max_used").copied().map(word_at).transpose()?})
                }
                "workqs" => {
                    let mut node = word("pending")?;
                    let mut seen = Vec::new();
                    let mut items = Vec::new();
                    while node != 0 && seen.len() < 16 && !seen.contains(&node) {
                        seen.push(node);
                        let item = node
                            .checked_sub(o["node"])
                            .context("invalid pending work pointer")?;
                        let handler = mem.word(address(item, o["handler"])?)?;
                        let handler = fw
                            .symbol(handler)
                            .map(str::to_owned)
                            .unwrap_or_else(|| format!("0x{handler:X}"));
                        items.push(
                            fw.symbol(item)
                                .map(|n| format!("{n} ({handler})"))
                                .unwrap_or(handler),
                        );
                        node = mem.word(node)?;
                    }
                    let thread = if o.contains_key("thread_id") {
                        Some(word("thread_id")?)
                    } else {
                        o.get("thread").map(|&off| address(at, off)).transpose()?
                    };
                    json!({"flags":word("flags")?,"pending":items,"is_pending_truncated":node!=0,"thread_name":thread.and_then(|p|names.get(&p))})
                }
                _ => unreachable!(),
            };
            (
                row,
                self.waiters(mem, at, self.wait.map(word_at).transpose()?, names)?,
            )
        };
        row["name"] = json!(name);
        row["address"] = json!(ptr);
        row["decl_site"] = json!(fw.inventory.decl.get(&ptr));
        super::presentation::describe(self.key, &mut row, name, waiters.as_deref());
        row["waiters"] = json!(waiters);
        Ok(row)
    }
}
