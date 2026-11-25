//! Native GPUI host for the shared session UI; browser startup stays in adapter_ui.
use super::*;

/// Consume the CLI request only once, after the first valid session snapshot.
/// Transport reconnects must not restart acquisition or replace another session.
fn take_live_start(
    build_dir: &mut Option<String>,
    state: &crate::adapter_protocol::ClientState,
) -> Result<Option<serde_json::Value>, String> {
    if state.connection != Connection::Connected {
        return Ok(None);
    }
    let Some(build_dir) = build_dir.take() else {
        return Ok(None);
    };
    if !state.can_open() {
        return Err(
            "Live auto-connect skipped: the adapter already has an active or pending session. Use the session controls to connect.".into(),
        );
    }
    Ok(Some(json!({ "build_dir": build_dir })))
}

impl ThreadsApp {
    pub(crate) fn start_native_adapter(
        &mut self,
        mut options: crate::native_adapter::Options,
        cx: &mut Context<Self>,
    ) {
        self.adapter.available = true;
        self.adapter.state.connection = Connection::Connecting;
        self.error = None;
        let mut live_build = options.build_dir.take();
        self.adapter.startup = Some(crate::native_adapter::Startup::new(options));
        self.adapter.connection_task = Some(cx.spawn(async move |this, cx| {
            let started = loop {
                let result = this.update(cx, |app, _| {
                    let result = app.adapter.startup.as_mut().and_then(|startup| startup.try_finish());
                    if result.is_some() {
                        app.adapter.startup.take();
                    }
                    result
                });
                match result {
                    Ok(Some(result)) => break result,
                    Err(_) => return,
                    Ok(None) => adapter_delay(50).await,
                }
            };
            let endpoint = match started {
                Ok((endpoint, process)) => {
                    if this
                        .update(cx, |app, cx| {
                            app.adapter.endpoint = endpoint.clone();
                            app.adapter.process = process;
                            cx.notify();
                        })
                        .is_err()
                    {
                        return;
                    }
                    endpoint
                }
                Err(error) => {
                    let _ = this.update(cx, |app, cx| {
                        app.adapter.state.connection = Connection::Failed;
                        app.error = Some(error);
                        cx.notify();
                    });
                    return;
                }
            };
            loop {
                let client_id = uuid::Uuid::new_v4().to_string();
                if this
                    .update(cx, |app, _| {
                        app.adapter.client_id = Some(client_id.clone());
                        app.adapter.view_interest.reset();
                    })
                    .is_err()
                {
                    return;
                }
                let result = crate::native_adapter::connect(endpoint.clone(), client_id).await;
                let mut failure = None;
                match result {
                    Ok(messages) => {
                        let mut needs_snapshot = true;
                        loop {
                            // The UI freshness tick is independent of acquisition.
                            let next = smol::future::race(
                                async { messages.recv().await.ok() },
                                async {
                                    adapter_delay(1000).await;
                                    None
                                },
                            )
                            .await;
                            if let Some(message) = next {
                                let accepted = message.and_then(|text| {
                                    let message: Envelope = serde_json::from_str(&text)
                                        .map_err(|e| e.to_string())?;
                                    if needs_snapshot && !matches!(message.event, Event::Snapshot(_)) {
                                        return Err("Expected adapter snapshot.".into());
                                    }
                                    this.update_in(cx, |app, window, cx| {
                                        app.accept_adapter(message, window, cx)?;
                                        match take_live_start(&mut live_build, &app.adapter.state) {
                                            Ok(Some(body)) => app.adapter_command("/api/session/live", body, None, cx),
                                            Ok(None) => {}
                                            Err(error) => {
                                                eprintln!("{error}");
                                                app.error = Some(error);
                                                cx.notify();
                                            }
                                        }
                                        Ok(())
                                    })
                                    .map_err(|e| e.to_string())?
                                });
                                if let Err(error) = accepted {
                                    failure = Some(error);
                                    break;
                                }
                                needs_snapshot = false;
                            } else if messages.is_closed() {
                                break;
                            } else if this.update(cx, |_, cx| cx.notify()).is_err() {
                                return;
                            }
                        }
                    }
                    Err(error) => failure = Some(error),
                }
                let exited = this.update(cx, |app, cx| {
                    app.adapter.client_id = None;
                    app.adapter.view_interest.reset();
                    app.live_history.interrupt();
                    let exited = app.adapter.process.as_mut().is_some_and(|p| p.exited());
                    app.adapter.state.connection = if exited {
                        Connection::Failed
                    } else {
                        Connection::Retrying
                    };
                    app.error = Some(if exited {
                        "The Python adapter exited. Check terminal diagnostics and restart the native app.".into()
                    } else {
                        format!("Adapter disconnected; reconnecting… {}", failure.unwrap_or_default())
                    });
                    cx.notify();
                    exited
                });
                if !matches!(exited, Ok(false)) {
                    return;
                }
                adapter_delay(1500).await;
            }
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::take_live_start;
    use crate::adapter_protocol::{ClientState, Connection, SessionIndicator};
    use gpui::TestAppContext;
    use serde_json::json;
    use std::time::{Duration, Instant};

    #[gpui_kit::test]
    fn missing_python_launch_leaves_session_in_non_busy_failure(cx: &mut TestAppContext) {
        let (app, mut cx) = crate::tests::setup(cx);
        let python =
            std::env::temp_dir().join(format!("missing-zview-python-{}", uuid::Uuid::new_v4()));
        let options = crate::native_adapter::Options::parse(
            ["--python".into(), python.into_os_string()].into_iter(),
        )
        .unwrap();
        cx.update(|_, cx| app.update(cx, |app, cx| app.start_native_adapter(options, cx)));
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            cx.run_until_parked();
            if app.read_with(&cx, |app, _| app.error.is_some()) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "startup failure did not reach the UI"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        app.read_with(&cx, |app, _| {
            let error = app.error.as_deref().unwrap();
            assert!(error.contains("Cannot start") && error.contains("missing-zview-python-"));
            assert!(error.contains("ZVIEW_GUI_PYTHON"));
            assert_eq!(app.adapter.state.connection, Connection::Failed);
            assert_eq!(app.adapter.state.indicator(false), SessionIndicator::Failed);
        });
    }

    #[test]
    fn cli_live_start_waits_for_snapshot_and_is_not_repeated_after_reconnect() {
        let mut build = Some("project build".to_string());
        let mut state = ClientState::default();
        assert_eq!(take_live_start(&mut build, &state).unwrap(), None);
        assert_eq!(build.as_deref(), Some("project build"));
        state.connection = Connection::Connected;
        assert_eq!(
            take_live_start(&mut build, &state).unwrap(),
            Some(json!({"build_dir": "project build"}))
        );
        state.connection = Connection::Retrying;
        assert_eq!(take_live_start(&mut build, &state).unwrap(), None);
        state.connection = Connection::Connected;
        assert_eq!(take_live_start(&mut build, &state).unwrap(), None);
    }

    #[test]
    fn no_build_argument_does_not_open_a_session_or_replace_a_pending_one() {
        let mut state = ClientState::default();
        state.connection = Connection::Connected;
        assert_eq!(take_live_start(&mut None, &state).unwrap(), None);
        state.pending = true;
        let mut build = Some("build".to_string());
        assert!(take_live_start(&mut build, &state).is_err());
        state.pending = false;
        assert_eq!(take_live_start(&mut build, &state).unwrap(), None);
    }
}
