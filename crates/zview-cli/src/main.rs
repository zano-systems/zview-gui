mod build;
use anyhow::{Context, Result};
use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
use std::{
    io::{self, Write},
    net::SocketAddr,
    path::PathBuf,
    process::ExitCode,
    time::Duration,
};
use zview_core::{
    GROUPS, Selection,
    live::{self, Bound, Cancellation, Config},
};

#[derive(Parser)]
#[command(
    name = "zview",
    about = "Experimental Rust inspection of recordings and explicit live sources (including local GDB; Python reference: 0.28.1)"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Decode a complete recorded or live polling frame, as text or JSON.
    Dump(Dump),
    /// Capture bounded live reads to a new gzip NDJSON file.
    Record(Record),
}
#[derive(Args)]
struct Sampling {
    #[arg(short, long, required_unless_present = "build_dir")]
    elf_file: Option<PathBuf>,
    #[arg(short = 'd', long)]
    build_dir: Option<PathBuf>,
    #[arg(long)]
    domain: Option<String>,
    /// Minimum seconds between poll starts (0 for unpaced replay).
    #[arg(long,default_value="0.1",value_parser=parse_period)]
    period: Duration,
    /// Comma-separated case-sensitive row globs, optionally group-scoped.
    #[arg(long = "match", default_value = "*")]
    pattern: String,
}
#[derive(Args)]
struct Live {
    #[arg(short, long, value_enum, requires_if("gdb", "runner_target"))]
    runner: Option<Runner>,
    /// Numeric loopback address:port or localhost:port (GDB only).
    #[arg(short='t', long, value_parser=parse_local, requires="runner")]
    runner_target: Option<SocketAddr>,
    /// Exact built-in chip name; defaults to the build's J-Link device (probe-rs only).
    #[cfg(all(feature = "probe-rs", not(target_family = "wasm")))]
    #[arg(long, requires = "runner", conflicts_with = "runner_target")]
    chip: Option<String>,
    /// Zero-based core index; defaults to 0 (probe-rs only).
    #[cfg(all(feature = "probe-rs", not(target_family = "wasm")))]
    #[arg(long, requires = "runner", conflicts_with = "runner_target")]
    core: Option<usize>,
    /// Exact USB VID:PID:SERIAL; omit only for a single visible J-Link (probe-rs only).
    #[cfg(all(feature = "probe-rs", not(target_family = "wasm")))]
    #[arg(long, requires = "runner", conflicts_with = "runner_target")]
    probe: Option<live::ProbeSelector>,
    /// SWD frequency in Hz (probe-rs only); defaults to 400000.
    #[cfg(all(feature = "probe-rs", not(target_family = "wasm")))]
    #[arg(long, requires = "runner", conflicts_with = "runner_target")]
    frequency: Option<u32>,
}
#[derive(Clone, Copy, ValueEnum)]
enum Runner {
    Gdb,
    #[cfg(all(feature = "probe-rs", not(target_family = "wasm")))]
    ProbeRs,
}
impl Live {
    fn target(self, _sampling: &Sampling) -> Result<live::Target> {
        // Clap checks conflicts before conditional requirements. Even if a
        // conflicting option waived a requirement, never infer a transport.
        match (self.runner, self.runner_target) {
            (Some(Runner::Gdb), Some(addr)) => Ok(live::Target::Gdb(addr)),
            #[cfg(all(feature = "probe-rs", not(target_family = "wasm")))]
            (Some(Runner::ProbeRs), None) => Ok(live::Target::ProbeRs {
                chip: match self.chip {
                    Some(chip) => chip,
                    None => build::jlink_chip(
                        _sampling
                            .build_dir
                            .as_deref()
                            .context("probe-rs requires --chip or --build-dir")?,
                        _sampling.domain.as_deref(),
                    )?,
                },
                core: self.core.unwrap_or(0),
                probe: self.probe,
                frequency_hz: self.frequency.unwrap_or(live::DEFAULT_PROBE_FREQUENCY_HZ),
            }),
            _ => Cli::command()
                .error(
                    clap::error::ErrorKind::ArgumentConflict,
                    "supply only the connection options for the selected runner",
                )
                .exit(),
        }
    }
}
#[derive(Args)]
struct Dump {
    #[command(flatten)]
    sampling: Sampling,
    #[arg(
        short,
        long,
        required_unless_present = "runner",
        conflicts_with = "Live"
    )]
    input: Option<PathBuf>,
    #[command(flatten)]
    live: Live,
    /// 1-based valid frame; the second can show CPU shares.
    #[arg(long,default_value_t=2,value_parser=clap::value_parser!(u32).range(1..))]
    frame: u32,
    #[arg(long)]
    json: bool,
}
#[derive(Args)]
struct Record {
    #[command(flatten)]
    sampling: Sampling,
    #[command(flatten)]
    live: Live,
    #[arg(short, long, requires = "runner")]
    output: PathBuf,
    #[arg(long, required_unless_present="duration", conflicts_with="duration", value_parser=clap::value_parser!(u32).range(1..))]
    frames: Option<u32>,
    #[arg(long, value_parser=parse_period)]
    duration: Option<Duration>,
}
fn parse_local(s: &str) -> Result<SocketAddr, String> {
    let addr: SocketAddr = if let Some(p) = s.strip_prefix("localhost:") {
        format!("127.0.0.1:{p}").parse()
    } else {
        s.parse()
    }
    .map_err(|_| "use a numeric loopback address:port or localhost:port")?;
    if !addr.ip().is_loopback() || addr.port() == 0 {
        return Err("expected loopback address and nonzero port".into());
    }
    Ok(addr)
}
fn parse_period(s: &str) -> Result<Duration, String> {
    let secs = s.parse::<f64>().map_err(|e| e.to_string())?;
    Duration::try_from_secs_f64(secs).map_err(|e| e.to_string())
}
fn cancellation() -> Result<Cancellation> {
    use async_signal::{Signal, Signals};
    use futures_lite::{StreamExt, future::block_on};
    let mut signals = Signals::new([
        Signal::Int,
        #[cfg(unix)]
        Signal::Term,
    ])?;
    let token = Cancellation::default();
    let copy = token.clone();
    // CLI process owns this listener; library users install no signal handlers.
    std::thread::Builder::new()
        .name("live-stop".into())
        .spawn(move || {
            while block_on(signals.next()).is_some() {
                copy.cancel();
            }
        })?;
    Ok(token)
}
impl Sampling {
    fn firmware(&self) -> Result<PathBuf> {
        match &self.elf_file {
            Some(p) => Ok(p.clone()),
            None => build::elf(self.build_dir.as_deref().unwrap(), self.domain.as_deref()),
        }
    }
}
impl Dump {
    fn run(self) -> Result<()> {
        let selection = Selection::parse(&self.sampling.pattern)?;
        let elf = self.sampling.firmware()?;
        let period = self.sampling.period;
        let snapshot = if let Some(input) = self.input {
            zview_core::dump(&input, &elf, self.frame, period, &selection, |m| {
                eprintln!("{m}")
            })?
        } else {
            let mut last = None;
            live::run(
                Config {
                    elf: &elf,
                    target: self.live.target(&self.sampling)?,
                    period,
                    selection,
                    heap_maps: true,
                    output: None,
                },
                Bound::Frames(self.frame),
                &cancellation()?,
                |s| {
                    last = Some(s);
                    Ok(())
                },
                |m| eprintln!("{m}"),
            )?;
            last.context("no live frame")?
        };
        let mut out = io::BufWriter::new(io::stdout().lock());
        if self.json {
            serde_json::to_writer(&mut out, &snapshot)?;
            writeln!(out)?;
        } else {
            for group in GROUPS {
                for row in snapshot.get(group).into_iter().flatten() {
                    writeln!(out, "{}", row["line"].as_str().unwrap())?;
                }
            }
        }
        out.flush()?;
        Ok(())
    }
}
impl Record {
    fn run(self) -> Result<()> {
        let elf = self.sampling.firmware()?;
        let bound = self
            .frames
            .map(Bound::Frames)
            .or_else(|| self.duration.map(Bound::Duration))
            .unwrap();
        let n = live::run(
            Config {
                elf: &elf,
                target: self.live.target(&self.sampling)?,
                period: self.sampling.period,
                selection: Selection::parse(&self.sampling.pattern)?,
                heap_maps: true,
                output: Some(&self.output),
            },
            bound,
            &cancellation()?,
            |_| Ok(()),
            |m| eprintln!("{m}"),
        )?;
        writeln!(
            io::stdout().lock(),
            "Recorded {n} frames to {}",
            self.output.display()
        )?;
        Ok(())
    }
}
fn main() -> ExitCode {
    let result = match Cli::parse().command {
        Command::Dump(d) => d.run(),
        Command::Record(r) => r.run(),
    };
    if let Err(e) = result {
        eprintln!("zview: {e:#}");
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

#[cfg(all(test, feature = "probe-rs", not(target_family = "wasm")))]
mod tests {
    use super::*;

    #[test]
    fn probe_clock_uses_cli_override_not_vendor_build_speed() {
        let dir = tempfile::tempdir().unwrap();
        let zephyr = dir.path().join("zephyr");
        std::fs::create_dir(&zephyr).unwrap();
        let runners = zephyr.join("runners.yaml");
        let frequency = |flags: &[&str]| {
            let mut args = vec!["zview", "dump", "-r", "probe-rs", "-d"];
            args.push(dir.path().to_str().unwrap());
            args.extend(flags);
            let Command::Dump(dump) = Cli::try_parse_from(args).unwrap().command else {
                unreachable!()
            };
            let live::Target::ProbeRs { frequency_hz, .. } =
                dump.live.target(&dump.sampling).unwrap()
            else {
                unreachable!()
            };
            frequency_hz
        };
        std::fs::write(
            &runners,
            "args:\n  jlink: [--device=nRF52833_xxAA, --speed, '4000']\n",
        )
        .unwrap();
        assert_eq!(frequency(&[]), 400_000);
        assert_eq!(frequency(&["--frequency", "1000000"]), 1_000_000);
    }
}
