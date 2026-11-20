//! DWARF discovery order, not symbol/address order, determines polling order.
use crate::dwarf::Reader;
use anyhow::Result;
use gimli::{self as g, AttributeValue as A};
use std::collections::HashMap;

pub(crate) type Site = (String, u64);
pub(crate) type Symbols = HashMap<String, Vec<(u32, u32)>>;
#[derive(Default)]
pub(crate) struct Inventory {
    pub objects: HashMap<String, Vec<(String, u32)>>,
    pub decl: HashMap<u32, Site>,
    pub functions: HashMap<u32, Site>,
}

type Die<'a, 'u, 'b> = g::DebuggingInformationEntry<'a, 'u, Reader<'b>>;
fn site<'b>(
    d: &g::Dwarf<Reader<'b>>,
    u: &g::Unit<Reader<'b>>,
    e: &Die<'_, '_, 'b>,
    named: &Die<'_, '_, 'b>,
) -> Result<Option<Site>> {
    let attr = |k| -> Result<_> { Ok(e.attr_value(k)?.or(named.attr_value(k)?)) };
    let (Some(A::FileIndex(index)), Some(line), Some(lp)) = (
        attr(g::DW_AT_decl_file)?,
        attr(g::DW_AT_decl_line)?.and_then(|v| v.udata_value()),
        &u.line_program,
    ) else {
        return Ok(None);
    };
    let h = lp.header();
    let Some(file) = h.file(index) else {
        return Ok(None);
    };
    let string = |a| -> Result<String> {
        Ok(String::from_utf8_lossy(d.attr_string(u, a)?.slice()).into_owned())
    };
    let mut path = string(file.path_name())?;
    if !path.starts_with('/')
        && let Some(dir) = file.directory(h)
    {
        let dir = string(dir)?;
        if !dir.is_empty() {
            path = format!("{dir}/{path}");
        }
    }
    Ok(Some((path, line)))
}
#[derive(Default)]
struct Instances {
    name: String,
    addresses: Vec<u32>,
    declared: bool,
}
impl Inventory {
    pub fn scan(
        d: &g::Dwarf<Reader<'_>>,
        types: &HashMap<&str, u32>,
        symbols: &Symbols,
    ) -> Result<Self> {
        let mut inventory = Self::default();
        let mut vars: HashMap<String, Vec<Instances>> = HashMap::new();
        let mut units = d.units();
        while let Some(header) = units.next()? {
            let u = d.unit(header)?;
            let mut dies = u.entries();
            while let Some((_, e)) = dies.next_dfs()? {
                if e.tag() == g::DW_TAG_subprogram {
                    if let Some(A::Addr(pc)) = e.attr_value(g::DW_AT_low_pc)?
                        && pc != 0
                        && let Some(s) = site(d, &u, e, e)?
                    {
                        inventory.functions.insert(u32::try_from(pc)? & !1, s);
                    }
                    continue;
                }
                if e.tag() != g::DW_TAG_variable {
                    continue;
                }
                let named = if let Some(A::UnitRef(r)) = e.attr_value(g::DW_AT_specification)? {
                    u.entry(r)?
                } else {
                    e.clone()
                };
                let addr = match e.attr_value(g::DW_AT_location)? {
                    Some(A::Exprloc(expr)) => match expr.0.slice() {
                        [3, a, b, c, d] => {
                            Some(u32::from_le_bytes([*a, *b, *c, *d])).filter(|&a| a != 0)
                        }
                        [0x50..=0x91, ..] => continue,
                        _ => None,
                    },
                    _ => None,
                };
                if let Some(a) = addr
                    && let Some(s) = site(d, &u, e, &named)?
                {
                    inventory.decl.insert(a, s);
                }
                let Some(A::UnitRef(r)) = named.attr_value(g::DW_AT_type)? else {
                    continue;
                };
                let ty = u.entry(r)?;
                if ty.tag() != g::DW_TAG_structure_type {
                    continue;
                }
                let Some(attr) = ty.attr_value(g::DW_AT_name)? else {
                    continue;
                };
                let typename = d.attr_string(&u, attr)?.to_string()?;
                if !types.contains_key(typename) {
                    continue;
                }
                let Some(attr) = named.attr_value(g::DW_AT_name)? else {
                    continue;
                };
                let name = d.attr_string(&u, attr)?.to_string()?;
                let vs = vars.entry(typename.into()).or_default();
                let idx = vs.iter().position(|v| v.name == name).unwrap_or_else(|| {
                    vs.push(Instances {
                        name: name.into(),
                        ..Default::default()
                    });
                    vs.len() - 1
                });
                let v = &mut vs[idx];
                if let Some(a) = addr {
                    if !v.addresses.contains(&a) {
                        v.addresses.push(a);
                    }
                } else {
                    v.declared = true;
                }
            }
        }
        for (ty, vs) in vars {
            let objects = inventory.objects.entry(ty.clone()).or_default();
            for v in vs {
                let mut addresses = v.addresses;
                if addresses.is_empty()
                    && v.declared
                    && let Some(syms) = symbols.get(&v.name)
                {
                    for &(addr, size) in syms {
                        if size == types[ty.as_str()] && !addresses.contains(&addr) {
                            addresses.push(addr)
                        }
                    }
                }
                for &a in &addresses {
                    objects.push((
                        if addresses.len() > 1 {
                            format!("{}@0x{a:X}", v.name)
                        } else {
                            v.name.clone()
                        },
                        a,
                    ));
                }
            }
        }
        Ok(inventory)
    }
}
