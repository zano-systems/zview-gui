//! Native GPUI host for the shared session UI; browser startup stays in web_adapter.
use super::*;

impl ThreadsApp {
    pub(crate) fn start_native_adapter(
        &mut self,
        options: crate::native_adapter::Options,
        cx: &mut Context<Self>,
    ) {
        self.adapter.available = true;
        self.error = None;
        self.adapter.connection_task = Some(cx.spawn(async move |this, cx| {
            let started = smol::unblock(move || options.start()).await;
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
                                        app.accept_adapter(message, window, cx)
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
                    app.adapter.state.connection = Connection::Retrying;
                    app.adapter.client_id = None;
                    app.adapter.view_interest.reset();
                    app.live_history.interrupt();
                    let exited = app.adapter.process.as_mut().is_some_and(|p| p.exited());
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
