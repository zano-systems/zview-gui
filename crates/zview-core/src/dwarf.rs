//! Resolve member paths from complete DWARF struct definitions. References
//! stay in their own compilation unit (no joining types by similar names).
use anyhow::{Context, Result, bail, ensure};
use gimli::{self as g, AttributeValue as Attr, EndianSlice, LittleEndian, UnitOffset};

pub(crate) type Reader<'a> = EndianSlice<'a, LittleEndian>;
type Die<'a, 'u, 'b> = g::DebuggingInformationEntry<'a, 'u, Reader<'b>>;

/// An optional kernel group may not be built in. Keep absence distinct from
/// corrupt DWARF or an unsupported field representation.
#[derive(Debug)]
pub(crate) struct MissingLayout(String);
impl std::fmt::Display for MissingLayout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for MissingLayout {}

pub(crate) struct Types<'a, 'b> {
    dwarf: &'a g::Dwarf<Reader<'b>>,
    unit: &'a g::Unit<Reader<'b>>,
}

#[derive(Copy, Clone)]
pub(crate) struct Field {
    pub(crate) offset: u32,
    pub(crate) size: u32,
}

fn target(entry: &Die<'_, '_, '_>) -> Result<UnitOffset> {
    match entry.attr_value(g::DW_AT_type)? {
        Some(Attr::UnitRef(offset)) => Ok(offset),
        _ => bail!("expected a unit-local DW_AT_type"),
    }
}

impl<'b> Types<'_, 'b> {
    fn name(&self, entry: &Die<'_, '_, 'b>) -> Result<Option<&'b str>> {
        entry
            .attr_value(g::DW_AT_name)?
            .map(|a| {
                Ok(std::str::from_utf8(
                    self.dwarf.attr_string(self.unit, a)?.slice(),
                )?)
            })
            .transpose()
    }

    fn unalias(&self, mut ty: UnitOffset) -> Result<UnitOffset> {
        for _ in 0..32 {
            let entry = self.unit.entry(ty)?;
            if !matches!(
                entry.tag(),
                g::DW_TAG_typedef | g::DW_TAG_const_type | g::DW_TAG_volatile_type
            ) {
                return Ok(ty);
            }
            ty = target(&entry)?;
        }
        bail!("too many DWARF type indirections")
    }

    pub(crate) fn size(&self, ty: UnitOffset) -> Result<u32> {
        let entry = self.unit.entry(self.unalias(ty)?)?;
        if let Some(n) = entry
            .attr_value(g::DW_AT_byte_size)?
            .and_then(|a| a.udata_value())
        {
            return Ok(n.try_into()?);
        }
        if entry.tag() == g::DW_TAG_pointer_type {
            return Ok(self.unit.encoding().address_size.into());
        }
        // GCC describes fixed arrays (including the thread name) with
        // subrange children rather than DW_AT_byte_size.
        ensure!(
            entry.tag() == g::DW_TAG_array_type,
            "missing DW_AT_byte_size"
        );
        let element = self.unit.entry(self.unalias(target(&entry)?)?)?;
        let mut size: u32 = element
            .attr_value(g::DW_AT_byte_size)?
            .and_then(|a| a.udata_value())
            .context("unsupported array element type")?
            .try_into()?;
        let mut tree = self.unit.entries_tree(Some(entry.offset()))?;
        let mut dims = tree.root()?.children();
        let mut found = false;
        while let Some(dim) = dims.next()? {
            let d = dim.entry();
            ensure!(
                d.tag() == g::DW_TAG_subrange_type && d.attr_value(g::DW_AT_lower_bound)?.is_none(),
                "unsupported DWARF array bounds"
            );
            let end: u32 = d
                .attr_value(g::DW_AT_upper_bound)?
                .and_then(|a| a.udata_value())
                .context("expected fixed DWARF array bound")?
                .try_into()?;
            size = size
                .checked_mul(end.checked_add(1).context("array bound overflow")?)
                .context("array size overflow")?;
            found = true;
        }
        ensure!(found, "array without bounds");
        Ok(size)
    }

    fn member(
        &self,
        ty: UnitOffset,
        name: &str,
        depth: usize,
    ) -> Result<Option<(u32, UnitOffset)>> {
        ensure!(depth < 32, "too many anonymous member indirections");
        let ty = self.unalias(ty)?;
        let mut tree = self.unit.entries_tree(Some(ty))?;
        let root = tree.root()?;
        let union = root.entry().tag() == g::DW_TAG_union_type;
        let mut members = root.children();
        while let Some(member) = members.next()? {
            let entry = member.entry();
            if entry.tag() != g::DW_TAG_member {
                continue;
            }
            let child = match self.name(entry)? {
                Some(n) if n == name => Some((0, target(entry)?)),
                None => self.member(target(entry)?, name, depth + 1)?,
                _ => None,
            };
            if let Some((nested, field_ty)) = child {
                let offset: u32 = match entry.attr_value(g::DW_AT_data_member_location)? {
                    None if union => 0,
                    Some(a) => a
                        .udata_value()
                        .context("nonconstant DWARF member offset")?
                        .try_into()?,
                    None => bail!("missing DWARF member offset"),
                };
                return Ok(Some((
                    offset
                        .checked_add(nested)
                        .context("member offset overflow")?,
                    field_ty,
                )));
            }
        }
        Ok(None)
    }

    pub(crate) fn field(&self, root: UnitOffset, path: &str) -> Result<Field> {
        Ok(self
            .optional(root, path)?
            .ok_or_else(|| MissingLayout(format!("missing DWARF member {path}")))?)
    }

    pub(crate) fn optional(&self, root: UnitOffset, path: &str) -> Result<Option<Field>> {
        let mut ty = root;
        let mut offset = 0u32;
        for name in path.split('.') {
            let Some((inner, next)) = self.member(ty, name, 0)? else {
                return Ok(None);
            };
            offset = offset.checked_add(inner).context("member path overflow")?;
            ty = next;
        }
        let size = self.size(ty)?;
        let root_size = self.size(root)?;
        ensure!(
            offset.checked_add(size).is_some_and(|end| end <= root_size),
            "field {path} outside its struct"
        );
        Ok(Some(Field { offset, size }))
    }
}

/// Skip forward declarations; resolve each path within one definition.
pub(crate) fn with_struct<T>(
    dwarf: &g::Dwarf<Reader<'_>>,
    name: &str,
    resolve: impl FnOnce(&Types<'_, '_>, UnitOffset) -> Result<T>,
) -> Result<T> {
    let mut units = dwarf.units();
    while let Some(header) = units.next()? {
        let unit = dwarf.unit(header)?;
        let types = Types { dwarf, unit: &unit };
        let mut entries = unit.entries();
        while let Some((_, e)) = entries.next_dfs()? {
            if e.tag() == g::DW_TAG_structure_type
                && e.has_children()
                && types.name(e)? == Some(name)
            {
                return resolve(&types, e.offset()).with_context(|| format!("DWARF struct {name}"));
            }
        }
    }
    Err(MissingLayout(format!("missing DWARF definition of {name}")).into())
}
