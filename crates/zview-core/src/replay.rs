//! Streaming strict cursor. Keep the pending entry until the request matches;
//! even a repeated address is a separate observation.
use std::io::{BufRead, Read};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};

use crate::{
    memory::Memory,
    selection::{GROUPS, Selection},
};

pub(crate) const MAX_LINE: u64 = 1024 * 1024;

fn line<R: BufRead, T: DeserializeOwned>(reader: &mut R) -> Result<T> {
    let mut buf = Vec::new();
    let n = reader.take(MAX_LINE + 1).read_until(b'\n', &mut buf)?;
    ensure!(n != 0, "unexpected end of recording");
    ensure!(n as u64 <= MAX_LINE, "NDJSON line exceeds 1 MiB");
    Ok(serde_json::from_slice(&buf)?)
}

#[derive(Deserialize)]
struct Header {
    schema: String,
    #[serde(default = "little_endian")]
    endianess: String,
    features: Option<Vec<String>>,
    selection: Option<Selection>,
}

fn little_endian() -> String {
    "<".into()
}

#[derive(Deserialize)]
struct Entry {
    op: String,
    args: Value,
    result: Option<Value>,
    error: Option<RecordedError>,
}

#[derive(Deserialize)]
struct RecordedError {
    kind: String,
    message: String,
}

pub(crate) struct Replay<R> {
    reader: R,
    pending: Option<Entry>,
    pub(crate) index: usize,
    pub(crate) features: Vec<String>,
    pub(crate) selection: Selection,
}

impl<R: BufRead> Replay<R> {
    pub(crate) fn new(mut reader: R) -> Result<Self> {
        let h: Header = line(&mut reader).context("recording header")?;
        ensure!(
            matches!(
                h.schema.as_str(),
                "zview-recording/2"
                    | "zview-recording/3"
                    | "zview-recording/4"
                    | "zview-recording/6"
            ) && h.endianess == "<",
            "unsupported recording schema or byte order: {} {}",
            h.schema,
            h.endianess
        );
        let selection = h.selection.unwrap_or_default();
        selection.validate()?;
        let features = h
            .features
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| vec!["threads".into(), "heaps".into()]);
        for f in &features {
            ensure!(
                GROUPS.contains(&f.as_str())
                    || matches!(
                        f.as_str(),
                        "merged_thread_meta"
                            | "heap_waiters"
                            | "usage_snapshot"
                            | "unfilled_stacks"
                            | "stack_sentinel"
                            | "distinct_names"
                            | "thread_usage_alone"
                            | "single_usage_pass"
                    ),
                "unsupported recording feature {f}"
            );
        }
        Ok(Self {
            reader,
            pending: None,
            index: 0,
            features,
            selection,
        })
    }

    fn peek(&mut self, op: &str, args: &Value) -> Result<&Entry> {
        if self.pending.is_none() {
            self.pending = Some(line(&mut self.reader).with_context(|| {
                format!("operation index {} (requested {op} {args})", self.index)
            })?);
        }
        Ok(self.pending.as_ref().unwrap())
    }

    /// Disconnect is clean only *between* batches, with no trailing entries.
    pub(crate) fn finished(&mut self) -> Result<bool> {
        if self.peek("begin_batch", &json!({}))?.op != "disconnect" {
            return Ok(false);
        }
        self.marker("disconnect")?;
        ensure!(
            self.reader.fill_buf()?.is_empty(),
            "trailing data after disconnect"
        );
        Ok(true)
    }

    fn next(&mut self, op: &str, args: Value) -> Result<Option<Value>> {
        let index = self.index;
        let entry = self.peek(op, &args)?;
        ensure!(
            entry.op == op && entry.args == args,
            "replay mismatch at index {}: expected {} {}, actual {op} {args}",
            index,
            entry.op,
            entry.args
        );
        let entry = self.pending.take().unwrap();
        let index = self.index;
        self.index += 1;
        if let Some(error) = entry.error {
            return Err(crate::ReadFault(format!(
                "recorded {op} failure at index {index}: {}: {}",
                error.kind, error.message
            ))
            .into());
        }
        Ok(entry.result)
    }

    fn marker(&mut self, op: &str) -> Result<()> {
        let index = self.index;
        ensure!(
            self.next(op, json!({}))?.is_none(),
            "unexpected {op} result at index {index}"
        );
        Ok(())
    }

    fn numbers<T: DeserializeOwned>(&mut self, op: &str, at: u32, amount: u32) -> Result<Vec<T>> {
        let index = self.index;
        let value = self
            .next(op, json!({"at": at, "amount": amount}))?
            .with_context(|| format!("missing {op} result at index {index}"))?;
        let data: Vec<T> = serde_json::from_value(value)
            .with_context(|| format!("invalid {op} result at index {index}"))?;
        ensure!(
            data.len() == amount as usize,
            "{op} result at index {index}: expected {amount} elements, got {}",
            data.len()
        );
        Ok(data)
    }
}

impl<R: BufRead> Memory for Replay<R> {
    fn connect(&mut self) -> Result<()> {
        self.marker("connect")
    }
    fn begin_batch(&mut self) -> Result<()> {
        self.marker("begin_batch")
    }
    fn end_batch(&mut self) -> Result<()> {
        self.marker("end_batch")
    }
    fn read8(&mut self, at: u32, amount: u32) -> Result<Vec<u8>> {
        self.numbers("read8", at, amount)
    }
    fn read32(&mut self, at: u32, amount: u32) -> Result<Vec<u32>> {
        self.numbers("read32", at, amount)
    }
    fn read64(&mut self, at: u32) -> Result<u64> {
        Ok(self.numbers("read64", at, 1)?[0])
    }
    fn optional_byte(&mut self, at: u32) -> Result<Option<u8>> {
        let args = json!({"at":at,"amount":1});
        let e = self.peek("read8", &args)?;
        if e.op != "read8" || e.args != args {
            return Ok(None);
        }
        Ok(Some(self.read8(at, 1)?[0]))
    }
    fn optional_word(&mut self, at: u32) -> Result<Option<u32>> {
        let args = json!({"at":at,"amount":1});
        let e = self.peek("read32", &args)?;
        if e.op != "read32" || e.args != args {
            return Ok(None);
        }
        self.word(at).map(Some)
    }
    fn read_bytes(&mut self, at: u32, amount: u32) -> Result<Vec<u8>> {
        let index = self.index;
        let v = self
            .next("read_bytes", json!({"at":at,"amount":amount}))?
            .context("missing byte result")?;
        let s = v.as_str().context("expected hexadecimal byte result")?;
        ensure!(
            s.len() as u64 == 2 * u64::from(amount) && s.bytes().all(|b| b.is_ascii_hexdigit()),
            "invalid hexadecimal byte result at index {index}"
        );
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).context("invalid hexadecimal byte"))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    const HEADER: &str = r#"{"schema":"zview-recording/4","endianess":"<","features":["threads","merged_thread_meta"]}"#;

    fn cursor(entries: &str) -> Replay<Cursor<String>> {
        Replay::new(Cursor::new(format!("{HEADER}\n{entries}"))).unwrap()
    }

    #[test]
    fn mismatch_preserves_next_entry_even_for_repeated_addresses() {
        let mut r = cursor(
            r#"{"op":"connect","args":{}}
{"op":"read32","args":{"at":42,"amount":1},"result":[0]}
{"op":"read32","args":{"at":42,"amount":1},"result":[17]}"#,
        );
        r.connect().unwrap();
        for error in [
            r.read8(42, 1).unwrap_err(),
            r.read32(43, 1).unwrap_err(),
            r.read32(42, 2).unwrap_err(),
        ] {
            let msg = error.to_string();
            assert!(
                msg.contains("index 1: expected read32") && msg.contains("actual read"),
                "{msg}"
            );
        }
        assert_eq!(r.index, 1);
        assert_eq!(r.word(42).unwrap(), 0);
        assert_eq!(r.word(42).unwrap(), 17);
        assert!(format!("{:#}", r.end_batch().unwrap_err()).contains("unexpected end"));
    }

    #[test]
    fn markers_require_empty_args() {
        let mut r = cursor(r#"{"op":"connect","args":{"extra":0}}"#);
        assert!(
            r.connect()
                .unwrap_err()
                .to_string()
                .contains("actual connect {}")
        );
        assert_eq!(r.index, 0);
    }

    #[test]
    fn malformed_required_data_is_not_zero_or_empty() {
        for (entry, expected) in [
            ("{", "operation index 0"),
            (r#"{"op":"read8","result":[0]}"#, "missing field `args`"),
            (
                r#"{"op":"read8","args":{"at":0,"amount":1}}"#,
                "missing read8 result",
            ),
            (
                r#"{"op":"read8","args":{"at":0,"amount":1},"result":[]}"#,
                "expected 1 elements",
            ),
            (
                r#"{"op":"read8","args":{"at":0,"amount":1},"result":[256]}"#,
                "invalid read8 result",
            ),
        ] {
            let msg = format!("{:#}", cursor(entry).read8(0, 1).unwrap_err());
            assert!(msg.contains(expected), "{msg}");
        }
    }

    #[test]
    fn recorded_failure_matches_then_advances() {
        let mut r = cursor(
            r#"{"op":"read64","args":{"at":1,"amount":1},"error":{"kind":"ProbeReadFailure","message":"test failure"}}"#,
        );
        let msg = r.read64(1).unwrap_err().to_string();
        assert!(msg.contains("index 0") && msg.contains("ProbeReadFailure: test failure"));
        assert_eq!(r.index, 1);
    }

    #[test]
    fn unsupported_header_is_rejected_before_reads() {
        for h in [
            HEADER.replace("/4", "/5"),
            HEADER.replace('<', ">"),
            HEADER.replace("merged_thread_meta", "unknown"),
            "{}".into(),
            "".into(),
        ] {
            assert!(Replay::new(h.as_bytes()).is_err(), "{h}");
        }
    }
}
