//! Narrow ACK-mode, all-stop RSP transport. Only the acquisition layer can
//! request memory; neither callers nor target data supply debugger commands.
use crate::live::Transport;
use anyhow::{Context, Result, bail, ensure};
use std::{
    io::{BufRead, BufReader, Write},
    net::{SocketAddr, TcpStream},
    time::{Duration, Instant},
};

pub(crate) const IO_TIMEOUT: Duration = Duration::from_secs(2);
const CHUNK: usize = 256;

pub(crate) struct Rsp {
    io: BufReader<TcpStream>,
    running: bool,
    closed: bool,
    deadline: Instant,
}

fn checksum(payload: &[u8]) -> u8 {
    payload.iter().fold(0u8, |a, b| a.wrapping_add(*b))
}

fn hex(pair: &[u8]) -> Result<u8> {
    ensure!(
        pair.len() == 2 && pair.iter().all(u8::is_ascii_hexdigit),
        "invalid RSP hex"
    );
    Ok(u8::from_str_radix(std::str::from_utf8(pair)?, 16)?)
}

impl Rsp {
    pub(crate) fn open(addr: SocketAddr) -> Result<Self> {
        ensure!(addr.ip().is_loopback(), "GDB requires a loopback address");
        let sock = TcpStream::connect_timeout(&addr, IO_TIMEOUT)
            .with_context(|| format!("connect to local GDB stub {addr}"))?;
        let mut s = Self {
            io: BufReader::new(sock),
            running: false,
            closed: false,
            deadline: Instant::now() + IO_TIMEOUT,
        };
        let result = (|| {
            s.io.get_ref().set_write_timeout(Some(IO_TIMEOUT))?;
            s.io.get_ref().set_nodelay(true)?;
            s.send("?")?;
            s.stop_reply()
        })();
        if let Err(e) = result {
            return Err(with_cleanup(e, s.close()));
        }
        Ok(s)
    }

    fn byte(&mut self) -> Result<u8> {
        if self.io.buffer().is_empty() {
            let left = self.deadline.saturating_duration_since(Instant::now());
            ensure!(!left.is_zero(), "RSP reply timed out");
            self.io.get_ref().set_read_timeout(Some(left))?;
        }
        let buffer = match self.io.fill_buf() {
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ) =>
            {
                return Err(e).context("timed out waiting for RSP reply");
            }
            r => r.context("waiting for RSP reply")?,
        };
        let b = *buffer.first().context("GDB stub closed connection")?;
        self.io.consume(1);
        Ok(b)
    }

    fn send(&mut self, payload: &str) -> Result<()> {
        let wire = format!("${payload}#{:02x}", checksum(payload.as_bytes()));
        self.deadline = Instant::now() + IO_TIMEOUT;
        for _ in 0..2 {
            self.io.get_mut().write_all(wire.as_bytes())?;
            match self.byte()? {
                b'+' => return Ok(()),
                b'-' => continue,
                _ => bail!("expected RSP acknowledgement"),
            }
        }
        bail!("GDB stub rejected packet checksum")
    }

    fn reply(&mut self) -> Result<Vec<u8>> {
        for _ in 0..2 {
            ensure!(self.byte()? == b'$', "expected RSP packet");
            let mut p = Vec::new();
            loop {
                let b = self.byte()?;
                if b == b'#' {
                    break;
                }
                ensure!(p.len() < 4096, "oversized RSP packet");
                p.push(b);
            }
            if hex(&[self.byte()?, self.byte()?])? == checksum(&p) {
                self.io.get_mut().write_all(b"+")?;
                return Ok(p);
            }
            self.io.get_mut().write_all(b"-")?;
        }
        bail!("invalid RSP reply checksum")
    }

    fn stop_reply(&mut self) -> Result<()> {
        let r = self.reply()?;
        ensure!(
            matches!(r.as_slice(), [b'S', _, _] | [b'T', _, _, ..]) && hex(&r[1..3]).is_ok(),
            "expected target stop reply"
        );
        Ok(())
    }
}

impl Transport for Rsp {
    fn halt(&mut self) -> Result<()> {
        if self.running {
            self.deadline = Instant::now() + IO_TIMEOUT;
            self.io.get_mut().write_all(&[3])?;
            self.stop_reply()?;
            self.running = false;
        }
        Ok(())
    }

    fn resume(&mut self) -> Result<()> {
        // If an ACK gets lost, cleanup must account for a pending continue.
        self.running = true;
        self.send("c")
    }

    fn memory(&mut self, at: u32, len: u32, batch_deadline: Instant) -> Result<Vec<u8>> {
        ensure!(!self.running, "read outside stopped batch");
        let mut data = Vec::new();
        while data.len() < len as usize {
            ensure!(Instant::now() < batch_deadline, "GDB batch timed out");
            let addr = at + data.len() as u32;
            let n = CHUNK.min(len as usize - data.len());
            self.send(&format!("m{addr:x},{n:x}"))?;
            let p = self
                .reply()
                .with_context(|| format!("read {n} bytes at {addr:#x}"))?;
            if p.len() == 3 && p[0] == b'E' && hex(&p[1..]).is_ok() {
                return Err(crate::ReadFault(format!(
                    "GDB read error at {addr:#x}: {}",
                    String::from_utf8_lossy(&p)
                ))
                .into());
            }
            ensure!(
                !p.is_empty() && p.len() <= n * 2 && p.len().is_multiple_of(2),
                "invalid GDB read length at {addr:#x}"
            );
            for pair in p.as_chunks::<2>().0 {
                data.push(hex(pair)?);
            }
        }
        Ok(data)
    }

    fn close(&mut self) -> Result<()> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        // A detach resumes the inferior. Try it even when the interrupt or
        // its reply failed. Never report success on an unconfirmed detach.
        let halt = self.halt();
        let detach = (|| {
            self.send("D")?;
            ensure!(self.reply()? == b"OK", "GDB stub rejected detach");
            Ok(())
        })();
        if let Err(e) = detach {
            // Some stubs do not implement detach. On a broken link this can
            // only be best effort; the caller must see the cleanup failure.
            return Err(with_cleanup(e, self.resume()).context("could not confirm GDB detach"));
        }
        halt
    }
}

pub(crate) fn with_cleanup(primary: anyhow::Error, cleanup: Result<()>) -> anyhow::Error {
    match cleanup {
        Ok(()) => primary,
        Err(e) => primary.context(format!("cleanup also failed: {e:#}")),
    }
}

impl Drop for Rsp {
    fn drop(&mut self) {
        let _ = self.close();
    }
}
