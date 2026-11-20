use crate::{
    dwarf::{Field, with_struct},
    inventory::{Inventory, Symbols},
    objects::ObjectLayout,
};
use anyhow::{Context, Result, ensure};
use gimli::{Dwarf, EndianSlice, LittleEndian, SectionId};
use object::{Architecture, Object, ObjectSection, ObjectSegment, ObjectSymbol, SymbolKind};
use std::{collections::HashMap, ops::Range};

pub(crate) struct ThreadLayout {
    pub size: u32,
    pub next: Field,
    pub name: Option<Field>,
    pub stack: Option<(Field, Field)>,
    pub cycles: Option<Field>,
    pub meta: [Option<Field>; 3],
    pub entry: Option<Field>,
    pub head: u32,
    pub kernel_usage: Option<u32>,
    pub stamps: Vec<u32>,
    pub idle: Range<u32>,
}
pub(crate) struct Firmware {
    pub thread: Option<ThreadLayout>,
    pub objects: Vec<ObjectLayout>,
    pub inventory: Inventory,
    symbols: HashMap<u32, String>,
}
pub(crate) fn address(base: u32, offset: u32) -> Result<u32> {
    base.checked_add(offset).context("target address overflow")
}

impl Firmware {
    /// Check immutable entry code before trusting live pointers. Keep this read
    /// outside the operation trace: replay never verifies a connected target.
    pub(crate) fn live(bytes: &[u8]) -> Result<(Self, Vec<String>, u32, &[u8])> {
        let elf = object::read::elf::ElfFile32::<object::Endianness>::parse(bytes)?;
        let entry = elf.entry() & !1;
        let span = elf
            .segments()
            .find_map(|s| s.data_range(entry, 64).transpose())
            .context("ELF entry lacks 64 file-backed bytes")??;
        let mut features: Vec<String> = crate::GROUPS
            .into_iter()
            .chain([
                "merged_thread_meta",
                "distinct_names",
                "thread_usage_alone",
                "single_usage_pass",
                "heap_waiters",
                "usage_snapshot",
            ])
            .map(String::from)
            .collect();
        if elf.symbol_by_name("CONFIG_INIT_STACKS").is_none() {
            features.push("unfilled_stacks".into());
        }
        if elf.symbol_by_name("CONFIG_STACK_SENTINEL").is_some() {
            features.push("stack_sentinel".into());
        }
        let fw = Self::inspect(&elf, &features, true)?;
        let present = |key: &str| {
            fw.objects.iter().any(|g| {
                g.key == key
                    && fw
                        .inventory
                        .objects
                        .get(g.ty)
                        .is_some_and(|rows| !rows.is_empty())
            })
        };
        features.retain(|f| match f.as_str() {
            "threads" => true,
            "heap_waiters" => present("heaps"),
            group if crate::GROUPS.contains(&group) => present(group),
            _ => true,
        });
        if fw.thread.as_ref().is_none_or(|t| t.cycles.is_none()) {
            features.retain(|f| f != "usage_snapshot");
        }
        Ok((fw, features, entry.try_into()?, span))
    }

    pub(crate) fn parse(bytes: &[u8], features: &[String]) -> Result<Self> {
        let elf = object::read::elf::ElfFile32::<object::Endianness>::parse(bytes)?;
        Self::inspect(&elf, features, false)
    }
    fn inspect(
        elf: &object::read::elf::ElfFile32<'_, object::Endianness>,
        features: &[String],
        discover: bool,
    ) -> Result<Self> {
        ensure!(
            elf.is_little_endian() && elf.architecture() == Architecture::Arm,
            "requires little-endian ARM ELF32"
        );
        let has = |name: &str| features.iter().any(|f| f == name);
        let symbol = |name| -> Result<(u32, u32)> {
            let s = elf
                .symbol_by_name(name)
                .filter(|s| s.is_definition())
                .with_context(|| format!("missing ELF symbol {name}"))?;
            Ok((s.address().try_into()?, s.size().try_into()?))
        };
        let mut symbols = HashMap::new();
        let mut data = Symbols::new();
        for s in elf.symbols().filter(|s| s.is_definition()) {
            let a = s.address().try_into()?;
            if s.kind() == SymbolKind::Data {
                data.entry(s.name()?.into())
                    .or_default()
                    .push((a, s.size().try_into()?));
            }
            if matches!(s.kind(), SymbolKind::Text | SymbolKind::Data) {
                symbols.entry(a).or_insert(s.name()?.to_owned());
            }
        }
        let dwarf = Dwarf::load(|id: SectionId| -> Result<_> {
            let section = elf
                .section_by_name(id.name())
                .map(|s| s.data())
                .transpose()?
                .unwrap_or_default();
            Ok(EndianSlice::new(section, LittleEndian))
        })?;
        let simple = elf.symbol_by_name("CONFIG_WAITQ_SIMPLE").is_some();
        let thread = if has("threads") {
            let (kernel, _) = symbol("_kernel")?;
            let (idle, idle_size) = symbol("z_idle_threads")?;
            let (head, kernel_usage, cpus) = with_struct(&dwarf, "z_kernel", |t, r| {
                Ok((
                    address(kernel, t.field(r, "threads")?.offset)?,
                    t.optional(r, "usage")?
                        .map(|f| address(kernel, f.offset))
                        .transpose()?,
                    t.optional(r, "cpus")?
                        .map(|f| address(kernel, f.offset))
                        .transpose()?,
                ))
            })?;
            let mut layout = with_struct(&dwarf, "k_thread", |t, r| {
                let field = |p, sizes: &[u32]| -> Result<Option<Field>> {
                    let f = t.optional(r, p)?;
                    ensure!(
                        f.is_none_or(|f| sizes.contains(&f.size)),
                        "unsupported field width: {p}"
                    );
                    Ok(f)
                };
                Ok(ThreadLayout {
                    size: t.size(r)?,
                    next: field("next_thread", &[4])?.context("missing next_thread")?,
                    name: t.optional(r, "name")?,
                    stack: field("stack_info.start", &[4])?.zip(field("stack_info.size", &[4])?),
                    cycles: field("base.usage.total", &[8])?,
                    meta: [
                        field("base.prio", &[1])?,
                        field("base.thread_state", &[1])?,
                        field("base.user_options", &[1, 2])?,
                    ],
                    entry: field("entry.pEntry", &[4])?,
                    head,
                    kernel_usage,
                    stamps: Vec::new(),
                    idle: idle..address(idle, idle_size)?,
                })
            })?;
            ensure!(
                layout.size != 0 && layout.size % 4 == 0 && idle_size % layout.size == 0,
                "invalid k_thread size"
            );
            if has("usage_snapshot")
                && let Some(base) = cpus
            {
                layout.stamps = with_struct(&dwarf, "_cpu", |t, r| {
                    let Some(stamp) = t.optional(r, "usage0")? else {
                        return Ok(vec![]);
                    };
                    let first = address(base, stamp.offset)?;
                    let stride = t.size(r)?;
                    (0..(idle_size / layout.size).max(1))
                        .map(|i| address(first, i.checked_mul(stride).context("_cpu overflow")?))
                        .collect()
                })?;
            }
            Some(layout)
        } else {
            None
        };
        let objects = ObjectLayout::resolve(&dwarf, features, simple, discover)?;
        let types = objects.iter().map(|o| (o.ty, o.size)).collect();
        let inventory = Inventory::scan(&dwarf, &types, &data)?;
        Ok(Self {
            thread,
            objects,
            inventory,
            symbols,
        })
    }
    pub fn symbol(&self, ptr: u32) -> Option<&str> {
        self.symbols
            .get(&ptr)
            .or_else(|| self.symbols.get(&(ptr ^ 1)))
            .map(String::as_str)
    }
}
