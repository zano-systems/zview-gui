use crate::{firmware::address, memory::Memory};
use anyhow::{Result, bail, ensure};
use serde_json::{Value, json};

/// Old recordings didn't declare map capture. Probe only its first size read;
/// once present, every subsequent byte and size read must match.
pub(crate) fn chunks(
    mem: &mut impl Memory,
    ptr: u32,
    end_offset: u32,
    captured: &mut Option<bool>,
) -> Result<Option<Vec<Value>>> {
    if *captured == Some(false) {
        return Ok(None);
    }
    if ptr == 0 {
        return Ok(Some(vec![]));
    }
    let at = address(ptr, end_offset)?;
    let end = if captured.is_some() {
        mem.word(at)?
    } else {
        let value = mem.optional_word(at)?;
        *captured = Some(value.is_some());
        let Some(end) = value else { return Ok(None) };
        end
    };
    if end == 0 {
        return Ok(Some(vec![]));
    }
    ensure!(end <= 32 * 1024 * 1024 / 8, "heap exceeds 32 MiB");
    let raw = mem.read_bytes(ptr, end * 8)?;
    let header = |chunk: u32, width: usize| {
        let i = chunk as usize * 8 + width;
        if width == 2 {
            u16::from_le_bytes([raw[i], raw[i + 1]]).into()
        } else {
            u32::from_le_bytes(raw[i..i + 4].try_into().unwrap())
        }
    };
    let Some((width, mut chunk)) = [2, 4].into_iter().find_map(|w| {
        let h = header(0, w);
        (h & 1 == 1 && (1..end).contains(&(h >> 1))).then_some((w, h >> 1))
    }) else {
        bail!("invalid chunk zero signature")
    };
    let mut chunks = Vec::new();
    while chunk < end {
        let h = header(chunk, width);
        let size = h >> 1;
        ensure!(
            (1..=end - chunk).contains(&size),
            "invalid size for heap chunk {chunk}"
        );
        chunks.push(json!({"used":h&1!=0,"size":size*8}));
        chunk += size;
    }
    Ok(Some(chunks))
}
