//! In-process native backend. The worker owns the core cursor/connection;
//! the window owns delivery, sequencing and admission of the next session.
use super::*;
use crate::adapter_protocol::{
    Groups, LiveConfiguration, PROTOCOL_VERSION, Session, Source, StatusUpdate,
};
use anyhow::{Context as _, Result, ensure};
use serde::Deserialize;
use serde_json::Value;
use smol::channel::{Receiver, Sender};
use std::{
    net::SocketAddr,
    path::Path,
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use zview_core::{
    ReplayPoll, ReplaySession, Snapshot,
    live::{self, Cancellation},
};

const PERIOD: Duration = Duration::from_millis(100);
const QUEUE_DEPTH: usize = 4;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplayRequest {
    elf: String,
    recording: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LiveRequest {
    elf: String,
    target: String,
}

fn loopback(target: &str) -> Result<SocketAddr> {
    let target: SocketAddr = if let Some(port) = target.strip_prefix("localhost:") {
        format!("127.0.0.1:{port}").parse()
    } else {
        target.parse()
    }
    .context("Enter a numeric loopback address:port or localhost:port")?;
    ensure!(
        target.ip().is_loopback() && target.port() != 0,
        "GDB Live requires a loopback address and a nonzero port"
    );
    Ok(target)
}

enum Input {
    Replay { recording: String },
    Live { target: SocketAddr },
}

struct Acquisition {
    elf: String,
    input: Input,
}

impl Acquisition {
    fn parse(path: &str, body: Value) -> Result<Self> {
        let (elf, input) = if path == "/api/session/replay" {
            let ReplayRequest { elf, recording } = serde_json::from_value(body)?;
            ensure!(!recording.trim().is_empty(), "Enter a recording path");
            (elf, Input::Replay { recording })
        } else {
            let LiveRequest { elf, target } = serde_json::from_value(body)?;
            (
                elf,
                Input::Live {
                    target: loopback(&target)?,
                },
            )
        };
        ensure!(!elf.trim().is_empty(), "Enter a matching ELF path");
        Ok(Self { elf, input })
    }

    fn session(&self, id: String) -> Session {
        let source = match &self.input {
            Input::Replay { recording } => Source::Replay {
                recording: recording.clone(),
            },
            Input::Live { target } => Source::Live {
                configuration: LiveConfiguration {
                    build_dir: String::new(),
                    domain: None,
                    elf: Some(self.elf.clone()),
                    runner: Some("gdb".into()),
                    target: Some(target.to_string()),
                    frequency: None,
                },
                runner: Some("gdb".into()),
                target: Some(target.to_string()),
                frequency: None,
            },
        };
        Session {
            id,
            source: Box::new(source),
            elf: self.elf.clone(),
            status: Status::Opening,
            frames_received: 0,
            last_frame: None,
            last_notice: None,
            read_warning: false,
            error: None,
            context: None,
            samples_per_second: None,
            capabilities: None,
            heap_detail: Default::default(),
            timing: None,
            inventory: None,
            live_view: None,
            usage_window: None,
        }
    }
}

fn frame_event(snapshot: Snapshot, live_elapsed: Option<f64>) -> Result<Event> {
    let sampled_groups = snapshot.keys().copied().map(str::to_owned).collect();
    let mut groups = serde_json::to_value(snapshot)?;
    // The core dump represents an entry location as [file, line]; the shared
    // GUI model uses optional entry_file/entry_line. All measurements, in
    // particular nulls and omitted groups, must retain their observed meaning.
    if let Some(threads) = groups.get_mut("threads").and_then(Value::as_array_mut) {
        for thread in threads {
            if let Some(site) = thread.get("entry_site").filter(|v| !v.is_null()) {
                let (file, line): (String, u64) = serde_json::from_value(site.clone())?;
                thread["entry_file"] = Value::String(file);
                thread["entry_line"] = Value::from(line);
            }
        }
    }
    let groups: Groups = serde_json::from_value(groups)?;
    Ok(Event::Frame(crate::adapter_protocol::Frame {
        decoded_at: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs_f64(),
        live_elapsed,
        view_revision: 0,
        view: None,
        sampled_groups,
        groups,
    }))
}

fn status(status: Status, error: Option<String>) -> Event {
    Event::Status(StatusUpdate { status, error })
}

fn regular_file(path: &Path) -> Result<()> {
    let meta = path
        .metadata()
        .with_context(|| format!("Open {}", path.display()))?;
    ensure!(meta.is_file(), "{} is not a regular file", path.display());
    Ok(())
}

fn acquire(acquisition: Acquisition, cancel: Cancellation, events: Sender<Event>) -> Result<()> {
    if cancel.is_cancelled() {
        return Ok(());
    }
    let elf = Path::new(&acquisition.elf);
    regular_file(elf)?;
    let send = |event| {
        // Bound queued memory without dropping observations. For Live this
        // runs only after the target resumes; Stop closes/wakes the sender.
        if !cancel.is_cancelled() {
            let _ = events.send_blocking(event);
        }
    };
    match acquisition.input {
        Input::Replay { recording } => {
            let recording = Path::new(&recording);
            regular_file(recording)?;
            let mut replay = ReplaySession::open(recording, elf)?;
            send(status(Status::Reading, None));
            while !cancel.is_cancelled() {
                let started = Instant::now();
                match replay.poll()? {
                    ReplayPoll::Frame(frame) => send(frame_event(frame, None)?),
                    ReplayPoll::Notice(message) => send(Event::Notice { message }),
                    ReplayPoll::End => break,
                }
                // Pace decoding, not the time at which data was recorded.
                thread::park_timeout(PERIOD.saturating_sub(started.elapsed()));
            }
        }
        Input::Live { target } => {
            let started = Instant::now();
            let mut previous = None;
            live::run(
                live::Config {
                    elf,
                    target: live::Target::Gdb(target),
                    period: PERIOD,
                    selection: Default::default(),
                    heap_maps: false,
                    output: None,
                },
                live::Bound::UntilCancelled,
                &cancel,
                |frame| {
                    if cancel.is_cancelled() {
                        return Ok(());
                    }
                    let elapsed = started.elapsed().as_secs_f64();
                    let event = frame_event(frame, Some(elapsed))?;
                    let rate = if let Some(last) = previous.replace(elapsed) {
                        let delta: f64 = elapsed - last;
                        (delta > 0.).then(|| 1. / delta)
                    } else {
                        send(status(Status::Reading, None));
                        None
                    };
                    send(event);
                    if rate.is_some() {
                        send(Event::Health {
                            samples_per_second: rate,
                        });
                    }
                    Ok(())
                },
                |message| {
                    send(Event::Notice {
                        message: message.to_owned(),
                    })
                },
            )?;
        }
    }
    Ok(())
}

struct Worker {
    id: String,
    sequence: u64,
    events: Receiver<Event>,
    cancel: Cancellation,
    join: Option<JoinHandle<Result<()>>>,
    delivery_error: Option<String>,
}

impl Worker {
    fn start(id: String, acquisition: Acquisition) -> Result<Self> {
        let cancel = Cancellation::default();
        let signal = cancel.clone();
        let (send, events) = smol::channel::bounded(QUEUE_DEPTH);
        let join = thread::Builder::new()
            .name("zview-core-acquisition".into())
            .spawn(move || acquire(acquisition, signal, send))?;
        Ok(Self {
            id,
            sequence: 0,
            events,
            cancel,
            join: Some(join),
            delivery_error: None,
        })
    }

    fn envelope(&mut self, event: Event) -> Envelope {
        self.sequence += 1;
        Envelope {
            version: PROTOCOL_VERSION,
            session_id: Some(self.id.clone()),
            sequence: self.sequence,
            event,
        }
    }

    fn stop(&self) {
        self.cancel.cancel();
        self.events.close();
        if let Some(join) = &self.join {
            join.thread().unpark();
        }
    }

    fn ready_to_join(&self) -> bool {
        self.join.as_ref().is_none_or(|join| join.is_finished())
    }

    fn join(&mut self) -> Result<()> {
        match self.join.take() {
            Some(handle) => handle
                .join()
                .map_err(|_| anyhow::anyhow!("Rust acquisition worker panicked"))?,
            None => Ok(()),
        }
    }

    /// No queued frame may cross Stop, and no terminal status may precede
    /// joining the worker (including live detach). Sequence only what we send.
    fn next(&mut self) -> Option<(Envelope, bool)> {
        let stopped = self.cancel.is_cancelled();
        if !stopped && let Ok(event) = self.events.try_recv() {
            return Some((self.envelope(event), false));
        }
        if !self.ready_to_join() || (!stopped && !self.events.is_empty()) {
            return None;
        }
        let result = self.join();
        let error = match (self.delivery_error.take(), result) {
            (Some(delivery), Err(cleanup)) => {
                Some(format!("{delivery}; acquisition ended with: {cleanup:#}"))
            }
            (None, Err(error)) => Some(format!("{error:#}")),
            (error, Ok(())) => error,
        };
        let status_code = if error.is_some() {
            Status::Failed
        } else if stopped {
            Status::Stopped
        } else {
            Status::Ended
        };
        Some((self.envelope(status(status_code, error)), true))
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        // Normally Stop/Quit joins after is_finished. This synchronous path
        // covers entity destruction or OS quit that bypasses the Quit action.
        self.stop();
        if let Err(error) = self.join() {
            eprintln!("Rust acquisition shutdown failed: {error:#}");
        }
    }
}

#[derive(Default)]
pub(crate) struct Backend {
    worker: Option<Worker>,
}

impl Backend {
    pub(crate) fn stop(&self) {
        if let Some(worker) = &self.worker {
            worker.stop();
        }
    }

    pub(crate) fn finish_shutdown(&mut self) -> bool {
        if self
            .worker
            .as_ref()
            .is_some_and(|worker| !worker.ready_to_join())
        {
            return false;
        }
        self.worker.take();
        true
    }
}

impl ThreadsApp {
    pub(crate) fn start_rust_core(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.adapter.available = true;
        self.adapter.rust_core = Some(Backend::default());
        self.adapter.draft.use_local_paths(window, cx);
        self.accept_adapter(
            Envelope {
                version: PROTOCOL_VERSION,
                session_id: None,
                sequence: 0,
                event: Event::Snapshot(None),
            },
            window,
            cx,
        )
        .expect("empty Rust session snapshot");
    }

    pub(super) fn rust_command(
        &mut self,
        path: &str,
        body: Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if self.adapter.closing {
            return Err("The application is closing.".into());
        }
        if matches!(path, "/api/session/stop" | "/api/session/reconnect")
            && !self
                .adapter
                .state
                .session
                .as_ref()
                .is_some_and(|s| body["session_id"].as_str() == Some(&s.id))
        {
            return Err("This command belongs to a different session.".into());
        }
        match path {
            "/api/session/stop" => {
                let worker = self
                    .adapter
                    .rust_core
                    .as_mut()
                    .unwrap()
                    .worker
                    .as_mut()
                    .filter(|_| self.adapter.state.can_stop())
                    .ok_or("No active Rust session to stop.")?;
                worker.stop();
                let event = worker.envelope(status(Status::Stopping, None));
                self.accept_adapter(event, window, cx)
            }
            "/api/session/reconnect" => {
                if !self.adapter.state.can_reconnect() {
                    return Err(
                        "Wait for the current session to finish before reconnecting.".into(),
                    );
                }
                let session = self.adapter.state.session.as_ref().unwrap();
                let Source::Live { target, .. } = session.source.as_ref() else {
                    return Err("Only Live sessions can reconnect.".into());
                };
                let input = Acquisition::parse(
                    "/api/session/live",
                    json!({"elf": session.elf, "target": target}),
                )
                .map_err(|e| format!("{e:#}"))?;
                self.open_rust_session(input, window, cx)
            }
            "/api/session/live" | "/api/session/replay" => {
                let input = Acquisition::parse(path, body).map_err(|e| format!("{e:#}"))?;
                self.open_rust_session(input, window, cx)
            }
            _ => Err("This operation is not supported by the experimental Rust backend.".into()),
        }
    }

    fn open_rust_session(
        &mut self,
        input: Acquisition,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let backend = self.adapter.rust_core.as_mut().unwrap();
        if backend.worker.is_some() || !self.adapter.state.can_open() {
            return Err(
                "Stop the current session and wait for cleanup before opening another.".into(),
            );
        }
        let id = uuid::Uuid::new_v4().to_string();
        let session = input.session(id.clone());
        let worker = Worker::start(id.clone(), input)
            .map_err(|e| format!("Cannot start Rust acquisition: {e:#}"))?;
        backend.worker = Some(worker);
        self.accept_adapter(
            Envelope {
                version: PROTOCOL_VERSION,
                session_id: Some(id.clone()),
                sequence: 0,
                event: Event::Snapshot(Some(Box::new(session))),
            },
            window,
            cx,
        )?;
        self.adapter.connection_task = Some(cx.spawn(async move |this, cx| {
            let mut freshness_tick = Instant::now();
            loop {
                let active = this.update_in(cx, |app, window, cx| {
                    if freshness_tick.elapsed() >= Duration::from_secs(1) {
                        cx.notify();
                        freshness_tick = Instant::now();
                    }
                    app.pump_rust_session(&id, window, cx)
                });
                if !matches!(active, Ok(true)) {
                    return;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(25))
                    .await;
            }
        }));
        Ok(())
    }

    fn pump_rust_session(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        // Bound the work per UI tick even if the producer immediately refills.
        for _ in 0..QUEUE_DEPTH {
            let Some(worker) = self
                .adapter
                .rust_core
                .as_mut()
                .and_then(|b| b.worker.as_mut())
                .filter(|worker| worker.id == id)
            else {
                return false;
            };
            let Some((event, terminal)) = worker.next() else {
                break;
            };
            if terminal {
                self.adapter.rust_core.as_mut().unwrap().worker.take();
            }
            if let Err(error) = self.accept_adapter(event, window, cx) {
                if let Some(worker) = self.adapter.rust_core.as_mut().unwrap().worker.as_mut() {
                    worker.delivery_error = Some(error);
                    worker.stop();
                } else {
                    self.error = Some(error);
                }
            }
            if terminal {
                return false;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::{Acquisition, Worker, frame_event, loopback, status};
    use crate::adapter_protocol::{Connection, Event, Status};
    use crate::model;
    use gpui::{TestAppContext, VisualTestContext};
    use serde_json::json;
    use std::{
        thread,
        time::{Duration, Instant},
    };
    use zview_core::{Snapshot, live::Cancellation};

    #[test]
    fn endpoint_and_supported_arguments_are_explicit() {
        assert_eq!(
            loopback("localhost:1234").unwrap().to_string(),
            "127.0.0.1:1234"
        );
        assert!(loopback("[::1]:1234").is_ok());
        for bad in [
            "localhost:0",
            "192.0.2.2:1234",
            "remote.example:1234",
            "::1:1234",
        ] {
            assert!(loopback(bad).is_err(), "{bad}");
        }
        for unsupported in [
            json!({"elf": "a.elf", "target": "localhost:1234", "frequency": 1}),
            json!({"elf": "a.elf", "target": "localhost:1234", "runner": "jlink"}),
            json!({"elf": "a.elf", "target": "localhost:1234", "build_dir": "build"}),
        ] {
            assert!(Acquisition::parse("/api/session/live", unsupported).is_err());
        }
    }

    #[test]
    fn maps_core_snapshot_into_the_shared_model_without_filling_missing_values() {
        let snapshot = Snapshot::from([
            (
                "threads",
                vec![json!({
                    "name": "demo", "address": 256, "entry_site": ["main.c", 42], "is_idle": false,
                    "runtime": {"cpu": null, "cpu_normalized": null, "is_active": false}
                })],
            ),
            (
                "heaps",
                vec![json!({
                    "name": "heap", "address": 512, "allocated_bytes": 0, "free_bytes": 128,
                    "max_allocated_bytes": null, "chunks": null, "waiters": null
                })],
            ),
        ]);
        let Event::Frame(frame) = frame_event(snapshot, Some(1.)).unwrap() else {
            panic!()
        };
        assert_eq!(frame.sampled_groups, ["heaps", "threads"]);
        assert_eq!(frame.live_elapsed, Some(1.));
        assert_eq!(frame.view, None);
        let frames = model::parse(&serde_json::to_string(&frame.groups).unwrap()).unwrap();
        let thread = &frames[0].threads[0];
        assert_eq!(thread.entry_source(), "main.c:42");
        assert_eq!(thread.runtime.as_ref().unwrap().cpu, None);
        assert_eq!(thread.runtime.as_ref().unwrap().stack_watermark, None);
        let heap = &frames[0].heaps[0];
        assert_eq!(heap.allocated_bytes, 0);
        assert_eq!(heap.max_allocated_bytes, None);
        assert_eq!(heap.chunks, None);
        assert_eq!(heap.waiters, None);
        let Event::Frame(no_threads) = frame_event(Snapshot::new(), None).unwrap() else {
            panic!()
        };
        assert!(no_threads.sampled_groups.is_empty());
        assert!(no_threads.groups.threads.is_empty());
        assert_eq!(no_threads.live_elapsed, None);
    }

    #[test]
    fn stopping_discards_queued_events_but_waits_for_cleanup_and_reports_errors() {
        let cancel = Cancellation::default();
        let (send, events) = smol::channel::bounded(1);
        send.try_send(Event::Notice {
            message: "old queued event".into(),
        })
        .unwrap();
        let (release, cleanup) = std::sync::mpsc::channel();
        let (exited_send, exited) = std::sync::mpsc::channel();
        let signal = cancel.clone();
        let join = thread::spawn(move || {
            let result = send.send_blocking(Event::Notice {
                message: "blocked producer".into(),
            });
            exited_send
                .send(result.is_err() && signal.is_cancelled())
                .unwrap();
            let _ = cleanup.recv_timeout(Duration::from_secs(5));
            anyhow::bail!("detach failed");
        });
        let mut worker = Worker {
            id: "old".into(),
            sequence: 0,
            events,
            cancel,
            join: Some(join),
            delivery_error: None,
        };
        worker.stop();
        assert!(exited.recv_timeout(Duration::from_secs(2)).unwrap());
        let stopping = worker.envelope(status(Status::Stopping, None));
        assert_eq!(stopping.sequence, 1);
        assert!(worker.next().is_none());
        release.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        let (terminal, done) = loop {
            if let Some(next) = worker.next() {
                break next;
            }
            assert!(Instant::now() < deadline, "cleanup did not finish");
            thread::sleep(Duration::from_millis(5));
        };
        assert!(done);
        assert_eq!(terminal.sequence, 2);
        assert_eq!(terminal.session_id.as_deref(), Some("old"));
        let Event::Status(status) = terminal.event else {
            panic!()
        };
        assert_eq!(status.status, Status::Failed);
        assert_eq!(status.error.as_deref(), Some("detach failed"));
        assert!(worker.join.is_none());
    }

    fn spin_until(
        cx: &mut VisualTestContext,
        mut condition: impl FnMut(&mut VisualTestContext) -> bool,
    ) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            cx.executor().advance_clock(Duration::from_millis(25));
            cx.run_until_parked();
            if condition(cx) {
                return;
            }
            assert!(Instant::now() < deadline, "session condition not reached");
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[gpui_kit::test]
    fn rust_replay_fixture_reaches_shared_views_and_cannot_interleave_with_reopen(
        cx: &mut TestAppContext,
    ) {
        let (app, mut cx) = crate::tests::setup(cx);
        let bench = "/api/session/replay";
        let fixtures = json!({
            "recording": "test-data/tests/fixtures/recordings/sys_heap_v4.3.ndjson.gz",
            "elf": "test-data/tests/fixtures/zephyr.elf"
        });
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.start_rust_core(window, cx);
                assert_eq!(app.adapter.state.connection, Connection::Connected);
                app.adapter_command(bench, fixtures.clone(), cx);
            })
        });
        spin_until(&mut cx, |cx| {
            app.read_with(cx, |app, _| {
                app.adapter
                    .state
                    .session
                    .as_ref()
                    .is_some_and(|s| s.frames_received > 0)
            })
        });
        let old_id = cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                let s = app.adapter.state.session.as_ref().unwrap();
                assert_eq!(app.frames[app.frame].threads.len(), 3);
                assert_eq!(app.frames[app.frame].heaps.len(), 1);
                let heap = &app.frames[app.frame].heaps[0];
                assert!(heap.chunks.is_none() && heap.waiters.is_none());
                assert!(app.frames[app.frame].elapsed_seconds.is_none());
                assert!(app.adapter.process.is_none() && app.adapter.startup.is_none());
                let id = s.id.clone();
                assert!(
                    app.rust_command("/api/session/view", json!({}), window, cx)
                        .unwrap_err()
                        .contains("not supported")
                );
                app.rust_command("/api/session/stop", json!({"session_id": id}), window, cx)
                    .unwrap();
                assert_eq!(
                    app.adapter.state.session.as_ref().unwrap().status,
                    Status::Stopping
                );
                assert!(
                    app.rust_command(bench, fixtures.clone(), window, cx)
                        .is_err()
                );
                id
            })
        });
        spin_until(&mut cx, |cx| {
            app.read_with(cx, |app, _| app.adapter.state.can_open())
        });
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                assert_eq!(
                    app.adapter.state.session.as_ref().unwrap().status,
                    Status::Stopped
                );
                app.rust_command(bench, fixtures.clone(), window, cx)
                    .unwrap();
                assert_ne!(app.adapter.state.session.as_ref().unwrap().id, old_id);
                assert!(
                    app.rust_command(
                        "/api/session/stop",
                        json!({"session_id": old_id}),
                        window,
                        cx
                    )
                    .is_err()
                );
                assert!(!app.pump_rust_session(&old_id, window, cx));
            })
        });
        spin_until(&mut cx, |cx| {
            app.read_with(cx, |app, _| app.adapter.state.can_open())
        });
        cx.update(|window, cx| app.update(cx, |app, cx| {
            let s = app.adapter.state.session.as_ref().unwrap();
            assert_eq!(s.status, Status::Ended);
            assert_eq!(s.frames_received, 20);
            assert_eq!(app.frames.len(), 20);
            assert_eq!(app.adapter.displayed.as_ref(), Some(&(s.id.clone(), 20)));
            app.rust_command(bench, json!({"elf": "test-data/tests/fixtures/zephyr.elf", "recording": "test-data/tests/fixtures"}), window, cx).unwrap();
        }));
        spin_until(&mut cx, |cx| {
            app.read_with(cx, |app, _| app.adapter.state.can_open())
        });
        app.read_with(&cx, |app, _| {
            let s = app.adapter.state.session.as_ref().unwrap();
            assert_eq!(s.status, Status::Failed);
            assert_eq!(s.frames_received, 0);
            assert!(s.error.as_deref().unwrap().contains("not a regular file"));
        });
    }

    #[gpui_kit::test]
    #[ignore = "requires a dedicated local GDB stub running the benchmark ELF"]
    fn rust_live_benchmark_gdb_smoke(cx: &mut TestAppContext) {
        let target = std::env::var("ZVIEW_GUI_TEST_GDB").expect("set ZVIEW_GUI_TEST_GDB");
        let (app, mut cx) = crate::tests::setup(cx);
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.start_rust_core(window, cx);
                app.rust_command(
                    "/api/session/live",
                    json!({
                        "elf": "test-data/zview-bench-60s/zephyr.elf", "target": target,
                    }),
                    window,
                    cx,
                )
                .unwrap();
            })
        });
        let collect_and_stop = |cx: &mut VisualTestContext| {
            spin_until(cx, |cx| {
                app.read_with(cx, |app, _| {
                    let s = app.adapter.state.session.as_ref().unwrap();
                    assert_ne!(s.status, Status::Failed, "{:?}", s.error);
                    s.frames_received >= 2
                })
            });
            let id = cx.update(|window, cx| {
                app.update(cx, |app, cx| {
                    let s = app.adapter.state.session.as_ref().unwrap();
                    let id = s.id.clone();
                    let frame = &app.frames[app.frame];
                    assert_eq!(frame.threads.len(), 18);
                    assert!(frame.is_live && frame.elapsed_seconds.is_some());
                    assert!(!app.live_history.is_empty());
                    assert!(s.live_view.is_none() && s.usage_window.is_none());
                    let heap = frame.heaps.iter().find(|h| h.name == "bench_heap").unwrap();
                    assert!(heap.chunks.is_none());
                    assert!(app.adapter_usage_controls(cx).is_none());
                    app.rust_command("/api/session/stop", json!({"session_id": id}), window, cx)
                        .unwrap();
                    id
                })
            });
            spin_until(cx, |cx| {
                app.read_with(cx, |app, _| app.adapter.state.can_reconnect())
            });
            app.read_with(cx, |app, _| {
                assert_eq!(
                    app.adapter.state.session.as_ref().unwrap().status,
                    Status::Stopped
                )
            });
            id
        };
        let id = collect_and_stop(&mut cx);
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.rust_command(
                    "/api/session/reconnect",
                    json!({"session_id": id}),
                    window,
                    cx,
                )
                .unwrap();
            })
        });
        assert_ne!(collect_and_stop(&mut cx), id);
    }
}
