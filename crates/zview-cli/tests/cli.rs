use flate2::{Compression, read::GzDecoder, write::GzEncoder};
use serde_json::{Value, json};
use std::{
    fs::{self, File},
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Command, Output},
};
use tempfile::NamedTempFile;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../test-data")
        .join(name)
}
fn bench() -> PathBuf {
    fixture("zview-bench-60s")
}
fn recording() -> PathBuf {
    bench().join("capture.ndjson.gz")
}
fn zview(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_zview"))
        .args(args)
        .output()
        .unwrap()
}
fn dump(cap: &Path, elf: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_zview"))
        .args(["dump", "--period", "0", "-i"])
        .arg(cap)
        .arg("-e")
        .arg(elf)
        .args(args)
        .output()
        .unwrap()
}
fn benchmark(args: &[&str]) -> Output {
    dump(&recording(), &bench().join("zephyr.elf"), args)
}
fn success(o: Output) -> Vec<u8> {
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    o.stdout
}
fn snapshot(o: Output) -> Value {
    let bytes = success(o);
    assert!(bytes.ends_with(b"\n"));
    serde_json::from_slice(&bytes).unwrap()
}
fn failed(o: Output, code: i32, message: &str) {
    assert_eq!(o.status.code(), Some(code));
    assert!(o.stdout.is_empty());
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains(message), "{err}");
}

#[test]
fn usage_and_runtime_errors() {
    for args in [
        &[][..],
        &["live"],
        &["replay"],
        &["dump", "-e", "elf"],
        &["dump", "-i", "file"],
        &["dump", "-i", "file", "-e", "elf", "-r", "gdb"],
    ] {
        failed(zview(args), 2, "Usage:");
    }
    failed(benchmark(&["--frame", "0"]), 2, "--frame");
    failed(zview(&["record", "-o", "unused"]), 2, "Usage:");
    failed(
        zview(&["dump", "-i", "nonexistent", "-e", "elf"]),
        1,
        "open",
    );
    failed(benchmark(&["--match", "unknown:*"]), 1, "unknown group");
    failed(
        dump(
            &recording(),
            &fixture("tests/fixtures/zephyr.elf"),
            &["--json"],
        ),
        1,
        "zview:",
    );
    let help = String::from_utf8(success(zview(&["--help"]))).unwrap();
    assert!(help.contains(zview_core::REFERENCE_VERSION) && help.contains("local GDB"));
}

#[test]
fn live_requires_one_local_source_and_one_record_bound() {
    for args in [
        vec!["dump", "-e", "elf", "-r", "gdb"],
        vec!["dump", "-e", "elf", "-t", "127.0.0.1:1"],
        vec!["dump", "-e", "elf", "-r", "gdb", "-t", "192.0.2.1:1234"],
        vec!["dump", "-e", "elf", "-r", "pyocd", "-t", "127.0.0.1:1"],
        vec![
            "dump",
            "-e",
            "elf",
            "-r",
            "gdb",
            "-t",
            "localhost:1",
            "--frequency",
            "10M",
        ],
    ] {
        failed(zview(&args), 2, "error:");
    }
    for bound in [
        &[][..],
        &["--frames", "0"],
        &["--frames", "1", "--duration", "1"],
        &["--duration", "NaN"],
    ] {
        let mut args = vec![
            "record",
            "-e",
            "elf",
            "-r",
            "gdb",
            "-t",
            "localhost:1",
            "-o",
            "unused",
        ];
        args.extend_from_slice(bound);
        failed(zview(&args), 2, "error:");
    }
}

#[test]
fn existing_recording_is_never_overwritten() {
    // The smaller ELF also exercises live layout discovery without k_sem.
    // The output is checked before any attempt to connect.
    let tmp = NamedTempFile::new().unwrap();
    fs::write(tmp.path(), b"preserve this file").unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_zview"))
        .args(["record", "-e"])
        .arg(fixture("tests/fixtures/zephyr.elf"))
        .args(["-r", "gdb", "-t", "127.0.0.1:1", "--frames", "1", "-o"])
        .arg(tmp.path())
        .output()
        .unwrap();
    failed(o, 1, "create new recording");
    assert_eq!(fs::read(tmp.path()).unwrap(), b"preserve this file");
}

// Even a parser regression must not let these tests reach a device. Use an
// absent ELF in a private empty directory, never a connected probe or stub.
#[test]
fn probe_runner_is_feature_gated_and_requires_an_exclusive_source() {
    let dir = tempfile::tempdir().unwrap();
    let run = |cmd: &str, flags: &[&str]| {
        let mut c = Command::new(env!("CARGO_BIN_EXE_zview"));
        c.current_dir(dir.path()).args([cmd, "-e", "missing.elf"]);
        if cmd == "record" {
            c.args(["-o", "capture.gz"]);
        }
        c.args(flags).output().unwrap()
    };
    let connection = [
        "-r",
        "probe-rs",
        "--chip",
        "nRF52833_xxAA",
        "--core",
        "0",
        "--probe",
        "1234:5678:mock",
    ];
    if !cfg!(feature = "probe-rs") {
        failed(run("dump", &connection), 2, "error:");
        return;
    }
    failed(run("dump", &connection), 1, "missing.elf");
    failed(
        run("dump", &["-r", "probe-rs"]),
        1,
        "probe-rs requires --chip or --build-dir",
    );
    for pair in [4, 6] {
        let mut incomplete = connection.to_vec();
        incomplete.drain(pair..pair + 2);
        failed(run("dump", &incomplete), 1, "missing.elf");
    }
    for flags in [&["-i", "recording.gz"][..], &["-t", "localhost:1"]] {
        failed(
            run("dump", &[connection.as_slice(), flags].concat()),
            2,
            "error:",
        );
    }
    failed(run("dump", &["-r", "gdb", "--core", "0"]), 2, "error:");
    failed(run("record", &connection), 2, "error:"); // must be bounded
    for bound in [&["--frames", "1"][..], &["--duration", "1"]] {
        failed(
            run("record", &[connection.as_slice(), bound].concat()),
            1,
            "missing.elf",
        );
    }
    assert!(!dir.path().join("capture.gz").exists());
}

#[cfg(feature = "probe-rs")]
#[test]
fn probe_rs_uses_jlink_device_and_elf_from_build_for_dump_and_record() {
    let dir = tempfile::tempdir().unwrap();
    let zephyr = dir.path().join("zephyr");
    std::fs::create_dir(&zephyr).unwrap();
    let config = zephyr.join("runners.yaml");
    let capture = dir.path().join("capture.gz");
    let run = |command: &str, options: &[&str]| {
        let mut cli = Command::new(env!("CARGO_BIN_EXE_zview"));
        cli.args([command, "--runner", "probe-rs", "--build-dir"])
            .arg(dir.path());
        if command == "record" {
            cli.args(["--frames", "1", "--output"]).arg(&capture);
        } else {
            cli.arg("--json");
        }
        cli.args(options).output().unwrap()
    };
    let build = "debug-runner: jlink\nconfig:\n  elf_file: missing-app.elf\n";
    std::fs::write(
        &config,
        format!("{build}args:\n  jlink: [--dt-flash=y, --speed=4000, --device=nRF52833_xxAA]\n"),
    )
    .unwrap();
    for command in ["dump", "record"] {
        // ELF load must fail before any hardware discovery or attach.
        failed(run(command, &[]), 1, "missing-app.elf");
    }
    std::fs::write(&config, build).unwrap();
    failed(run("dump", &[]), 1, "no J-Link --device");
    failed(
        run("dump", &["--chip", "nRF52833_xxAA"]),
        1,
        "missing-app.elf",
    );
    assert!(!capture.exists());
}

#[test]
fn complete_first_frame_includes_every_group() {
    let frame = snapshot(benchmark(&["--frame", "1", "--json"]));
    assert_eq!(frame.as_object().unwrap().len(), 7);
    let threads = frame["threads"].as_array().unwrap();
    assert_eq!(threads.len(), 18);
    assert_eq!(threads[0]["entry_symbol"], "work_queue_main");
    assert_eq!(threads[0]["entry_site"][1], 671);
    assert_eq!(
        threads[0]["runtime"],
        json!({"cpu":0.0,"cpu_normalized":0.0,"is_active":false,"stack_watermark":224,"stack_watermark_percent":29.166666666666668,"is_cpu_coherent":null})
    );
    assert_eq!(threads[15]["priority"], -1);
    assert_eq!(threads[16]["state"], 0);
    assert_eq!(frame["heaps"][0]["allocated_bytes"], 528);
    assert_eq!(
        frame["heaps"][0]["chunks"][0],
        json!({"used":true,"size":16})
    );
    assert_eq!(
        frame["semaphores"][1]["waiters"],
        json!(["gate_waiter_b_id", "gate_waiter_a_id"])
    );
    assert_eq!(frame["mutexes"][0]["owner_name"], "lock_owner_id");
    assert_eq!(frame["msgqs"][0]["msg_size"], 4);
    assert_eq!(frame["mem_slabs"][0]["max_used"], 8);
    assert_eq!(
        frame["workqs"][0]["pending"],
        json!(["trail_work (work_handler)"])
    );
    assert_eq!(frame["workqs"][1]["waiters"], json!([]));
}

#[test]
fn default_second_frame_filter_keeps_cycle_denominators_and_text_order() {
    let pattern = "threads:frag_?d, mutexes:*, semaphores:bench_[gs]*, semaphores:!*gate";
    let frame = snapshot(benchmark(&["--match", pattern, "--json"]));
    assert_eq!(frame.as_object().unwrap().len(), 3);
    assert_eq!(frame["threads"][0]["runtime"]["cpu"], 100.0);
    assert!(
        (frame["threads"][0]["runtime"]["cpu_normalized"]
            .as_f64()
            .unwrap()
            - 0.02352890847254604)
            .abs()
            < 1e-14
    );
    let lines = String::from_utf8(success(benchmark(&["--match", pattern]))).unwrap();
    assert_eq!(
        lines,
        ["threads", "semaphores", "mutexes"]
            .map(|g| format!("{}\n", frame[g][0]["line"].as_str().unwrap()))
            .concat()
    );
    let none = benchmark(&["--match", "heaps:absent", "--json"]);
    assert!(String::from_utf8_lossy(&none.stderr).contains("matches no row"));
    assert_eq!(snapshot(none), json!({"heaps":[]}));
}

#[test]
fn final_frame_then_clean_disconnect() {
    let frame = snapshot(benchmark(&["--frame", "578", "--json"]));
    assert_eq!(frame["heaps"][0]["allocated_bytes"], 1188);
    failed(
        benchmark(&["--frame", "579", "--json"]),
        1,
        "only 578 frames",
    );
}

#[test]
fn legacy_recording_with_different_firmware_and_no_optional_reads() {
    let dir = fixture("tests/fixtures");
    let run = |args: &[&str]| {
        dump(
            &dir.join("recordings/sys_heap_v4.3.ndjson.gz"),
            &dir.join("zephyr.elf"),
            args,
        )
    };
    let frame = snapshot(run(&["--json"]));
    assert_eq!(frame.as_object().unwrap().len(), 2);
    assert_eq!(frame["threads"].as_array().unwrap().len(), 3);
    assert_eq!(frame["threads"][0]["name"], "stress_id");
    assert_eq!(frame["threads"][0]["runtime"]["cpu"], 100.0);
    for f in ["entry_point", "priority", "state", "user_options"] {
        assert_eq!(frame["threads"][0].get(f), Some(&Value::Null));
    }
    assert_eq!(frame["heaps"][0]["free_bytes"], 512);
    for f in ["chunks", "waiters"] {
        assert_eq!(frame["heaps"][0].get(f), Some(&Value::Null));
    }
    failed(run(&["--frame", "21"]), 1, "only 20 frames");
}

// Tests mutate copies of the first complete batch, never the tracked fixture.
fn prefix(lines: usize) -> Vec<Value> {
    BufReader::new(GzDecoder::new(File::open(recording()).unwrap()))
        .lines()
        .take(lines)
        .map(|line| serde_json::from_str(&line.unwrap()).unwrap())
        .collect()
}
fn capture(entries: &[Value]) -> NamedTempFile {
    let f = NamedTempFile::new().unwrap();
    let mut gz = GzEncoder::new(f.as_file(), Compression::fast());
    for e in entries {
        writeln!(gz, "{e}").unwrap();
    }
    gz.finish().unwrap();
    f
}
#[test]
fn even_filtered_output_requires_final_workq_and_matching_end_batch() {
    let run = |entries: &[Value]| {
        dump(
            capture(entries).path(),
            &bench().join("zephyr.elf"),
            &["--frame", "1", "--match", "threads:idle", "--json"],
        )
    };
    let mut lines = prefix(117); // header + operations 0..=115
    assert_eq!(lines.last().unwrap()["op"], "end_batch");
    failed(run(&lines[..116]), 1, "unexpected end");
    lines[116]["op"] = json!("disconnect");
    failed(run(&lines), 1, "actual end_batch");
    lines[115]["args"]["amount"] = json!(7);
    failed(run(&lines), 1, "mismatch at index 114");
}
#[test]
fn truncated_stream_after_valid_batch_is_not_a_clean_end() {
    let mut lines = prefix(117);
    let run = |es: &[Value]| {
        dump(
            capture(es).path(),
            &bench().join("zephyr.elf"),
            &["--frame", "2"],
        )
    };
    failed(run(&lines), 1, "unexpected end");
    lines.push(json!({"op":"disconnect","args":{}}));
    failed(run(&lines), 1, "only 1 frames");
    lines.push(json!({"op":"begin_batch","args":{}}));
    failed(run(&lines), 1, "trailing data after disconnect");
}

#[test]
fn recorded_read_fault_is_not_a_frame_or_a_cursor_mismatch() {
    let first = prefix(117);
    let mut fault = first[23..25].to_vec(); // begin_batch, kernel usage
    fault[1].as_object_mut().unwrap().remove("result");
    fault[1]["error"] = json!({"kind":"ProbeReadTimeout","message":"test timeout"});
    fault.push(first[116].clone());
    let mut lines = first[..23].to_vec();
    lines.extend_from_slice(&fault);
    lines.extend_from_slice(&first[23..]);
    let run = |es: &[Value]| {
        dump(
            capture(es).path(),
            &bench().join("zephyr.elf"),
            &["--frame", "1", "--json"],
        )
    };
    let o = run(&lines);
    assert!(String::from_utf8_lossy(&o.stderr).contains("test timeout"));
    assert_eq!(snapshot(o)["heaps"][0]["allocated_bytes"], 528);
    lines.truncate(23);
    for _ in 0..3 {
        lines.extend_from_slice(&fault);
    }
    failed(run(&lines), 1, "three successive recorded read faults");
}

#[test]
fn regression_resets_baselines_and_does_not_count_partial_poll() {
    let first = prefix(117);
    let mut lines = first.clone();
    lines.extend_from_slice(&first[23..26]); // through first thread's cycle counter
    lines.last_mut().unwrap()["result"] = json!([0]);
    lines.push(first[116].clone());
    // Watermarks start afresh; cached entry pointers survive the reset.
    lines.extend_from_slice(&first[23..25]);
    for thread in first[25..97].as_chunks::<4>().0 {
        lines.extend_from_slice(&thread[..3]);
    }
    lines.extend_from_slice(&first[97..]);
    let o = dump(
        capture(&lines).path(),
        &bench().join("zephyr.elf"),
        &["--frame", "2", "--json"],
    );
    assert!(String::from_utf8_lossy(&o.stderr).contains("resetting"));
    let frame = snapshot(o);
    assert_eq!(frame["threads"][0]["runtime"]["cpu_normalized"], 0.0);
    assert_eq!(
        frame["workqs"][0]["pending"][0],
        "trail_work (work_handler)"
    );
}

#[test]
fn synthetic_recorded_selection_counts_every_discovered_thread_in_usage_snapshot() {
    // Same real ELF and discovery, but a /6 selection records only the idle
    // thread's metadata, no stack fill, and a stamped pass over ALL counters.
    let original = prefix(191);
    let mut entries = original[..23].to_vec();
    entries[0] = json!({"schema":"zview-recording/6","endianess":"<",
        "features":["threads","usage_snapshot","single_usage_pass","merged_thread_meta","unfilled_stacks"],
        "selection":{"include":[["threads","idle"]]}});
    let stamp = |v| json!({"op":"read32","args":{"at":536874948,"amount":1},"result":[v]});
    for (i, reads) in [&original[25..97], &original[119..173]]
        .into_iter()
        .enumerate()
    {
        entries.push(json!({"op":"begin_batch","args":{}}));
        entries.push(stamp(12));
        entries.extend(reads.iter().filter(|r| r["op"] == "read64").cloned());
        entries.push(stamp(12 + i));
        entries.push(original[91].clone()); // idle metadata
        if i == 0 {
            entries.push(original[92].clone());
        } // cached entry
        entries.push(json!({"op":"end_batch","args":{}}));
    }
    let f = capture(&entries);
    let run = |args: &[&str]| snapshot(dump(f.path(), &bench().join("zephyr.elf"), args));
    let first = run(&["--frame", "1", "--json"]);
    assert_eq!(first["threads"][0]["runtime"]["is_cpu_coherent"], true);
    let frame = run(&["--json"]);
    assert_eq!(frame["threads"].as_array().unwrap().len(), 1);
    let r = &frame["threads"][0]["runtime"];
    assert_eq!(r["is_cpu_coherent"], false);
    assert_eq!(r["cpu"], 0.0);
    assert!(
        r["cpu_normalized"].as_f64().unwrap() > 99.9
            && r["cpu_normalized"].as_f64().unwrap() < 100.0
    );
    assert_eq!(r.get("stack_watermark"), Some(&Value::Null));
    assert_eq!(
        run(&["--json", "--match", "threads:frag_id"]),
        json!({"threads":[]})
    );
}

#[test]
fn build_dir_rebases_domain_and_uses_runners_elf() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("image/zephyr");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        temp.path().join("domains.yaml"),
        "default: app\nbuild_dir: /old\ndomains:\n  - name: app\n    build_dir: /old/image\n",
    )
    .unwrap();
    fs::write(
        dir.join("runners.yaml"),
        "config:\n  elf_file: renamed.elf\n",
    )
    .unwrap();
    fs::copy(bench().join("zephyr.elf"), dir.join("renamed.elf")).unwrap();
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_zview"))
            .args(["dump", "--period", "0", "-i"])
            .arg(recording())
            .arg("-d")
            .arg(temp.path())
            .args(args)
            .output()
            .unwrap()
    };
    assert_eq!(
        snapshot(run(&["--json", "--match", "heaps:*"]))["heaps"][0]["allocated_bytes"],
        552
    );
    failed(run(&["--domain", "missing"]), 1, "unknown build domain");
}
