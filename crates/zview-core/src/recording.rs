//! Record logical Memory calls, not transport fragments or decoded frames.
use crate::Selection;
use anyhow::{Context, Result, ensure};
use flate2::{Compression, write::GzEncoder};
use serde_json::{Value, json};
use std::{
    fs::File,
    io::{BufWriter, Write},
    path::Path,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

pub(crate) struct Trace {
    out: GzEncoder<BufWriter<File>>,
    start: Instant,
}
impl Trace {
    pub fn create(
        path: &Path,
        source: Value,
        selection: &Selection,
        features: &[String],
    ) -> Result<Self> {
        let f = File::create_new(path)
            .with_context(|| format!("create new recording {}", path.display()))?;
        let mut t = Self {
            out: GzEncoder::new(BufWriter::new(f), Compression::fast()),
            start: Instant::now(),
        };
        t.line(json!({
            "schema":"zview-recording/6", "zview":concat!("zview-rust/", env!("CARGO_PKG_VERSION")), "endianess":"<",
            "created_at":SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs_f64(),
            "features":features, "selection":selection,
            "source":source
        }))?;
        Ok(t)
    }
    fn line(&mut self, entry: Value) -> Result<()> {
        let line = serde_json::to_vec(&entry)?;
        ensure!(
            line.len() < crate::replay::MAX_LINE as usize,
            "recording entry exceeds replay line limit"
        );
        self.out.write_all(&line)?;
        self.out.write_all(b"\n")?;
        Ok(())
    }
    pub fn marker(&mut self, op: &str) -> Result<()> {
        self.line(json!({"t":self.start.elapsed().as_secs_f64(),"op":op,"args":{}}))
    }
    pub fn read<T>(
        &mut self,
        op: &str,
        at: u32,
        amount: u32,
        result: &Result<T>,
        encode: impl FnOnce(&T) -> Value,
    ) -> Result<()> {
        let mut e = json!({"t":self.start.elapsed().as_secs_f64(),"op":op,"args":{"at":at,"amount":amount}});
        match result {
            Ok(v) => e["result"] = encode(v),
            Err(err) => e["error"] = json!({"kind":"ProbeReadError","message":format!("{err:#}")}),
        }
        self.line(e)
    }
    pub fn finish(self) -> Result<()> {
        self.out.finish()?.flush()?;
        Ok(())
    }
}
