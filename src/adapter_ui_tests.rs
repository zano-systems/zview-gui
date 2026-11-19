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
