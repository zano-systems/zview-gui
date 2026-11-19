//! Native-only transport and owned Python process. No acquisition or decoding here.
use serde::Deserialize;
use std::{
    ffi::OsString,
    io::{BufRead, BufReader, Read},
    net::{TcpStream, ToSocketAddrs},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

const HELP: &str = "ZView native (zview-gui-rs)\n\
    cargo run -- [--python PYTHON] [--data-root DIRECTORY] [--build-dir DIRECTORY]\n\
    cargo run -- --adapter http://127.0.0.1:PORT\n\
    Python defaults to ZVIEW_GUI_PYTHON, then python3. Install this checkout's\n\
    adapter in that interpreter. The Python adapter is required for all native sessions.\n\
    --build-dir (-d) starts Live once the adapter connects. Relative build paths\n\
    are resolved under --data-root (default: current directory). Connecting does not flash firmware.";

pub(crate) struct Options {
    pub endpoint: Option<String>,
    pub build_dir: Option<String>,
    python: OsString,
    data_root: PathBuf,
}

impl Options {
    pub fn parse(args: impl Iterator<Item = OsString>) -> Result<Self, String> {
        let mut options = Self {
            endpoint: None,
            build_dir: None,
            python: std::env::var_os("ZVIEW_GUI_PYTHON").unwrap_or_else(|| "python3".into()),
            data_root: std::env::current_dir().map_err(|e| e.to_string())?,
        };
        let mut args = args.peekable();
        let mut configured = false;
        while let Some(arg) = args.next() {
            match arg.to_str() {
                Some("--help" | "-h") => return Err(HELP.into()),
                Some("--adapter" | "--python" | "--data-root" | "--build-dir" | "-d") => {
                    let value = args
                        .next()
                        .ok_or_else(|| format!("Missing value for {arg:?}."))?;
                    match arg.to_str().unwrap() {
                        "--adapter" => {
                            options.endpoint = Some(validate_endpoint(
                                value.to_str().ok_or("Adapter URL must be UTF-8.")?,
                            )?);
                        }
                        "--python" => {
                            options.python = value;
                            configured = true;
                        }
                        "--build-dir" | "-d" => {
                            let value = value.to_str().ok_or("Build directory must be UTF-8.")?;
                            if value.trim().is_empty() {
                                return Err("Build directory must not be empty.".into());
                            }
                            options.build_dir = Some(value.to_owned());
                            configured = true;
                        }
                        _ => {
                            options.data_root = PathBuf::from(value);
                            configured = true;
                        }
                    }
                }
                Some(value) if value.starts_with('-') => {
                    return Err(format!("Unknown option: {value}\n{HELP}"));
                }
                _ => {
                    return Err(format!(
                        "Unexpected argument: {arg:?}. Choose Live or Replay through Configure session.\n{HELP}"
                    ));
                }
            }
        }
        if options.endpoint.is_some() && configured {
            return Err(
                "Choose an existing adapter or an owned Python adapter; do not combine them."
                    .into(),
            );
        }
        Ok(options)
    }

    pub fn start(self) -> Result<(String, Option<OwnedAdapter>), String> {
        if let Some(endpoint) = self.endpoint {
            return Ok((endpoint, None));
        }
        let process = OwnedAdapter::start(&self.python, &self.data_root)?;
        Ok((process.endpoint.clone(), Some(process)))
    }
}

pub(crate) fn validate_endpoint(value: &str) -> Result<String, String> {
    let uri = value
        .parse::<tungstenite::http::Uri>()
        .map_err(|e| e.to_string())?;
    if uri.scheme_str() != Some("http")
        || !matches!(
            uri.host(),
            Some("127.0.0.1" | "localhost" | "[::1]" | "::1")
        )
        || uri.port_u16().is_none_or(|port| port == 0)
        || uri
            .path_and_query()
            .is_some_and(|path| path.as_str() != "/")
        || uri.authority().is_some_and(|a| a.as_str().contains('@'))
    {
        return Err(
            "Adapter URL must be http://<loopback-host>:<port>, without credentials or a path."
                .into(),
        );
    }
    Ok(value.trim_end_matches('/').to_owned())
}

#[derive(Deserialize)]
struct Ready {
    url: String,
    protocol: u32,
    package_version: String,
}

pub(crate) struct OwnedAdapter {
    child: Option<Child>,
    endpoint: String,
}

type Started = Result<(String, Option<OwnedAdapter>), String>;

/// Retain startup ownership even if the UI cancels its connection task.
/// Closing during startup waits for this worker and cleans up its result.
pub(crate) struct Startup(Option<std::thread::JoinHandle<Started>>);

impl Startup {
    pub fn new(options: Options) -> Self {
        Self(Some(std::thread::spawn(move || options.start())))
    }

    pub fn try_finish(&mut self) -> Option<Started> {
        if !self.0.as_ref()?.is_finished() {
            return None;
        }
        Some(
            self.0
                .take()
                .unwrap()
                .join()
                .unwrap_or_else(|_| Err("Adapter startup worker failed.".into())),
        )
    }
}

impl Drop for Startup {
    fn drop(&mut self) {
        if let Some(worker) = self.0.take() {
            // Dropping a successful result also shuts down its owned adapter.
            drop(worker.join());
        }
    }
}

/// A joinable cleanup worker survives both window Close and system Quit.
pub(crate) struct Shutdown(Option<std::thread::JoinHandle<()>>);

impl Shutdown {
    pub fn new(startup: Option<Startup>, process: Option<OwnedAdapter>) -> Self {
        Self(Some(std::thread::spawn(move || {
            drop(startup);
            drop(process);
        })))
    }

    pub fn finished(&self) -> bool {
        self.0.as_ref().is_none_or(|worker| worker.is_finished())
    }
}

impl Drop for Shutdown {
    fn drop(&mut self) {
        if let Some(worker) = self.0.take() {
            let _ = worker.join();
        }
    }
}

impl OwnedAdapter {
    fn start(python: &OsString, root: &std::path::Path) -> Result<Self, String> {
        // Isolated Python uses the selected environment's installed package,
        // never a guessed sibling checkout or an injected PYTHONPATH.
        let child = Command::new(python)
            .args(["-I", "-u", "-m", "zview_gui.native_host", "--data-root"])
            .arg(root)
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit())
            .spawn().map_err(|e| format!("Cannot start {python:?}: {e}. Set ZVIEW_GUI_PYTHON to the adapter environment's Python."))?;
        let mut process = Self {
            child: Some(child),
            endpoint: String::new(),
        };
        let stdout = process.child.as_mut().unwrap().stdout.take().unwrap();
        let (sender, receiver) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let mut stdout = BufReader::new(stdout);
            let mut line = String::new();
            let result = stdout
                .by_ref()
                .take(4096)
                .read_line(&mut line)
                .map(|_| line)
                .map_err(|e| e.to_string());
            let _ = sender.send(result);
            // Keep the pipe drained even if a native probe library writes
            // directly to fd 1 instead of Python's redirected sys.stdout.
            let _ = std::io::copy(&mut stdout, &mut std::io::stderr());
        });
        let line = receiver.recv_timeout(Duration::from_secs(15))
            .map_err(|_| "Adapter startup timed out. Check the selected Python environment and terminal diagnostics.".to_owned())??;
        let ready: Ready = serde_json::from_str(&line)
            .map_err(|_| "Adapter did not report readiness. Install this checkout's adapter in ZVIEW_GUI_PYTHON's environment; see terminal diagnostics.".to_owned())?;
        if ready.protocol != crate::adapter_protocol::PROTOCOL_VERSION {
            return Err(format!(
                "Adapter protocol {} is incompatible with native protocol {}.",
                ready.protocol,
                crate::adapter_protocol::PROTOCOL_VERSION
            ));
        }
        eprintln!("ZView adapter {} ready.", ready.package_version);
        process.endpoint = validate_endpoint(&ready.url)?;
        Ok(process)
    }

    pub fn exited(&mut self) -> bool {
        self.child
            .as_mut()
            .is_some_and(|child| matches!(child.try_wait(), Ok(Some(_))))
    }
}

impl Drop for OwnedAdapter {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            // EOF requests graceful session/hardware cleanup. The native close
            // flow runs this on a worker and awaits it before quitting. A
            // detached reaper would be lost when the application exits.
            // Attached servers never have an OwnedAdapter.
            stop_child(&mut child, Duration::from_secs(5));
        }
    }
}

fn stop_child(child: &mut Child, grace: Duration) {
    drop(child.stdin.take());
    let deadline = Instant::now() + grace;
    loop {
        if matches!(child.try_wait(), Ok(Some(_))) {
            return;
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    // Kill only this owned child, then reap it before returning.
    let _ = child.kill();
    if let Err(error) = child.wait() {
        eprintln!("Cannot reap native adapter: {error}");
    }
}

/// Match the small HTTP surface used by the shared GPUI session controls.
pub(crate) struct Request {
    url: String,
    body: Option<serde_json::Value>,
}

pub(crate) struct Response {
    status: u16,
    body: String,
}

impl Request {
    pub fn post(url: &str) -> Self {
        Self {
            url: url.into(),
            body: None,
        }
    }
    pub fn json(mut self, body: &serde_json::Value) -> Result<Self, String> {
        self.body = Some(body.clone());
        Ok(self)
    }
    pub async fn send(self) -> Result<Response, String> {
        smol::unblock(move || {
            let agent: ureq::Agent = ureq::Agent::config_builder()
                .http_status_as_error(false)
                .proxy(None)
                .max_redirects(0)
                .timeout_global(Some(Duration::from_secs(120)))
                .build()
                .into();
            let mut response = agent
                .post(&self.url)
                .send_json(self.body.unwrap_or_default())
                .map_err(|e| e.to_string())?;
            let status = response.status().as_u16();
            let body = response
                .body_mut()
                .with_config()
                .limit(16 * 1024 * 1024)
                .read_to_string()
                .map_err(|e| e.to_string())?;
            Ok(Response { status, body })
        })
        .await
    }
}

impl Response {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
    pub async fn text(self) -> Result<String, String> {
        Ok(self.body)
    }
    pub async fn json<T: serde::de::DeserializeOwned>(self) -> Result<T, String> {
        serde_json::from_str(&self.body).map_err(|e| e.to_string())
    }
}

/// One bounded stream per connection. Dropping the receiver releases the socket,
/// and therefore the adapter's controlling-client/view demand.
pub(crate) async fn connect(
    endpoint: String,
    client_id: String,
) -> Result<smol::channel::Receiver<Result<String, String>>, String> {
    smol::unblock(move || {
        let uri = endpoint
            .parse::<tungstenite::http::Uri>()
            .map_err(|e| e.to_string())?;
        let host = uri
            .host()
            .ok_or("Missing adapter host.")?
            .trim_matches(['[', ']']);
        let mut addresses = (host, uri.port_u16().ok_or("Missing adapter port.")?)
            .to_socket_addrs()
            .map_err(|e| e.to_string())?
            .filter(|address| address.ip().is_loopback());
        let mut failure = "Adapter must be loopback.".to_owned();
        let tcp = addresses
            .find_map(|address| {
                TcpStream::connect_timeout(&address, Duration::from_secs(3))
                    .map_err(|error| failure = error.to_string())
                    .ok()
            })
            .ok_or(failure)?;
        tcp.set_read_timeout(Some(Duration::from_secs(3)))
            .map_err(|e| e.to_string())?;
        tcp.set_write_timeout(Some(Duration::from_secs(3)))
            .map_err(|e| e.to_string())?;
        let url = format!("ws{}/api/events?client_id={client_id}", &endpoint[4..]);
        let config = tungstenite::protocol::WebSocketConfig::default()
            .max_message_size(Some(16 * 1024 * 1024));
        let (mut socket, _) = tungstenite::client::client_with_config(url, tcp, Some(config))
            .map_err(|e| e.to_string())?;
        socket
            .get_mut()
            .set_read_timeout(Some(Duration::from_millis(250)))
            .map_err(|e| e.to_string())?;
        let (sender, receiver) = smol::channel::bounded(32);
        std::thread::spawn(move || {
            while !sender.is_closed() {
                let result = match socket.read() {
                    Ok(tungstenite::Message::Text(text)) => Ok(text.to_string()),
                    Ok(tungstenite::Message::Ping(_)) => {
                        if socket.flush().is_err() {
                            break;
                        }
                        continue;
                    }
                    Ok(tungstenite::Message::Pong(_)) => continue,
                    Ok(tungstenite::Message::Close(_)) => break,
                    Ok(_) => Err("Expected JSON text from adapter.".into()),
                    Err(tungstenite::Error::Io(e))
                        if matches!(
                            e.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                        ) =>
                    {
                        continue;
                    }
                    Err(e) => Err(e.to_string()),
                };
                let failed = result.is_err();
                if sender.send_blocking(result).is_err() || failed {
                    break;
                }
            }
            let _ = socket.close(None);
        });
        Ok(receiver)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn test_child(script: &str) -> Child {
        Command::new("/bin/sh")
            .args(["-c", script])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap()
    }

    #[test]
    #[cfg(unix)]
    fn owned_adapter_drop_waits_for_eof_cleanup() {
        let mut child = test_child("cat >/dev/null; printf cleaned");
        let mut stdout = child.stdout.take().unwrap();
        drop(OwnedAdapter {
            child: Some(child),
            endpoint: String::new(),
        });
        let mut result = String::new();
        stdout.read_to_string(&mut result).unwrap();
        assert_eq!(result, "cleaned");
    }

    #[test]
    #[cfg(unix)]
    fn unresponsive_owned_child_is_killed_and_reaped_not_another_process() {
        let mut child = test_child("exec sleep 30");
        let mut external = test_child("cat >/dev/null");
        stop_child(&mut child, Duration::from_millis(25));
        assert!(child.try_wait().unwrap().is_some());
        assert!(external.try_wait().unwrap().is_none());
        stop_child(&mut external, Duration::from_secs(1));
        assert!(external.try_wait().unwrap().is_some());
    }

    #[test]
    #[cfg(unix)]
    fn closing_during_startup_waits_and_cleans_up_the_result() {
        let mut child = test_child("cat >/dev/null; printf cleaned");
        let mut stdout = child.stdout.take().unwrap();
        let startup = Startup(Some(std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            Ok((
                String::new(),
                Some(OwnedAdapter {
                    child: Some(child),
                    endpoint: String::new(),
                }),
            ))
        })));
        drop(startup);
        let mut result = String::new();
        stdout.read_to_string(&mut result).unwrap();
        assert_eq!(result, "cleaned");
    }

    #[test]
    #[cfg(unix)]
    fn system_quit_during_window_cleanup_joins_the_same_worker() {
        let mut child = test_child("cat >/dev/null; printf cleaned");
        let mut stdout = child.stdout.take().unwrap();
        let worker = Shutdown::new(
            None,
            Some(OwnedAdapter {
                child: Some(child),
                endpoint: String::new(),
            }),
        );
        // The quit hook drops this guard even if window Close already took
        // the owned process out of the UI's process slot.
        drop(worker);
        let mut result = String::new();
        stdout.read_to_string(&mut result).unwrap();
        assert_eq!(result, "cleaned");
    }

    #[test]
    fn endpoints_are_local_explicit_and_not_redirectable() {
        for value in ["http://127.0.0.1:8765", "http://localhost:1234/"] {
            assert!(validate_endpoint(value).is_ok(), "{value}");
        }
        for value in [
            "https://127.0.0.1:1234",
            "http://example.com:1234",
            "http://127.0.0.1",
            "http://127.0.0.1:0",
            "http://127.0.0.1:1234/foo",
            "http://user@127.0.0.1:1234",
            "http://127.0.0.1:1234/?x=1",
        ] {
            assert!(validate_endpoint(value).is_err(), "{value}");
        }
    }

    #[test]
    fn native_options_validate_adapter_configuration() {
        assert!(Options::parse(std::iter::empty()).is_ok());
        for args in [
            vec!["--adapter"],
            vec!["--python"],
            vec!["--data-root"],
            vec!["--build-dir"],
            vec!["-d"],
            vec!["--build-dir", ""],
            vec!["--adapter", "http://localhost:1234", "--python", "python3"],
            vec!["--adapter", "http://localhost:1234", "--build-dir", "build"],
        ] {
            assert!(Options::parse(args.into_iter().map(OsString::from)).is_err());
        }
    }

    #[test]
    fn native_build_argument_preserves_paths_and_data_root() {
        assert!(
            Options::parse(std::iter::empty())
                .unwrap()
                .build_dir
                .is_none()
        );
        for flag in ["--build-dir", "-d"] {
            let options = Options::parse(
                [
                    "--data-root",
                    "/project workspace",
                    flag,
                    "application build",
                ]
                .into_iter()
                .map(OsString::from),
            )
            .unwrap();
            assert_eq!(options.build_dir.as_deref(), Some("application build"));
            assert_eq!(options.data_root, PathBuf::from("/project workspace"));
            assert!(options.endpoint.is_none());
        }
        let options = Options::parse(
            ["--build-dir", "/project workspace/application build"]
                .into_iter()
                .map(OsString::from),
        )
        .unwrap();
        assert_eq!(
            options.build_dir.as_deref(),
            Some("/project workspace/application build")
        );
    }

    #[test]
    fn attachment_does_not_start_or_own_a_python_process() {
        let options = Options::parse(
            ["--adapter", "http://127.0.0.1:8765/"]
                .into_iter()
                .map(OsString::from),
        )
        .unwrap();
        let (endpoint, owned) = options.start().unwrap();
        assert_eq!(endpoint, "http://127.0.0.1:8765");
        assert!(owned.is_none());
    }

    #[test]
    #[ignore = "requires ZVIEW_GUI_PYTHON with this checkout's adapter installed"]
    fn native_transport_replays_through_owned_python_adapter() {
        let python = std::env::var_os("ZVIEW_GUI_PYTHON").expect("set ZVIEW_GUI_PYTHON");
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let process = OwnedAdapter::start(&python, &root).expect("start owned adapter");
        let endpoint = process.endpoint.clone();
        smol::block_on(async {
            let messages = connect(endpoint.clone(), uuid::Uuid::new_v4().to_string())
                .await
                .unwrap();
            let receive = || async {
                smol::future::race(async { messages.recv().await.unwrap().unwrap() }, async {
                    smol::Timer::after(Duration::from_secs(20)).await;
                    panic!("timed out waiting for an adapter event");
                })
                .await
            };
            let mut state = crate::adapter_protocol::ClientState::default();
            let snapshot = serde_json::from_str(&receive().await).unwrap();
            assert!(state.accept(snapshot).unwrap());
            let response = Request::post(&format!("{endpoint}/api/session/replay"))
                .json(&serde_json::json!({
                    "recording": "test-data/tests/fixtures/recordings/sys_heap_v4.3.ndjson.gz",
                    "elf": "test-data/tests/fixtures/zephyr.elf"
                }))
                .unwrap()
                .send()
                .await
                .unwrap();
            let status = response.status;
            let body = response.text().await.unwrap();
            assert!((200..300).contains(&status), "{status}: {body}");
            loop {
                let event = serde_json::from_str(&receive().await).unwrap();
                state.accept(event).unwrap();
                let session = state.session.as_ref().unwrap();
                if session.status == crate::adapter_protocol::Status::Failed {
                    panic!("replay failed: {:?}", session.error);
                }
                if session.status == crate::adapter_protocol::Status::Ended {
                    assert!(session.frames_received > 0);
                    assert!(session.last_frame.is_some());
                    break;
                }
            }
            drop(messages);
            // A second client can recover the same completed session snapshot.
            let recovered = connect(endpoint, uuid::Uuid::new_v4().to_string())
                .await
                .unwrap();
            let text = recovered.recv().await.unwrap().unwrap();
            let snapshot: crate::adapter_protocol::Envelope = serde_json::from_str(&text).unwrap();
            assert!(matches!(
                snapshot.event,
                crate::adapter_protocol::Event::Snapshot(Some(_))
            ));
        });
        drop(process);
    }
}
