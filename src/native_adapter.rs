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

const HELP: &str = "Native ZView\n\
    cargo run -- [--python PYTHON] [--data-root DIRECTORY]\n\
    cargo run -- --adapter http://127.0.0.1:PORT\n\
    cargo run -- [--offline] [DUMP.json]\n\
    Python defaults to ZVIEW_GUI_PYTHON, then python3. Install this checkout's\n\
    adapter in that interpreter. --offline opens the bundled sample without Python.";

pub(crate) struct Options {
    pub offline: bool,
    pub source: Option<PathBuf>,
    pub endpoint: Option<String>,
    python: OsString,
    data_root: PathBuf,
}

impl Options {
    pub fn parse(args: impl Iterator<Item = OsString>) -> Result<Self, String> {
        let mut options = Self {
            offline: false,
            source: None,
            endpoint: None,
            python: std::env::var_os("ZVIEW_GUI_PYTHON").unwrap_or_else(|| "python3".into()),
            data_root: std::env::current_dir().map_err(|e| e.to_string())?,
        };
        let mut args = args.peekable();
        let mut configured = false;
        while let Some(arg) = args.next() {
            match arg.to_str() {
                Some("--help" | "-h") => return Err(HELP.into()),
                Some("--offline") => options.offline = true,
                Some("--adapter" | "--python" | "--data-root") => {
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
                        _ => {
                            options.data_root = PathBuf::from(value);
                            configured = true;
                        }
                    }
                }
                Some(value) if value.starts_with('-') => {
                    return Err(format!("Unknown option: {value}\n{HELP}"));
                }
                _ if options.source.is_none() => options.source = Some(PathBuf::from(arg)),
                _ => return Err(HELP.into()),
            }
        }
        if options.source.is_some() {
            options.offline = true;
        }
        if (options.offline && (configured || options.endpoint.is_some()))
            || (options.endpoint.is_some() && configured)
        {
            return Err("Choose offline JSON, an existing adapter, or an owned Python adapter; do not combine them.".into());
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
        eprintln!("ZView adapter {}: {}", ready.package_version, ready.url);
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
            // EOF is the graceful shutdown request. Do not block the UI while
            // the backend disconnects hardware. Never terminate an attached server.
            drop(child.stdin.take());
            std::thread::spawn(move || {
                let deadline = Instant::now() + Duration::from_secs(5);
                loop {
                    if matches!(child.try_wait(), Ok(Some(_))) {
                        return;
                    }
                    if Instant::now() >= deadline {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                let _ = child.kill();
                let _ = child.wait();
            });
        }
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
    fn positional_json_preserves_offline_workflow() {
        let options =
            Options::parse(["file with spaces.json"].into_iter().map(OsString::from)).unwrap();
        assert!(options.offline);
        assert_eq!(options.source, Some("file with spaces.json".into()));
        for args in [
            vec!["--adapter"],
            vec!["--offline", "--python", "python3"],
            vec!["--adapter", "http://localhost:1234", "dump.json"],
        ] {
            assert!(Options::parse(args.into_iter().map(OsString::from)).is_err());
        }
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
