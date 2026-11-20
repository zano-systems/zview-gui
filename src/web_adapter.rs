//! Shared adapter session state and controls, with platform-specific transports.
use super::*;
use crate::adapter_protocol::{
    ClientState, Connection, Envelope, Event, LiveView, Mode, Status, ViewInterest,
};
#[cfg(not(target_family = "wasm"))]
use crate::native_adapter::Request;
#[cfg(target_family = "wasm")]
use futures_util::StreamExt;
#[cfg(target_family = "wasm")]
use gloo_net::{
    http::Request,
    websocket::{Message, futures::WebSocket},
};
use serde_json::json;
#[cfg(target_family = "wasm")]
use wasm_bindgen::prelude::*;
#[cfg(target_family = "wasm")]
use wasm_bindgen_futures::spawn_local;

#[cfg(target_family = "wasm")]
#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = ["globalThis", "crypto"], js_name = randomUUID)]
    fn browser_client_id() -> String;
}

#[cfg(not(target_family = "wasm"))]
#[path = "native_session.rs"]
mod native;
#[cfg(not(target_family = "wasm"))]
#[path = "native_core.rs"]
pub(super) mod native_core;
#[path = "web_session.rs"]
mod session_ui;
#[cfg(all(test, not(target_family = "wasm")))]
#[path = "adapter_ui_tests.rs"]
mod tests;
#[path = "web_usage.rs"]
mod usage_ui;

async fn adapter_delay(milliseconds: u64) {
    #[cfg(target_family = "wasm")]
    gloo_timers::future::TimeoutFuture::new(milliseconds as u32).await;
    #[cfg(not(target_family = "wasm"))]
    smol::Timer::after(std::time::Duration::from_millis(milliseconds)).await;
}

/// Browser callbacks retain their existing JS scheduling and host access.
/// Native tasks instead carry a weak view and GPUI's async application context.
struct SessionTask {
    #[cfg(not(target_family = "wasm"))]
    view: WeakEntity<ThreadsApp>,
    #[cfg(not(target_family = "wasm"))]
    app: AsyncApp,
}

impl SessionTask {
    fn update(
        &mut self,
        update: impl FnOnce(&mut ThreadsApp, &mut Window, &mut Context<ThreadsApp>),
    ) -> Result<(), String> {
        #[cfg(target_family = "wasm")]
        return web::update_view(update).map_err(|e| format!("{e:?}"));
        #[cfg(not(target_family = "wasm"))]
        self.view
            .update_in(&mut self.app, update)
            .map_err(|e| e.to_string())
    }
}

fn spawn_session_task<F: std::future::Future<Output = ()> + 'static>(
    cx: &mut Context<ThreadsApp>,
    task: impl FnOnce(SessionTask) -> F + 'static,
) {
    #[cfg(target_family = "wasm")]
    {
        let _ = cx;
        spawn_local(task(SessionTask {}));
    }
    #[cfg(not(target_family = "wasm"))]
    cx.spawn(async move |view, cx| {
        task(SessionTask {
            view,
            app: cx.clone(),
        })
        .await;
    })
    .detach();
}

struct AcquisitionWarningNotification;

pub(super) struct Controls {
    pub available: bool,
    pub state: ClientState,
    displayed: Option<(String, u64)>,
    history_view: Option<LiveView>,
    history_screen: Option<Screen>,
    last_frame_received: Option<(web_time::Instant, f64)>,
    client_id: Option<String>,
    view_interest: ViewInterest,
    usage: usage_ui::UsageInput,
    action_bounds: std::rc::Rc<std::cell::Cell<Bounds<Pixels>>>,
    panel_bounds: std::rc::Rc<std::cell::Cell<Bounds<Pixels>>>,
    session_toggle_bounds: std::rc::Rc<std::cell::Cell<Bounds<Pixels>>>,
    pub(super) sidebar_toggle_bounds: std::rc::Rc<std::cell::Cell<Bounds<Pixels>>>,
    pub(super) backing_bounds: std::rc::Rc<std::cell::Cell<Bounds<Pixels>>>,
    pub(super) about_bounds: std::rc::Rc<std::cell::Cell<Bounds<Pixels>>>,
    pub(super) help_bounds: std::rc::Rc<std::cell::Cell<Bounds<Pixels>>>,
    pub(super) session_focus: FocusHandle,
    draft: session_ui::Draft,
    _draft_subscriptions: Vec<Subscription>,
    #[cfg(not(target_family = "wasm"))]
    endpoint: String,
    #[cfg(not(target_family = "wasm"))]
    pub(super) connection_task: Option<Task<()>>,
    #[cfg(not(target_family = "wasm"))]
    pub(super) process: Option<crate::native_adapter::OwnedAdapter>,
    #[cfg(not(target_family = "wasm"))]
    pub(super) startup: Option<crate::native_adapter::Startup>,
    #[cfg(not(target_family = "wasm"))]
    pub(super) rust_core: Option<native_core::Backend>,
    #[cfg(not(target_family = "wasm"))]
    pub(super) closing: bool,
    #[cfg(not(target_family = "wasm"))]
    pub(super) shutdown: Option<crate::native_adapter::Shutdown>,
}

impl Controls {
    pub fn new(window: &mut Window, cx: &mut Context<ThreadsApp>) -> Self {
        let draft = session_ui::Draft::new(window, cx);
        let subscriptions = draft
            .inputs()
            .into_iter()
            .map(|input| {
                cx.subscribe_in(&input, window, |app, _, _: &InputEvent, window, cx| {
                    app.session_draft_changed(window, cx);
                })
            })
            .collect();
        Self {
            available: false,
            state: ClientState::default(),
            displayed: None,
            history_view: None,
            history_screen: None,
            last_frame_received: None,
            client_id: None,
            view_interest: ViewInterest::default(),
            usage: usage_ui::UsageInput::new(window, cx),
            action_bounds: Default::default(),
            panel_bounds: Default::default(),
            session_toggle_bounds: Default::default(),
            sidebar_toggle_bounds: Default::default(),
            backing_bounds: Default::default(),
            about_bounds: Default::default(),
            help_bounds: Default::default(),
            session_focus: cx.focus_handle().tab_stop(true),
            draft,
            _draft_subscriptions: subscriptions,
            #[cfg(not(target_family = "wasm"))]
            endpoint: String::new(),
            #[cfg(not(target_family = "wasm"))]
            connection_task: None,
            #[cfg(not(target_family = "wasm"))]
            process: None,
            #[cfg(not(target_family = "wasm"))]
            startup: None,
            #[cfg(not(target_family = "wasm"))]
            rust_core: None,
            #[cfg(not(target_family = "wasm"))]
            closing: false,
            #[cfg(not(target_family = "wasm"))]
            shutdown: None,
        }
    }
}

impl Controls {
    fn uses_rust_core(&self) -> bool {
        #[cfg(not(target_family = "wasm"))]
        return self.rust_core.is_some();
        #[cfg(target_family = "wasm")]
        false
    }

    fn fresh_frame(&self) -> bool {
        let same_session = self.state.session.as_ref().is_some_and(|session| {
            self.displayed
                .as_ref()
                .is_some_and(|(id, _)| *id == session.id)
        });
        same_session
            && self.last_frame_received.is_some_and(|(received, age)| {
                age.is_finite() && age + received.elapsed().as_secs_f64() < 5.
            })
    }

    fn indicator(&self) -> crate::adapter_protocol::SessionIndicator {
        self.state.indicator(self.fresh_frame())
    }
}

impl ThreadsApp {
    fn adapter_url(&self, path: &str) -> String {
        #[cfg(target_family = "wasm")]
        return path.to_owned();
        #[cfg(not(target_family = "wasm"))]
        format!("{}{path}", self.adapter.endpoint)
    }

    fn set_usage_window(&mut self, polls: u32, cx: &mut Context<Self>) {
        if let (Some(session), Some(client_id)) =
            (&self.adapter.state.session, &self.adapter.client_id)
        {
            self.adapter_command(
                "/api/session/usage-window",
                json!({"session_id": session.id, "client_id": client_id, "polls": polls}),
                cx,
            );
        }
    }

    fn desired_live_view(&self, cx: &Context<Self>) -> LiveView {
        let label = self.heap.read(cx).selected_value();
        let heap = self
            .adapter
            .state
            .session
            .as_ref()
            .and_then(|s| s.inventory.as_ref())
            .and_then(|inventory| inventory.get("heaps"))
            .and_then(|rows| {
                rows.iter()
                    .find(|row| Some(&format!("{} · {:#x}", row.name, row.address)) == label)
            })
            .cloned();
        self.screen.live_view(
            heap,
            self.object_kind,
            self.object_search.read(cx).value().to_string(),
        )
    }

    pub(crate) fn live_view_loading(&self, cx: &Context<Self>) -> bool {
        let Some(session) = self.adapter.state.session.as_ref() else {
            return false;
        };
        if self.adapter.uses_rust_core() {
            return session.mode() == Mode::Live
                && session.status.is_active()
                && session.last_frame.is_none();
        }
        if session.mode() != Mode::Live
            || session.status != Status::Reading
            || session.inventory.is_none()
        {
            return false;
        }
        if self.live_history.is_empty() {
            return true;
        }
        let Some(view) = &session.live_view else {
            return true;
        };
        view.owner.as_ref() != self.adapter.client_id.as_ref()
            || session.last_frame.as_ref().is_none_or(|frame| {
                frame.view_revision != view.revision
                    || frame.view.as_ref() != Some(&self.desired_live_view(cx))
            })
    }

    pub(crate) fn live_view_loading_note(&self) -> String {
        if self.adapter.uses_rust_core() {
            return "Waiting for the first Rust GDB sample…".into();
        }
        if self
            .adapter
            .state
            .session
            .as_ref()
            .and_then(|s| s.live_view.as_ref())
            .is_some_and(|view| {
                view.owner.is_some() && view.owner.as_ref() != self.adapter.client_id.as_ref()
            })
        {
            return "Another tab controls Live acquisition. Close it before controlling this view."
                .into();
        }
        self.adapter
            .view_interest
            .error
            .clone()
            .unwrap_or_else(|| "Reading the selected view…".into())
    }

    /// The current screen owns one selection and optional detail request.
    pub(crate) fn sync_live_view(&mut self, cx: &mut Context<Self>) {
        if self.adapter.uses_rust_core() {
            if self.adapter.state.is_live()
                && self.adapter.history_screen.as_ref() != Some(&self.screen)
            {
                self.live_history.clear();
                self.adapter.history_screen = Some(self.screen.clone());
            }
            return;
        }
        let binding = self
            .adapter
            .state
            .session
            .as_ref()
            .filter(|session| {
                session.mode() == Mode::Live
                    && session.status == Status::Reading
                    && self.adapter.state.connection == Connection::Connected
            })
            .map(|session| session.id.as_str());
        self.adapter.view_interest.bind(binding);
        if self.adapter.state.is_live() {
            let desired = self.desired_live_view(cx);
            if self.adapter.history_screen.as_ref() != Some(&self.screen) {
                self.live_history.clear();
                self.adapter.history_screen = Some(self.screen.clone());
            }
            self.adapter.history_view = Some(desired);
        }
        let Some(session) = self.adapter.state.session.as_ref() else {
            return;
        };
        if session.mode() != Mode::Live
            || session.status != Status::Reading
            || self.adapter.state.connection != Connection::Connected
            || session.inventory.is_none()
        {
            return;
        }
        let Some(client_id) = self.adapter.client_id.clone() else {
            return;
        };
        if session
            .live_view
            .as_ref()
            .is_some_and(|view| view.owner.is_some() && view.owner.as_ref() != Some(&client_id))
        {
            return;
        }
        let view = self.desired_live_view(cx);
        let Some(epoch) = self.adapter.view_interest.begin(
            Some(view.clone()),
            session
                .live_view
                .as_ref()
                .and_then(|state| state.active_view()),
        ) else {
            return;
        };
        let session_id = session.id.clone();
        let url = self.adapter_url("/api/session/view");
        spawn_session_task(cx, async move |mut host| {
            let result =
                async {
                    let response = Request::post(&url)
                    .json(&json!({"session_id": session_id, "client_id": client_id, "view": view}))
                    .map_err(|e| e.to_string())?.send().await.map_err(|e| e.to_string())?;
                    if response.ok() {
                        return Ok((None, false));
                    }
                    let text = response.text().await.map_err(|e| e.to_string())?;
                    let data = serde_json::from_str::<serde_json::Value>(&text).ok();
                    let conflict = data.as_ref().is_some_and(|v| v["code"] == "view_busy");
                    let error = data
                        .and_then(|v| v["error"].as_str().map(str::to_owned))
                        .unwrap_or(text);
                    Ok((Some(error), conflict))
                }
                .await;
            let _ = host.update(|this, _, cx| {
                let (error, conflict) = result.unwrap_or_else(|e| (Some(e), false));
                this.adapter.view_interest.finish(epoch, error, conflict);
                cx.notify();
            });
        });
    }

    pub(crate) fn live_heap_map(
        &self,
        key: &model::HeapKey,
    ) -> Option<&crate::adapter_protocol::CapturedHeapMap> {
        let session = self.adapter.state.session.as_ref()?;
        if session.mode() != Mode::Live
            || self
                .adapter
                .displayed
                .as_ref()
                .is_none_or(|(id, _)| *id != session.id)
            || key.in_frame(&self.frames[self.frame]).is_none()
        {
            return None;
        }
        if session.status == Status::Reading
            && self.adapter.state.connection == Connection::Connected
            && session.live_view.as_ref().is_none_or(|view| {
                view.owner.as_ref() != self.adapter.client_id.as_ref()
                    || session
                        .last_frame
                        .as_ref()
                        .is_none_or(|frame| frame.view_revision != view.revision)
            })
        {
            return None;
        }
        if !matches!(session.last_frame.as_ref().and_then(|frame| frame.view.as_ref()),
            Some(LiveView::Detail { group, object })
                if group == "heaps" && object.address == key.address && object.name == key.name)
        {
            return None;
        }
        let origin = self.live_history.origin()?;
        session.heap_detail.last_map.as_ref().filter(|map| {
            map.heap.address == key.address
                && map.heap.name == key.name
                && map.live_elapsed >= origin
        })
    }

    pub(crate) fn live_heap_note(&self, key: &model::HeapKey) -> Option<String> {
        let session = self.adapter.state.session.as_ref()?;
        if session.mode() != Mode::Live {
            return None;
        }
        if self.adapter.uses_rust_core() {
            return Some(
                "Live heap maps are not supported by the experimental Rust backend.".into(),
            );
        }
        if self.adapter.state.connection != Connection::Connected
            || session.status != Status::Reading
        {
            return Some("Retained heap map · acquisition is not updating this view.".into());
        }
        if let Some(error) = &self.adapter.view_interest.error {
            return Some(error.clone());
        }
        let detail = &session.heap_detail;
        if let Some(heap) = &detail.heap
            && (heap.address != key.address || heap.name != key.name)
        {
            return Some(
                "Another tab is inspecting a different heap. Close that heap detail first.".into(),
            );
        }
        if let Some(error) = &detail.error {
            return Some(error.clone());
        }
        if let Some(map) = self.live_heap_map(key) {
            return Some(format!(
                "Last captured map at {:.2}s since opening this view · {} Read time limits the achieved rate.",
                map.live_elapsed - self.live_history.origin().unwrap_or(map.live_elapsed),
                session
                    .timing
                    .as_ref()
                    .map(|timing| timing.heap_label())
                    .unwrap_or_else(|| "Requested map timing unavailable.".into()),
            ));
        }
        Some("Collecting heap map…".into())
    }

    fn stop_adapter(&mut self, cx: &mut Context<Self>) {
        if self.adapter.state.can_stop()
            && let Some(session) = &self.adapter.state.session
        {
            self.adapter_command("/api/session/stop", json!({"session_id": session.id}), cx);
        }
    }

    fn reconnect_adapter(&mut self, cx: &mut Context<Self>) {
        if self.adapter.state.can_reconnect()
            && let Some(session) = &self.adapter.state.session
        {
            self.adapter_command(
                "/api/session/reconnect",
                json!({"session_id": session.id}),
                cx,
            );
        }
    }

    fn adapter_command(
        &mut self,
        path: &'static str,
        body: serde_json::Value,
        cx: &mut Context<Self>,
    ) {
        self.adapter.state.pending = true;
        self.error = None;
        cx.notify();
        #[cfg(not(target_family = "wasm"))]
        if self.adapter.uses_rust_core() {
            spawn_session_task(cx, async move |mut host| {
                let _ = host.update(|this, window, cx| {
                    this.adapter.state.pending = false;
                    let result = this.rust_command(path, body, window, cx);
                    this.finish_adapter_command(path, result, window, cx);
                });
            });
            return;
        }
        let url = self.adapter_url(path);
        spawn_session_task(cx, async move |mut host| {
            let result = async {
                let response = Request::post(&url)
                    .json(&body)
                    .map_err(|error| error.to_string())?
                    .send()
                    .await
                    .map_err(|error| error.to_string())?;
                if response.ok() {
                    return Ok(());
                }
                let text = response.text().await.map_err(|error| error.to_string())?;
                Err(serde_json::from_str::<serde_json::Value>(&text)
                    .ok()
                    .and_then(|value| value["error"].as_str().map(str::to_owned))
                    .unwrap_or(text))
            }
            .await;
            let _ = host.update(|this, window, cx| {
                this.adapter.state.pending = false;
                this.finish_adapter_command(path, result, window, cx);
            });
        });
    }

    fn finish_adapter_command(
        &mut self,
        path: &str,
        result: Result<(), String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Err(error) = result {
            self.adapter.draft.error = Some(error.clone());
            self.error = Some(error);
            if path == "/api/session/usage-window" {
                self.adapter.usage.restore(window, cx);
            }
        } else if !matches!(path, "/api/session/stop" | "/api/session/usage-window")
            && self.adapter.draft.open
        {
            self.adapter.draft.dismiss();
            window.close_dialog(cx);
        }
        // Command acknowledgement never replaces state; only events do.
        cx.notify();
    }

    fn accept_adapter(
        &mut self,
        message: Envelope,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let notice = match &message.event {
            Event::Notice { message }
                if self
                    .adapter
                    .state
                    .session
                    .as_ref()
                    .and_then(|session| session.last_notice.as_ref())
                    != Some(message) =>
            {
                Some(message.clone())
            }
            _ => None,
        };
        let interrupted = matches!(
            &message.event,
            Event::Notice { .. } | Event::View(_) | Event::Status(_) | Event::UsageWindow { .. }
        );
        if !self.adapter.state.accept(message)? {
            return Ok(());
        }
        if let Some(notice) = notice {
            window.push_notification(
                Notification::warning(notice)
                    .id::<AcquisitionWarningNotification>()
                    .title("Acquisition warning")
                    .placement(Anchor::TopRight)
                    .autohide(true)
                    .delivery(NotificationDelivery::InApp),
                cx,
            );
        }
        self.adapter
            .usage
            .sync(self.adapter.state.session.as_ref(), window, cx);
        if interrupted {
            self.live_history.interrupt();
        }
        if let Some(session) = &self.adapter.state.session {
            let key = (session.id.clone(), session.frames_received);
            let is_live = session.mode() == Mode::Live;
            if let Some(frame) = &session.last_frame
                && self.adapter.displayed.as_ref() != Some(&key)
            {
                // Reuse the shared model, its validations and all existing views.
                // Parse the measurements first: a reconnect snapshot may start
                // after zero. Only Live gets monotonic receipt-time chart axes.
                let text = serde_json::to_string(&frame.groups).map_err(|e| e.to_string())?;
                let mut frames = model::parse(&text)?;
                if session.mode() == Mode::Live {
                    frames[0].elapsed_seconds = frame.live_elapsed;
                    frames[0].is_live = true;
                }
                let new_session =
                    self.adapter.displayed.as_ref().map(|key| &key.0) != Some(&session.id);
                let source = PathBuf::from(session.source_label());
                let elf = PathBuf::from(&session.elf);
                // Snapshot recovery must not make old measurements look fresh.
                let source_age = web_time::SystemTime::now()
                    .duration_since(web_time::UNIX_EPOCH)
                    .map(|now| (now.as_secs_f64() - frame.decoded_at).max(0.))
                    .unwrap_or(f64::NAN);
                let received_view = frame.view.clone();
                let overview = matches!(&received_view, None | Some(LiveView::Overview { .. }));
                if new_session {
                    let recording_elf = (session.mode() == Mode::Replay).then_some(elf);
                    self.install(frames, source, recording_elf, window, cx);
                    self.adapter.history_view = None;
                    self.adapter.history_screen = None;
                } else if is_live {
                    self.pause(cx);
                    self.frames = frames;
                    self.frame = 0;
                    self.sync_heaps(window, cx);
                } else {
                    self.pause(cx);
                    if frames[0].elapsed_seconds.is_some_and(|time| {
                        self.frames
                            .last()
                            .and_then(|f| f.elapsed_seconds)
                            .is_some_and(|last| time <= last)
                    }) {
                        return Err("Live receipt times must increase within a session.".into());
                    }
                    self.frames.append(&mut frames);
                    if self.frames.len() > 256 {
                        self.frames.remove(0);
                    }
                    self.frame = self.frames.len() - 1;
                    self.sync_heaps(window, cx);
                }
                if is_live && self.adapter.uses_rust_core() {
                    self.sync_live_view(cx);
                    use crate::live_history::Key;
                    self.live_history.record_where(
                        &self.frames[self.frame],
                        self.screen == Screen::Overview,
                        |key| match (&self.screen, key) {
                            (Screen::Overview, Key::Heap(_)) => true,
                            (Screen::Thread(a), Key::Thread(b)) => a == b,
                            (Screen::Heap(a), Key::Heap(b)) => a == b,
                            (Screen::Semaphore(a), Key::Semaphore(b)) => a == b,
                            (Screen::Mutex(a), Key::Mutex(b)) => a == b,
                            (Screen::MessageQueue(a), Key::MessageQueue(b)) => a == b,
                            (Screen::MemorySlab(a), Key::Slab(b)) => a == b,
                            (Screen::WorkQueue(a), Key::WorkQueue(b)) => a == b,
                            _ => false,
                        },
                    );
                } else if is_live
                    && self.adapter.history_view.is_some()
                    && self.adapter.history_view == received_view
                {
                    self.live_history.record(&self.frames[self.frame], overview);
                }
                self.adapter.last_frame_received = Some((web_time::Instant::now(), source_age));
                self.adapter.displayed = Some(key);
            }
        }
        self.error = self.adapter.state.error.clone();
        cx.notify();
        Ok(())
    }

    pub(crate) fn adapter_display_notice(&self) -> Option<AnyElement> {
        let session = self.adapter.state.session.as_ref()?;
        let previous = self
            .adapter
            .displayed
            .as_ref()
            .is_none_or(|(id, _)| *id != session.id);
        let text = if self.adapter.state.connection != Connection::Connected {
            "Adapter disconnected · displayed measurements are retained, not confirmed current."
                .to_owned()
        } else if previous {
            "Showing previous data until the selected session provides its first valid frame."
                .to_owned()
        } else if session.mode() == Mode::Live
            && session.status != crate::adapter_protocol::Status::Reading
        {
            format!("Retained live measurements · {}", session.status_label())
        } else {
            return None;
        };
        Some(layout::note("adapter-display-notice", text).into_any_element())
    }
}

/// Called once by the browser host. The adapter is required; connection/API
/// failures remain visible and retry instead of selecting another data source.
#[cfg(target_family = "wasm")]
#[wasm_bindgen]
pub fn connect_adapter() {
    spawn_local(async {
        // Freshness changes even when neither frames nor health events arrive.
        // This is one UI-only timer, never another acquisition loop.
        spawn_local(async {
            loop {
                adapter_delay(1000).await;
                if web::update_view(|this, _, cx| {
                    if this
                        .adapter
                        .state
                        .session
                        .as_ref()
                        .is_some_and(|s| s.status == crate::adapter_protocol::Status::Reading)
                    {
                        cx.notify();
                    }
                })
                .is_err()
                {
                    break;
                }
            }
        });
        let location = web_sys::window().expect("browser window").location();
        let scheme = if location.protocol().unwrap_or_default() == "https:" {
            "wss"
        } else {
            "ws"
        };
        let url = format!(
            "{scheme}://{}/api/events",
            location.host().unwrap_or_default()
        );
        loop {
            let failure = match Request::get("/api/session").send().await {
                Ok(response) if response.ok() => None,
                Ok(response) => Some(format!(
                    "The required ZView adapter is unavailable (HTTP {}). Start it with just web-gui or zview-gui. Retrying…",
                    response.status()
                )),
                Err(error) => Some(format!(
                    "Cannot reach the required ZView adapter: {error}. Retrying…"
                )),
            };
            if let Some(error) = failure {
                if web::update_view(|this, _, cx| {
                    this.adapter.state.connection = Connection::Retrying;
                    this.error = Some(error);
                    cx.notify();
                })
                .is_err()
                {
                    break;
                }
                adapter_delay(1500).await;
                continue;
            }
            let client_id = browser_client_id();
            let _ = web::update_view(|this, _, _| {
                this.adapter.client_id = Some(client_id.clone());
                this.adapter.view_interest.reset();
            });
            if let Ok(mut socket) = WebSocket::open(&format!("{url}?client_id={client_id}")) {
                let mut needs_snapshot = true;
                while let Some(message) = socket.next().await {
                    let result = match message {
                        Ok(Message::Text(text)) => serde_json::from_str::<Envelope>(&text)
                            .map_err(|error| error.to_string())
                            .and_then(|message| {
                                if needs_snapshot && !matches!(message.event, Event::Snapshot(_)) {
                                    return Err("Expected adapter snapshot.".into());
                                }
                                let mut accepted = Ok(());
                                web::update_view(|this, window, cx| {
                                    accepted = this.accept_adapter(message, window, cx);
                                })
                                .map_err(|_| "Browser window is unavailable.".to_owned())?;
                                accepted
                            }),
                        Ok(Message::Bytes(_)) => Err("Expected JSON text from adapter.".into()),
                        Err(_) => break,
                    };
                    if let Err(error) = result {
                        let _ = web::update_view(|this, _, cx| {
                            this.error = Some(format!("Adapter protocol error: {error}"));
                            cx.notify();
                        });
                        break;
                    }
                    needs_snapshot = false;
                }
            }
            let _ = web::update_view(|this, _, cx| {
                this.adapter.state.connection = Connection::Retrying;
                this.adapter.client_id = None;
                this.adapter.view_interest.reset();
                this.live_history.interrupt();
                this.error.get_or_insert_with(|| {
                    "The required ZView adapter disconnected. Retrying…".into()
                });
                cx.notify();
            });
            adapter_delay(1500).await;
        }
    });
}

/// Use the same Rust command path as the GPUI controls; useful to browser hosts.
#[cfg(target_family = "wasm")]
#[wasm_bindgen]
pub fn adapter_open_replay(recording: &str, elf: &str) -> Result<(), JsValue> {
    web::update_view(|this, _, cx| {
        if this.adapter.state.can_open() {
            this.adapter_command(
                "/api/session/replay",
                json!({
                    "recording": recording, "elf": elf,
                }),
                cx,
            );
        }
    })
}

#[cfg(target_family = "wasm")]
#[wasm_bindgen]
pub fn adapter_stop() -> Result<(), JsValue> {
    web::update_view(|this, _, cx| this.stop_adapter(cx))
}

#[cfg(target_family = "wasm")]
#[wasm_bindgen]
pub fn adapter_reconnect() -> Result<(), JsValue> {
    web::update_view(|this, _, cx| this.reconnect_adapter(cx))
}

/// Read-only inspection of Rust state, including measurements in the shared UI.
/// Canvas rendering has no DOM table for browser tests to inspect.
#[cfg(target_family = "wasm")]
#[wasm_bindgen]
pub fn adapter_state() -> Result<String, JsValue> {
    let mut state = serde_json::Value::Null;
    web::update_view(|this, window, cx| {
        let adapter = &this.adapter;
        state = json!({
            "available": adapter.available,
            "screen": format!("{:?}", this.screen),
            "connected": adapter.state.connection == Connection::Connected,
            "status": adapter.state.session.as_ref().map(|s| s.status.as_str()),
            "mode": adapter.state.session.as_ref().map(|s| s.mode()),
            "samples_per_second": adapter.state.session.as_ref().and_then(|s| s.samples_per_second),
            "timing": adapter.state.session.as_ref().and_then(|s| s.timing.as_ref()),
            "live_elapsed": this.frames[this.frame].is_live.then_some(this.frames[this.frame].elapsed_seconds),
            "displayed_session": adapter.displayed.as_ref().map(|(id, _)| id),
            "session_id": adapter.state.session.as_ref().map(|s| &s.id),
            "frames_received": adapter.state.session.as_ref().map(|s| s.frames_received),
            "buffered_frames": this.frames.len(),
            "history_changes": this.live_history.changes(),
            "history_bytes": this.live_history.bytes(),
            "history_seconds": this.live_history.retained_seconds(),
            "history_label": this.live_history.label(),
            "heap_capture": adapter.state.session.as_ref().and_then(|s| s.heap_detail.heap.as_ref()),
            "heap_map_chunks": adapter.state.session.as_ref().and_then(|s| s.heap_detail.last_map.as_ref()).map(|m| m.chunks.len()),
            "displayed_heap_map_chunks": match &this.screen {
                Screen::Heap(key) => this.live_heap_map(key).map(|map| map.chunks.len()),
                _ => None,
            },
            "heap_map_frame": adapter.state.session.as_ref().and_then(|s| s.heap_detail.last_map.as_ref()).map(|m| m.frame_number),
            "heap_map_note": match &this.screen { Screen::Heap(key) => this.live_heap_note(key), _ => None },
            "error": this.error,
            "visible_threads": this.frames[this.frame].visible_threads().len(),
            "cpu": this.frames[this.frame].cpu(),
            "source": this.source,
            "can_open": adapter.state.can_open(),
            "can_stop": adapter.state.can_stop(),
            "can_reconnect": adapter.state.can_reconnect(),
            "primary_action": adapter.state.primary_action().label(),
            "primary_enabled": adapter.state.can_primary_action(),
            "indicator": adapter.indicator().as_str(),
            "sidebar_collapsed": this.collapsed,
            "session_labels": crate::adapter_protocol::session_labels(
                adapter.state.session.as_ref(), adapter.state.connection),
            "context": adapter.state.session.as_ref().and_then(|s| s.context.clone()),
            "dialog_open": adapter.draft.open,
            "draft_mode": adapter.draft.mode,
            "draft_error": adapter.draft.error,
            "build_context": adapter.draft.context(cx),
            "draft_request": adapter.draft.open.then(|| adapter.draft.request(cx)),
            "draft_recording": adapter.draft.recording_path(cx),
            "native_picker": adapter.draft.picker,
        });
        state["sidebar_width"] = json!(this.sidebar_width().as_f32());
        state["navigation_width"] = json!(this.navigation_width().as_f32());
        state["modal_open"] = json!(window.has_active_dialog(cx));
        state["backing_focused"] = json!(this.backing_focus.is_focused(window));
        state["session_toggle_focused"] =
            json!(this.collapsed && adapter.session_focus.is_focused(window));
        state["session_panel_focused"] =
            json!(!this.collapsed && adapter.session_focus.is_focused(window));
        state["status_shimmer"] = json!(
            adapter
                .state
                .animate_live_status(adapter.fresh_frame(), cx.reduce_motion())
        );
        state["reduced_motion"] = json!(cx.reduce_motion());
        state["history_origin"] = json!(this.live_history.origin());
        state["history_view"] = json!(adapter.history_view);
        let actions = adapter.action_bounds.get();
        state["session_action_bounds"] = json!({
            "x": actions.left().as_f32(), "y": actions.top().as_f32(),
            "width": actions.size.width.as_f32(), "height": actions.size.height.as_f32(),
        });
        for (name, bounds) in [
            ("session_panel_bounds", adapter.panel_bounds.get()),
            ("session_toggle_bounds", adapter.session_toggle_bounds.get()),
            ("session_mode_bounds", adapter.draft.mode_bounds.get()),
            ("sidebar_toggle_bounds", adapter.sidebar_toggle_bounds.get()),
            ("backing_bounds", adapter.backing_bounds.get()),
            ("about_bounds", adapter.about_bounds.get()),
            ("help_bounds", adapter.help_bounds.get()),
        ] {
            state[name] = json!({
                "x": bounds.left().as_f32(), "y": bounds.top().as_f32(),
                "width": bounds.size.width.as_f32(), "height": bounds.size.height.as_f32(),
            });
        }
        state["history_percent"] = json!(this.live_history.memory_percent());
        state["history_memory_label"] = json!(this.live_history.memory_label());
        state["history_retention_label"] = json!(this.live_history.retention_label());
        state["usage_window"] = json!(adapter.state.session.as_ref().and_then(|s| s.usage_window));
        state["usage_input"] = json!(adapter.usage.state.read(cx).value().to_string());
        let input = adapter.usage.state.read(cx).input_bounds();
        state["usage_input_bounds"] = json!({
            "x": input.left().as_f32(), "y": input.top().as_f32(),
            "width": input.size.width.as_f32(), "height": input.size.height.as_f32(),
        });
        state["requested_usage_window"] = json!(
            adapter
                .state
                .session
                .as_ref()
                .and_then(|s| s.live_view.as_ref())
                .map(|v| v.usage_window)
        );
        state["draft_advanced"] = json!(adapter.draft.advanced);
        state["view_loading"] = json!(this.live_view_loading(cx));
        state["view_error"] = json!(adapter.view_interest.error);
        state["requested_view"] = json!(this.desired_live_view(cx));
        state["sampled_view"] = json!(
            adapter
                .state
                .session
                .as_ref()
                .and_then(|s| s.last_frame.as_ref())
                .and_then(|f| f.view.as_ref())
        );
        state["sampled_groups"] = json!(
            adapter
                .state
                .session
                .as_ref()
                .and_then(|s| s.last_frame.as_ref())
                .map(|f| &f.sampled_groups)
        );
        state["controls_view"] = json!(
            adapter
                .state
                .session
                .as_ref()
                .and_then(|s| s.live_view.as_ref())
                .is_some_and(|view| view.owner.as_ref() == adapter.client_id.as_ref())
        );
    })?;
    Ok(state.to_string())
}
