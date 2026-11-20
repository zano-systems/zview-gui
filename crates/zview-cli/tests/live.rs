//! A loopback, all-stop stub. It only knows the verified entry span, the
//! monitored thread list and the selected heap's bytes from the first real
//! benchmark frame. An unselected poll or an unexpected command fails the test.
use flate2::read::GzDecoder;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Command, Output},
    thread::{self, JoinHandle},
    time::Duration,
};
use zview_core::{
    Selection,
    live::{self, Bound, Cancellation, Config},
};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../test-data/zview-bench-60s")
        .join(name)
}
fn entries(p: &Path) -> impl Iterator<Item = Value> {
    BufReader::new(GzDecoder::new(File::open(p).unwrap()))
        .lines()
        .map(|l| serde_json::from_str(&l.unwrap()).unwrap())
}
const ENTRY: u32 = 0xd88;
const HEAP: u32 = 536871232;
fn image() -> BTreeMap<u32, u8> {
    let mut image = BTreeMap::new();
    // This ELF's first PT_LOAD starts at file offset 0xf8, vaddr 0.
    let elf = fs::read(fixture("zephyr.elf")).unwrap();
    let off = 0xf8 + ENTRY as usize;
    image.extend((ENTRY..).zip(elf[off..off + 64].iter().copied()));
    for (i, e) in entries(&fixture("capture.ndjson.gz")).take(104).enumerate() {
        if !(3..22).contains(&i) && !(97..104).contains(&i) {
            continue;
        }
        let at = e["args"]["at"].as_u64().unwrap() as u32;
        let bytes: Vec<u8> = match e["op"].as_str().unwrap() {
            "read32" => e["result"]
                .as_array()
                .unwrap()
                .iter()
                .flat_map(|v| (v.as_u64().unwrap() as u32).to_le_bytes())
                .collect(),
            "read_bytes" => e["result"]
                .as_str()
                .unwrap()
                .as_bytes()
                .as_chunks::<2>()
                .0
                .iter()
                .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
                .collect(),
            other => panic!("unexpected fixture op {other}"),
        };
        image.extend((at..).zip(bytes));
    }
    image
}
enum Behavior {
    Normal,
    WrongImage,
    ReadError,
    Malformed,
    WithholdReply,
    RejectDetach,
    Cancel(Cancellation),
}

fn byte(sock: &mut TcpStream) -> u8 {
    let mut b = [0];
    sock.read_exact(&mut b).unwrap();
    b[0]
}
fn sum(s: &[u8]) -> u8 {
    s.iter().copied().fold(0, u8::wrapping_add)
}
fn reply(sock: &mut TcpStream, s: &str) {
    write!(sock, "${s}#{:02x}", sum(s.as_bytes())).unwrap();
    assert_eq!(byte(sock), b'+');
}
fn stub(behavior: Behavior) -> (SocketAddr, JoinHandle<usize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    (
        addr,
        thread::spawn(move || {
            let image = image();
            let (mut sock, _) = listener.accept().unwrap();
            sock.set_nodelay(true).unwrap();
            sock.set_read_timeout(Some(Duration::from_secs(6))).unwrap();
            let mut running = false;
            let mut detached = false;
            let mut resumes = 0;
            let mut first = true;
            let mut tag = [0];
            while sock.read(&mut tag).unwrap() != 0 {
                if tag[0] == 3 {
                    assert!(running);
                    running = false;
                    reply(&mut sock, "S02");
                    continue;
                }
                assert_eq!(tag[0], b'$');
                let mut p = vec![];
                loop {
                    let b = byte(&mut sock);
                    if b == b'#' {
                        break;
                    }
                    p.push(b);
                }
                let checksum = [byte(&mut sock), byte(&mut sock)];
                assert_eq!(format!("{:02x}", sum(&p)).as_bytes(), checksum);
                let op = std::str::from_utf8(&p).unwrap();
                assert!(!running, "command {op} sent before stopping");
                if first {
                    assert_eq!(op, "?");
                    sock.write_all(b"-").unwrap();
                    first = false;
                    continue;
                }
                sock.write_all(b"+").unwrap();
                match op {
                    "?" => {
                        sock.write_all(b"$S05#00").unwrap();
                        assert_eq!(byte(&mut sock), b'-'); // corrupted reply is not consumed
                        reply(&mut sock, "S05");
                    }
                    "c" => {
                        running = true;
                        resumes += 1;
                    }
                    "D" => {
                        detached = true;
                        if matches!(behavior, Behavior::RejectDetach) {
                            reply(&mut sock, "");
                        } else {
                            running = true;
                            reply(&mut sock, "OK");
                        }
                    }
                    _ => {
                        let (at, n) = op.strip_prefix('m').unwrap().split_once(',').unwrap();
                        let at = u32::from_str_radix(at, 16).unwrap();
                        let n = u32::from_str_radix(n, 16).unwrap();
                        assert!((1..=256).contains(&n));
                        if at == HEAP {
                            match &behavior {
                                Behavior::ReadError => {
                                    reply(&mut sock, "E01");
                                    continue;
                                }
                                Behavior::Malformed => {
                                    reply(&mut sock, "0z");
                                    continue;
                                }
                                Behavior::WithholdReply => continue, // wait for cleanup, no sleeps
                                Behavior::Cancel(token) => token.cancel(),
                                _ => {}
                            }
                        }
                        // Deliberately short replies exercise continuation addresses.
                        let mut data = String::new();
                        for a in at..at + n.min(113) {
                            use std::fmt::Write;
                            let b = image[&a]
                                ^ u8::from(a == ENTRY && matches!(behavior, Behavior::WrongImage));
                            write!(data, "{b:02x}").unwrap();
                        }
                        reply(&mut sock, &data);
                    }
                }
            }
            assert!(
                detached && running,
                "client left the stub stopped or attached"
            );
            resumes
        }),
    )
}

fn cli() -> Command {
    Command::new(env!("CARGO_BIN_EXE_zview"))
}
fn source(cmd: &str, addr: SocketAddr) -> Command {
    let mut c = cli();
    c.args([cmd, "-e"]).arg(fixture("zephyr.elf")).args([
        "-r",
        "gdb",
        "-t",
        &addr.to_string(),
        "--match",
        "heaps:bench_heap",
    ]);
    c
}
fn ok(o: Output) -> Vec<u8> {
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    o.stdout
}
#[test]
fn selected_live_record_and_offline_dump_agree() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("capture.gz");
    let (addr, server) = stub(Behavior::Normal);
    let o = source("record", addr)
        .args(["--frames", "2", "--period", "0.03", "-o"])
        .arg(&file)
        .output()
        .unwrap();
    assert!(
        String::from_utf8(ok(o))
            .unwrap()
            .starts_with("Recorded 2 frames to ")
    );
    assert_eq!(server.join().unwrap(), 3); // discovery and two complete polls
    let trace: Vec<_> = entries(&file).collect();
    assert_eq!(trace[0]["schema"], "zview-recording/6");
    assert_eq!(
        trace[0]["source"],
        json!({"period":0.03,"runner":"gdb","frequency":null,"runner_target":addr.to_string()})
    );
    assert_eq!(
        trace[0]["selection"]["include"],
        json!([["heaps", "bench_heap"]])
    );
    let polls: Vec<_> = trace.iter().filter(|e| e["op"] == "begin_batch").collect();
    assert_eq!(polls.len(), 3);
    assert!(polls[2]["t"].as_f64().unwrap() - polls[1]["t"].as_f64().unwrap() >= 0.028);
    assert_eq!(trace.last().unwrap()["op"], "disconnect");

    let (addr, server) = stub(Behavior::Normal);
    let live = ok(source("dump", addr)
        .args(["--json", "--period", "0"])
        .output()
        .unwrap());
    assert_eq!(server.join().unwrap(), 3);
    let replay = ok(cli()
        .args(["dump", "-e"])
        .arg(fixture("zephyr.elf"))
        .arg("-i")
        .arg(&file)
        .args(["--json", "--period", "0"])
        .output()
        .unwrap());
    assert_eq!(live, replay);
    let f: Value = serde_json::from_slice(&replay).unwrap();
    assert_eq!(f.as_object().unwrap().len(), 1);
    assert_eq!(f["heaps"][0]["allocated_bytes"], 528);
    assert!(!f["heaps"][0]["chunks"].as_array().unwrap().is_empty());
}
#[test]
fn duration_does_not_wait_for_next_period() {
    let dir = tempfile::tempdir().unwrap();
    let (addr, server) = stub(Behavior::Normal);
    let o = source("record", addr)
        .args(["--duration", "0.05", "--period", "60", "-o"])
        .arg(dir.path().join("out.gz"))
        .output()
        .unwrap();
    assert!(
        String::from_utf8(ok(o))
            .unwrap()
            .starts_with("Recorded 1 frames to ")
    );
    assert_eq!(server.join().unwrap(), 2);
}
#[test]
fn startup_read_timeout_and_detach_errors_do_not_publish() {
    for (b, diagnostic) in [
        (Behavior::WrongImage, "does not match ELF"),
        (Behavior::ReadError, "GDB read error"),
        (Behavior::Malformed, "invalid RSP hex"),
        (Behavior::WithholdReply, "timed out"),
        (Behavior::RejectDetach, "confirm GDB detach"),
    ] {
        let (addr, server) = stub(b);
        let o = source("dump", addr)
            .args(["--frame", "1", "--json"])
            .output()
            .unwrap();
        assert_eq!(o.status.code(), Some(1));
        assert!(o.stdout.is_empty());
        let err = String::from_utf8_lossy(&o.stderr);
        assert!(err.contains(diagnostic), "{err}");
        server.join().unwrap();
    }
}
#[test]
fn cancel_while_stopped_finishes_current_frame_and_detaches() {
    let token = Cancellation::default();
    let (addr, server) = stub(Behavior::Cancel(token.clone()));
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("cancelled.gz");
    let mut delivered = 0;
    let err = live::run(
        Config {
            elf: &fixture("zephyr.elf"),
            target: live::Target::Gdb(addr),
            selection: Selection::parse("heaps:bench_heap").unwrap(),
            period: Duration::from_secs(600),
            heap_maps: true,
            output: Some(&file),
        },
        Bound::Frames(10),
        &token,
        |_| {
            delivered += 1;
            Ok(())
        },
        |_| {},
    )
    .unwrap_err();
    assert!(err.to_string().contains("cancelled"));
    assert_eq!(delivered, 1);
    assert_eq!(server.join().unwrap(), 2);
    assert_eq!(entries(&file).last().unwrap()["op"], "disconnect");
}

#[test]
fn cancel_only_bound_returns_after_detach_with_maps_disabled() {
    let token = Cancellation::default();
    let (addr, server) = stub(Behavior::Cancel(token.clone()));
    let delivered = live::run(
        Config {
            elf: &fixture("zephyr.elf"),
            target: live::Target::Gdb(addr),
            selection: Selection::parse("heaps:bench_heap").unwrap(),
            period: Duration::from_secs(600),
            heap_maps: false,
            output: None,
        },
        Bound::UntilCancelled,
        &token,
        |snapshot| {
            assert_eq!(snapshot["heaps"][0]["allocated_bytes"], 528);
            assert!(snapshot["heaps"][0]["chunks"].is_null());
            Ok(())
        },
        |_| {},
    )
    .unwrap();
    assert_eq!(delivered, 1);
    assert_eq!(server.join().unwrap(), 2);
}

#[test]
fn record_writes_read_error_and_closes_stream() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("read-error.gz");
    let (addr, server) = stub(Behavior::ReadError);
    let o = source("record", addr)
        .args(["--frames", "1", "-o"])
        .arg(&file)
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(1));
    server.join().unwrap();
    let es: Vec<_> = entries(&file).collect();
    let last = &es[es.len() - 3..];
    assert_eq!(last[0]["op"], "read32");
    assert_eq!(last[0]["error"]["kind"], "ProbeReadError");
    assert!(last[0].get("result").is_none());
    assert_eq!(last[1]["op"], "end_batch");
    assert_eq!(last[2]["op"], "disconnect");
}
