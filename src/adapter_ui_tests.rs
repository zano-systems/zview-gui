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
