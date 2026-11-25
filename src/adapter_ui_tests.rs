//! Native rendering uses the same event installation and session form as web.
use gpui::{TestAppContext, px, size};
use serde_json::json;
use std::path::PathBuf;

#[gpui_kit::test]
fn native_session_controls_install_replay_frames_and_reject_stale_events(cx: &mut TestAppContext) {
    let (app, mut cx) = crate::tests::setup(cx);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.adapter.available = true;
            let snapshot = serde_json::from_value(json!({
                "version": crate::adapter_protocol::PROTOCOL_VERSION,
                "type": "snapshot", "sequence": 0, "session_id": "native-replay",
                "data": {
                    "id": "native-replay",
                    "source": {"mode": "replay", "recording": "native.ndjson.gz"},
                    "elf": "zephyr.elf", "status": "opening", "frames_received": 0,
                    "last_frame": null, "last_notice": null, "error": null
                }
            }))
            .unwrap();
            app.accept_adapter(snapshot, window, cx).unwrap();
            for sequence in 1..=2 {
                let frame = serde_json::from_value(json!({
                    "version": crate::adapter_protocol::PROTOCOL_VERSION,
                    "type": "frame", "sequence": sequence, "session_id": "native-replay",
                    "data": {"decoded_at": 123.5 + sequence as f64,
                        "groups": {"threads": []}}
                }))
                .unwrap();
                app.accept_adapter(frame, window, cx).unwrap();
            }
            assert_eq!(app.frames.len(), 2);
            assert_eq!(app.source, Some(PathBuf::from("native.ndjson.gz")));
            assert_eq!(app.recording_elf, Some(PathBuf::from("zephyr.elf")));
            let stale = serde_json::from_value(json!({
                "version": crate::adapter_protocol::PROTOCOL_VERSION,
                "type": "frame", "sequence": 3, "session_id": "old-session",
                "data": {"decoded_at": 200., "groups": {"threads": []}}
            }))
            .unwrap();
            app.accept_adapter(stale, window, cx).unwrap();
            assert_eq!(app.frames.len(), 2);
        });
        window.draw(cx).clear(cx);
    });
    assert!(cx.debug_bounds("session-header-controls").is_some());
    cx.update(|window, cx| {
        app.update(cx, |app, cx| app.open_session_dialog(window, cx));
        window.draw(cx).clear(cx);
    });
    assert!(cx.debug_bounds("session-configuration").is_some());
    cx.simulate_resize(size(px(1000.), px(680.)));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(cx.debug_bounds("session-configuration").is_some());
}

#[gpui_kit::test]
fn command_error_survives_health_and_is_scoped_to_the_confirmed_session(cx: &mut TestAppContext) {
    let (app, mut cx) = crate::tests::setup(cx);
    let snapshot = |id| {
        serde_json::from_value(json!({
            "version": crate::adapter_protocol::PROTOCOL_VERSION,
            "type": "snapshot", "sequence": 0, "session_id": id,
            "data": {
                "id": id, "elf": "zephyr.elf", "status": "reading", "frames_received": 0,
                "source": {"mode": "live", "configuration": {"build_dir": "build"}, "runner": "jlink"},
                "usage_window": 23, "last_frame": null, "last_notice": null, "error": null
            }
        }))
        .unwrap()
    };
    let usage = "/api/session/usage-window";
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.accept_adapter(snapshot("first"), window, cx).unwrap();
            let request = app.begin_adapter_command(usage, None);
            app.finish_adapter_command(request, Err("Averaging rejected".into()), window, cx);
            let health = serde_json::from_value(json!({
                "version": crate::adapter_protocol::PROTOCOL_VERSION,
                "type": "health", "sequence": 1, "session_id": "first",
                "data": {"samples_per_second": 9.8}
            }))
            .unwrap();
            app.accept_adapter(health, window, cx).unwrap();
            assert_eq!(app.adapter_command_error(), Some("Averaging rejected"));
            assert!(app.adapter.draft.error.is_none());
        });
        window.draw(cx).clear(cx);
    });
    assert!(cx.debug_bounds("session-command-error").is_some());
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            let retry = app.begin_adapter_command(usage, None);
            assert!(app.adapter_command_error().is_none());
            app.finish_adapter_command(retry, Err("Retry rejected".into()), window, cx);
            let stop = app.begin_adapter_command("/api/session/stop", None);
            assert_eq!(app.adapter_command_error(), Some("Retry rejected"));
            app.accept_adapter(snapshot("second"), window, cx).unwrap();
            assert!(!app.adapter.state.pending);
            assert!(app.adapter_command_error().is_none());

            app.finish_adapter_command(stop, Err("Late stop rejection".into()), window, cx);
            assert!(app.adapter_command_error().is_none());

            let stale = app.begin_adapter_command(usage, None);
            app.accept_adapter(snapshot("third"), window, cx).unwrap();
            let new_request = app.begin_adapter_command(usage, None);
            app.finish_adapter_command(stale, Err("Late usage rejection".into()), window, cx);
            assert!(app.adapter.state.pending);
            assert!(app.adapter_command_error().is_none());
            app.finish_adapter_command(new_request, Ok(()), window, cx);
        });
        window.draw(cx).clear(cx);
    });
    assert!(cx.debug_bounds("session-command-error").is_none());
}

#[gpui_kit::test]
fn submitting_form_gets_the_rejection_in_either_snapshot_order(cx: &mut TestAppContext) {
    for snapshot_first in [false, true] {
        let (app, mut cx) = crate::tests::setup(cx);
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.open_session_dialog(window, cx);
                let owner = app.adapter.draft.open_generation().unwrap();
                let attempt = app.begin_adapter_command("/api/session/live", Some(owner));
                let snapshot = serde_json::from_value(json!({
                    "version": crate::adapter_protocol::PROTOCOL_VERSION,
                    "type": "snapshot", "sequence": 0, "session_id": "winner",
                    "data": {
                        "id": "winner", "elf": "zephyr.elf", "status": "reading",
                        "frames_received": 0,
                        "source": {"mode": "live", "configuration": {"build_dir": "build"}, "runner": "jlink"},
                        "last_frame": null, "last_notice": null, "error": null
                    }
                }))
                .unwrap();
                let rejection = Err("Stop the current session first.".into());
                if snapshot_first {
                    app.accept_adapter(snapshot, window, cx).unwrap();
                    app.finish_adapter_command(attempt, rejection, window, cx);
                } else {
                    app.finish_adapter_command(attempt, rejection, window, cx);
                    app.accept_adapter(snapshot, window, cx).unwrap();
                }
                assert!(app.adapter.draft.open);
                assert_eq!(
                    app.adapter.draft.error.as_deref(),
                    Some("Stop the current session first."),
                    "snapshot first: {snapshot_first}",
                );
                assert!(app.adapter_command_error().is_none());
            });
        });
    }
}

#[gpui_kit::test]
fn non_form_open_completions_leave_an_open_draft_untouched(cx: &mut TestAppContext) {
    for result in [Err("Other recording rejected".into()), Ok(())] {
        let (app, mut cx) = crate::tests::setup(cx);
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.open_session_dialog(window, cx);
                let draft = app.adapter.draft.request(cx);
                let generation = app.adapter.draft.open_generation();
                app.adapter.draft.error = Some("Existing form error".into());
                let attempt = app.begin_adapter_command("/api/session/replay", None);
                app.finish_adapter_command(attempt, result, window, cx);
                assert_eq!(app.adapter.draft.open_generation(), generation);
                assert_eq!(app.adapter.draft.request(cx), draft);
                assert_eq!(
                    app.adapter.draft.error.as_deref(),
                    Some("Existing form error")
                );
            });
        });
    }
}

#[gpui_kit::test]
fn reading_without_a_first_frame_shows_mode_specific_sidebar_status(cx: &mut TestAppContext) {
    let (app, mut cx) = crate::tests::setup(cx);
    for (source, status, live) in [
        (
            json!({"mode": "replay", "recording": "capture.ndjson.gz"}),
            "session-status-text:REPLAY · Decoding",
            false,
        ),
        (
            json!({"mode": "live", "configuration": {"build_dir": "build"}, "runner": "jlink"}),
            "session-status-text:LIVE · Waiting",
            true,
        ),
    ] {
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.adapter.available = true;
                app.adapter.state = crate::adapter_protocol::ClientState::default();
                app.adapter
                    .state
                    .accept(
                        serde_json::from_value(json!({
                            "version": crate::adapter_protocol::PROTOCOL_VERSION,
                            "type": "snapshot", "sequence": 0, "session_id": "first-frame",
                            "data": {
                                "id": "first-frame", "source": source,
                                "elf": "zephyr.elf", "status": "reading", "frames_received": 0,
                                "last_frame": null, "last_notice": null, "error": null
                            }
                        }))
                        .unwrap(),
                    )
                    .unwrap();
                cx.notify();
            });
            window.draw(cx).clear(cx);
        });
        assert!(cx.debug_bounds(status).is_some(), "{status}");
        assert_eq!(cx.debug_bounds("session-waiting-detail").is_some(), live);
    }
}

#[gpui_kit::test]
fn session_status_and_read_warnings_remain_readable_beside_actions(cx: &mut TestAppContext) {
    let (app, mut cx) = crate::tests::setup(cx);
    let snapshot = json!({
        "version": crate::adapter_protocol::PROTOCOL_VERSION,
        "type": "snapshot", "sequence": 0, "session_id": "status-layout",
        "data": {
            "id": "status-layout",
            "source": {"mode": "live", "configuration": {"build_dir": "build"}, "runner": "jlink"},
            "elf": "build/zephyr/zephyr.elf", "status": "reading", "frames_received": 0,
            "last_frame": null, "last_notice": null, "error": null
        }
    });
    for (width, height) in [(1000., 680.), (1480., 940.)] {
        cx.simulate_resize(size(px(width), px(height)));
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.adapter.available = true;
                app.adapter.state = crate::adapter_protocol::ClientState::default();
                app.adapter
                    .state
                    .accept(serde_json::from_value(snapshot.clone()).unwrap())
                    .unwrap();
                cx.notify();
            });
            window.draw(cx).clear(cx);
        });
        let status = cx.debug_bounds("session-header-status").unwrap();
        let progress = cx.debug_bounds("session-header-progress").unwrap();
        let reading_status_width = status.size.width;
        let actions = app.read_with(&cx, |app, _| app.adapter.action_bounds.get());
        assert!((progress.center().y - status.center().y).abs() <= px(0.5));
        assert!((progress.center().y - actions.center().y).abs() <= px(0.5));
        assert!(actions.size.width > px(0.));
        assert!(status.right() <= progress.left());
        assert!(progress.right() <= actions.left());
        let waiting = cx.debug_bounds("session-waiting-detail").unwrap();
        let elf = cx.debug_bounds("session-header-source").unwrap();
        assert!(waiting.top() >= elf.bottom());
        assert!(status.left() >= px(0.) && status.right() <= px(300.));
        assert!(actions.right() <= px(300.));

        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                let session = app.adapter.state.session.as_mut().unwrap();
                session.read_warning = true;
                session.last_notice = Some(
                    "Unable to read the current target measurements. Check the probe connection and matching firmware before reconnecting. ".repeat(3),
                );
                cx.notify();
            });
            window.draw(cx).clear(cx);
        });
        let warning = cx.debug_bounds("session-read-warning").unwrap();
        let status = cx.debug_bounds("session-header-status").unwrap();
        let elf = cx.debug_bounds("session-header-source").unwrap();
        assert!(warning.top() >= elf.bottom());
        assert!(warning.size.height > status.size.height * 2.);
        assert!(warning.left() >= px(0.) && warning.right() <= px(300.));
        assert!(cx.debug_bounds("session-waiting-detail").is_none());

        for inactive in [
            crate::adapter_protocol::Status::Opening,
            crate::adapter_protocol::Status::Stopping,
            crate::adapter_protocol::Status::Stopped,
            crate::adapter_protocol::Status::Ended,
            crate::adapter_protocol::Status::Failed,
            crate::adapter_protocol::Status::Lost,
        ] {
            cx.update(|window, cx| {
                app.update(cx, |app, cx| {
                    let session = app.adapter.state.session.as_mut().unwrap();
                    session.status = inactive;
                    cx.notify();
                });
                window.draw(cx).clear(cx);
            });
            assert!(
                cx.debug_bounds("session-header-progress").is_none(),
                "{inactive:?}"
            );
            let status = cx.debug_bounds("session-header-status").unwrap();
            let actions = app.read_with(&cx, |app, _| app.adapter.action_bounds.get());
            assert!(status.size.width > reading_status_width, "{inactive:?}");
            assert!(status.right() <= actions.left());
        }

        // Replay frame counts remain useful after decoding has ended.
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                let session = app.adapter.state.session.as_mut().unwrap();
                *session.source = crate::adapter_protocol::Source::Replay {
                    recording: "test-data/zview-bench-60s/capture.ndjson.gz".into(),
                };
                session.status = crate::adapter_protocol::Status::Ended;
                cx.notify();
            });
            window.draw(cx).clear(cx);
        });
        assert!(cx.debug_bounds("session-header-progress").is_some());
    }
}
