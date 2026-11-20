//! Session configuration stays a draft until an HTTP command is accepted.
//! This child of web_adapter uses its transport, not a second session owner.
use super::*;
use crate::adapter_protocol::{
    BuildContext, Connection, Mode, SessionAction, SessionIndicator, Source, session_labels,
};
use crate::session_settings::AdvancedValues;
use gpui_kit::component::collapsible::Collapsible;
use gpui_kit::component::input::Input;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::tag::Tag;

#[path = "web_native_picker.rs"]
mod picker;

pub(super) struct Draft {
    pub open: bool,
    pub mode: Mode,
    pub error: Option<String>,
    pub resolved: Option<(serde_json::Value, BuildContext)>,
    generation: u64,
    resolving: bool,
    resolution_revision: u64,
    scheduled_request: Option<serde_json::Value>,
    pub(super) advanced: bool,
    detected: AdvancedValues,
    bound_build: String,
    build: Entity<InputState>,
    domain: Entity<InputState>,
    elf: Entity<InputState>,
    recording: Entity<InputState>,
    runner: Entity<InputState>,
    target: Entity<InputState>,
    clock: Entity<InputState>,
    pub(super) picker: Option<crate::adapter_protocol::PickerKind>,
    picker_revision: u64,
    mode_focus: FocusHandle,
    pub(super) mode_bounds: std::rc::Rc<std::cell::Cell<Bounds<Pixels>>>,
}

impl Draft {
    pub fn new(window: &mut Window, cx: &mut Context<ThreadsApp>) -> Self {
        let mode_focus = cx.focus_handle();
        let mut input =
            |placeholder| cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        Self {
            open: false,
            mode: Mode::Replay,
            error: None,
            resolved: None,
            generation: 0,
            resolving: false,
            resolution_revision: 0,
            scheduled_request: None,
            advanced: false,
            detected: AdvancedValues::default(),
            bound_build: String::new(),
            build: input("Build directory under the server data root"),
            domain: input("Default firmware image"),
            elf: input("Matching ELF, or leave empty to use the build"),
            recording: input("Recording under the server data root (.ndjson.gz)"),
            runner: input("Runner from the build"),
            target: input("MCU or GDB host:port from the build"),
            clock: input("Default probe clock (Hz)"),
            picker: None,
            picker_revision: 0,
            mode_focus,
            mode_bounds: Default::default(),
        }
    }

    pub fn inputs(&self) -> Vec<Entity<InputState>> {
        vec![
            self.build.clone(),
            self.domain.clone(),
            self.elf.clone(),
            self.recording.clone(),
            self.runner.clone(),
            self.target.clone(),
            self.clock.clone(),
        ]
    }

    pub fn dismiss(&mut self) {
        self.open = false;
        self.generation = self.generation.wrapping_add(1);
        self.resolving = false;
        self.resolution_revision = self.resolution_revision.wrapping_add(1);
        self.scheduled_request = None;
        self.resolved = None;
        self.error = None;
        self.picker = None;
        self.picker_revision = self.picker_revision.wrapping_add(1);
    }

    fn value(input: &Entity<InputState>, cx: &App) -> String {
        input.read(cx).value().trim().to_string()
    }

    pub(super) fn request(&self, cx: &App) -> serde_json::Value {
        self.advanced_values(cx)
            .request(self.mode, &Self::value(&self.build, cx), None)
    }

    fn advanced_inputs(&self) -> [Entity<InputState>; 5] {
        [
            self.domain.clone(),
            self.elf.clone(),
            self.runner.clone(),
            self.target.clone(),
            self.clock.clone(),
        ]
    }

    fn advanced_values(&self, cx: &App) -> AdvancedValues {
        AdvancedValues(self.advanced_inputs().map(|input| Self::value(&input, cx)))
    }

    fn set_advanced(&self, values: &AdvancedValues, window: &mut Window, cx: &mut App) {
        for (input, value) in self.advanced_inputs().into_iter().zip(&values.0) {
            input.update(cx, |input, cx| input.set_value(value.clone(), window, cx));
        }
    }

    fn detection_request(&self, cx: &App) -> serde_json::Value {
        self.advanced_values(cx).request(
            self.mode,
            &Self::value(&self.build, cx),
            (self.mode == Mode::Live).then_some(&self.detected),
        )
    }

    pub(super) fn context(&self, cx: &App) -> Option<&BuildContext> {
        self.resolved
            .as_ref()
            .and_then(|(request, context)| (*request == self.request(cx)).then_some(context))
    }

    fn capabilities<'a>(
        &self,
        session: Option<&'a crate::adapter_protocol::Session>,
        cx: &App,
    ) -> Option<&'a crate::adapter_protocol::Capabilities> {
        let session = session?;
        let Source::Live { configuration, .. } = session.source.as_ref() else {
            return None;
        };
        let context = session.context.as_ref()?;
        // Discovery describes the connected firmware, never an edited draft.
        crate::session_settings::matches_discovery(&self.request(cx), context, configuration)
            .then_some(session.capabilities.as_ref())
            .flatten()
    }

    fn can_open(&self, state: &ClientState, cx: &App) -> bool {
        self.picker.is_none()
            && state.can_open()
            && match self.mode {
                Mode::Live => !Self::value(&self.build, cx).is_empty(),
                Mode::Replay => {
                    !Self::value(&self.recording, cx).is_empty()
                        && (!Self::value(&self.elf, cx).is_empty()
                            || !Self::value(&self.build, cx).is_empty())
                }
            }
    }
}

fn field(label: &'static str, input: &Entity<InputState>) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .min_w_0()
        .child(div().text_xs().text_color(rgb(MUTED)).child(label))
        .child(Input::new(input).aria_label(label))
}

fn provenance_note(context: &BuildContext) -> impl IntoElement {
    let checkout = context
        .git
        .as_ref()
        .map(|git| {
            format!(
                "{} · {}{}",
                git.branch.as_deref().unwrap_or("detached HEAD"),
                git.commit.chars().take(8).collect::<String>(),
                if git.dirty { " · tracked changes" } else { "" }
            )
        })
        .unwrap_or_else(|| "Git unavailable".into());
    let tooltip = context.tooltip();
    div()
        .id("resolved-provenance")
        .debug_selector(|| "resolved-provenance".into())
        .text_xs()
        .text_color(rgb(MUTED))
        .min_w_0()
        .truncate()
        .child(format!(
            "{} · Checkout at resolution: {checkout}",
            context.project
        ))
        .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
}

impl ThreadsApp {
    fn reveal_session_controls(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.collapsed = false;
        // The same focus handle follows the button into the expanded panel.
        // Tab can then continue to Configure and the acquisition action.
        self.adapter.session_focus.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn adapter_session_controls(
        &self,
        window: &Window,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        if !self.adapter.available {
            return None;
        }
        if !self.collapsed {
            return Some(
                div()
                    .id("sidebar-session-panel")
                    .track_focus(&self.adapter.session_focus)
                    .tab_stop(true)
                    .aria_label("Session controls")
                    .w_full()
                    .min_w_0()
                    .bg(rgb(crate::BG))
                    .border_1()
                    .border_color(rgb(crate::BG))
                    .focus_visible(|style| style.border_color(rgb(PURPLE)))
                    .py_2()
                    .px_2()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .children(self.adapter_session_panel(window, cx))
                    .into_any_element(),
            );
        }
        let indicator = self.adapter.indicator();
        let label = format!(
            "Session: {}. {}. Expand sidebar to show session controls",
            indicator.as_str(),
            session_labels(
                self.adapter.state.session.as_ref(),
                self.adapter.state.connection
            )
            .join(". ")
        );
        let bounds = self.adapter.session_toggle_bounds.clone();
        Some(
            div()
                .id("sidebar-session-toggle")
                .debug_selector(|| "sidebar-session-toggle".into())
                .track_focus(&self.adapter.session_focus)
                .tab_stop(true)
                .role(Role::Button)
                .aria_label(label.clone())
                .size_6()
                .relative()
                .flex()
                .items_center()
                .justify_center()
                .rounded_md()
                .border_1()
                .border_color(rgb(BORDER))
                .hover(|style| style.bg(rgb(CARD)))
                .focus_visible(|style| style.border_color(rgb(PURPLE)))
                .cursor_pointer()
                .on_click(cx.listener(|this, _, window, cx| {
                    this.reveal_session_controls(window, cx);
                }))
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                    if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                        window.prevent_default();
                        cx.stop_propagation();
                        this.reveal_session_controls(window, cx);
                    }
                }))
                .tooltip(move |window, cx| Tooltip::new(label.clone()).build(window, cx))
                .child(
                    Icon::new(match indicator {
                        SessionIndicator::Healthy => assets::IconName::Activity,
                        SessionIndicator::Busy => assets::IconName::LoaderCircle,
                        SessionIndicator::Waiting
                        | SessionIndicator::Warning
                        | SessionIndicator::Failed => assets::IconName::CircleAlert,
                        SessionIndicator::Idle => assets::IconName::CircleDashed,
                    })
                    .text_color(rgb(match indicator {
                        SessionIndicator::Healthy => GREEN,
                        SessionIndicator::Busy => BLUE,
                        SessionIndicator::Waiting | SessionIndicator::Warning => 0xe0b76d,
                        SessionIndicator::Failed => 0xf08b9f,
                        SessionIndicator::Idle => MUTED,
                    })),
                )
                .child(
                    div().absolute().inset_0().child(
                        canvas(move |rect, _, _| bounds.set(rect), |_, _, _, _| {}).size_full(),
                    ),
                )
                .into_any_element(),
        )
    }

    fn adapter_session_panel(&self, window: &Window, cx: &Context<Self>) -> Option<AnyElement> {
        if !self.adapter.available {
            return None;
        }
        let state = &self.adapter.state;
        let indicator = self.adapter.indicator();
        let animate = state.animate_live_status(self.adapter.fresh_frame(), cx.reduce_motion());
        let mut labels = session_labels(state.session.as_ref(), state.connection);
        if state.is_live()
            && !self.adapter.fresh_frame()
            && state
                .session
                .as_ref()
                .is_some_and(|s| s.status == crate::adapter_protocol::Status::Reading)
        {
            labels[2] = "— samples/s".into();
        }
        let mode = if state.is_live() { "LIVE" } else { "REPLAY" };
        let status = if state.connection != Connection::Connected {
            state.connection.label().to_owned()
        } else if state.pending {
            "Updating session…".into()
        } else if indicator == SessionIndicator::Waiting {
            format!("{mode} · Waiting")
        } else if indicator == SessionIndicator::Warning {
            format!("{mode} · Warning")
        } else {
            labels[0].clone()
        };
        #[cfg(test)]
        let status_selector = format!("session-status-text:{status}");
        #[cfg(not(test))]
        let status_selector = format!("session-indicator-{}", indicator.as_str());
        let icon = match indicator {
            SessionIndicator::Busy => Spinner::new()
                .icon(assets::IconName::LoaderCircle)
                .small()
                .color(rgb(BLUE).into())
                .into_any_element(),
            SessionIndicator::Healthy => div()
                .size_2p5()
                .rounded_full()
                .bg(rgb(GREEN))
                .into_any_element(),
            SessionIndicator::Waiting | SessionIndicator::Warning => {
                Icon::new(assets::IconName::CircleAlert)
                    .small()
                    .text_color(rgb(0xe0b76d))
                    .into_any_element()
            }
            SessionIndicator::Failed => Icon::new(assets::IconName::CircleAlert)
                .small()
                .text_color(rgb(0xf08b9f))
                .into_any_element(),
            SessionIndicator::Idle => Icon::new(assets::IconName::CircleDashed)
                .small()
                .text_color(rgb(MUTED))
                .into_any_element(),
        };
        let action = state.primary_action();
        let action_icon = match action {
            SessionAction::Connect => assets::IconName::Plug,
            SessionAction::Disconnect => assets::IconName::Unplug,
            SessionAction::Reconnect => assets::IconName::RotateCw,
            SessionAction::OpenReplay => assets::IconName::Folder,
            SessionAction::StopReplay => assets::IconName::Pause,
        };
        Some(
            div()
                .id("session-header-controls")
                .debug_selector(|| "session-header-controls".into())
                .w_full()
                .min_w_0()
                .relative()
                // Reserve space for navigation even on short windows.
                .max_h(window.viewport_size().height * 0.45)
                .overflow_y_scroll()
                .p_2()
                .when(self.collapsed, |el| {
                    el.rounded_md()
                        .border_1()
                        .border_color(rgb(BORDER))
                        .bg(rgb(CARD))
                })
                .flex()
                .flex_col()
                .gap_3()
                .child(div().absolute().inset_0().child({
                    let bounds = self.adapter.panel_bounds.clone();
                    canvas(move |rect, _, _| bounds.set(rect), |_, _, _, _| {}).size_full()
                }))
                .child(
                    div()
                        .id("session-header-context")
                        .min_w_0()
                        .max_w_full()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .min_w_0()
                                .child(
                                    div()
                                        .id("session-status-indicator")
                                        .debug_selector(move || status_selector.clone())
                                        .flex_shrink_0()
                                        .aria_label(format!("Session status: {status}"))
                                        .child(icon),
                                )
                                .child(layout::note_with_shimmer(
                                    "session-header-status",
                                    status,
                                    animate,
                                    false,
                                ).flex_1())
                                .when(
                                    state.connection == Connection::Connected
                                        && state.session.as_ref().is_some_and(|session| {
                                            session.mode() == Mode::Replay
                                                || session.status == crate::adapter_protocol::Status::Reading
                                        }),
                                    |el| el.child(layout::note_with_shimmer(
                                        "session-header-progress", labels[2].clone(), animate, false,
                                    ).flex_shrink_1()),
                                )
                        .child(
                    div()
                        .relative()
                        .flex()
                        .items_center()
                        .flex_shrink_0()
                        .ml_auto()
                        .gap_2()
                        .child(
                            Button::new("session-configure")
                                .small()
                                .icon(Icon::new(assets::IconName::Settings))
                                .accessibility_label("Configure session")
                                .tooltip("Build, target, recording and connection settings")
                                .disabled(state.pending)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.open_session_dialog(window, cx)
                                })),
                        )
                        .child(
                            Button::new("session-primary-action")
                                .small()
                                .primary()
                                .icon(Icon::new(action_icon))
                                .accessibility_label(format!("{} session", action.label()))
                                .tooltip(match action {
                                    SessionAction::Connect => {
                                        "Configure and connect to a live target"
                                    }
                                    SessionAction::OpenReplay => "Choose or reopen a recording",
                                    SessionAction::Disconnect => {
                                        "Stop acquisition and release the target"
                                    }
                                    SessionAction::StopReplay => "Stop decoding the recording",
                                    SessionAction::Reconnect => {
                                        "Reconnect with the saved live configuration"
                                    }
                                })
                                .disabled(!state.can_primary_action())
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    if !this.adapter.state.can_primary_action() {
                                        return;
                                    }
                                    match action {
                                        SessionAction::Connect | SessionAction::OpenReplay => {
                                            this.open_session_dialog(window, cx);
                                            this.adapter.draft.mode =
                                                if action == SessionAction::Connect {
                                                    Mode::Live
                                                } else {
                                                    Mode::Replay
                                                };
                                            cx.notify();
                                        }
                                        SessionAction::Disconnect | SessionAction::StopReplay => {
                                            this.stop_adapter(cx)
                                        }
                                        SessionAction::Reconnect => this.reconnect_adapter(cx),
                                    }
                                })),
                        )
                        .child(div().absolute().inset_0().child({
                            let bounds = self.adapter.action_bounds.clone();
                            canvas(move |rect, _, _| bounds.set(rect), |_, _, _, _| {}).size_full()
                        })),
                        ),
                        )
                        .child(layout::note_with_shimmer("session-header-board", labels[1].clone(), false, false).w_full())
                        .child(layout::note_with_shimmer("session-header-source",
                            state.session.as_ref().map(|s| s.elf.clone()).unwrap_or_else(|| labels[3].clone()), false, false)
                            .w_full()
                            .text_ellipsis_start()),
                )
                .when(indicator == SessionIndicator::Waiting, |el| {
                    el.child(
                        layout::wrapped_note(
                            "session-waiting-detail",
                            "Waiting for new live measurements. Previously received values may be out of date.",
                            false,
                        ).w_full().text_color(rgb(0xe0b76d)),
                    )
                })
                .when(indicator == SessionIndicator::Warning, |el| {
                    el.when_some(state.session.as_ref().and_then(|s| s.last_notice.clone()), |el, notice| {
                        el.child(
                            layout::wrapped_note("session-read-warning", notice, false)
                                .w_full().text_color(rgb(0xe0b76d)),
                        )
                    })
                })
                .when_some(state.session.as_ref().and_then(|s| s.error.clone()), |el, error| {
                    el.child(div().debug_selector(|| "session-panel-error".into())
                        .text_xs().text_color(rgb(0xf08b9f)).whitespace_normal().child(error))
                })
                .when(state.is_live(), |el| el.child(self.session_history()))
                .into_any_element(),
        )
    }

    fn session_history(&self) -> impl IntoElement {
        let history = &self.live_history;
        let description = history.cache_description();
        let percent = history.memory_percent();
        div()
            .id("session-history")
            .debug_selector(|| "session-history".into())
            .w_full()
            .max_w_full()
            .min_w_0()
            .mt_6()
            .flex()
            .flex_col()
            .gap_1()
            .text_xs()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .text_color(rgb(MUTED))
                            .child("History cache")
                            .child(layout::info(
                                "session-history-info",
                                "About current-view history cache",
                                description,
                            )),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(MUTED))
                            .child(history.retention_label()),
                    ),
            )
            .child(
                gpui_kit::component::progress::Progress::new("session-history-progress")
                    .value(percent)
                    .color(rgb(MUTED))
                    .accessibility_label(format!(
                        "Estimated current-view history memory: {}",
                        history.memory_label()
                    ))
                    .h(px(6.)),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(MUTED))
                    .child(history.memory_label()),
            )
            .when(history.limited(), |el| {
                el.child(
                    div()
                        .debug_selector(|| "session-history-warning".into())
                        .text_color(rgb(0xe0b76d))
                        .child("Memory limit shortened history"),
                )
            })
    }

    pub(super) fn open_session_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.adapter.draft.open {
            return;
        }
        let session = self.adapter.state.session.clone();
        let context = session.as_ref().and_then(|s| s.context.as_ref());
        let configuration = session.as_ref().and_then(|s| match s.source.as_ref() {
            Source::Live { configuration, .. } => Some(configuration),
            _ => None,
        });
        let draft = &mut self.adapter.draft;
        draft.dismiss();
        draft.open = true;
        draft.mode = session.as_ref().map(|s| s.mode()).unwrap_or(Mode::Replay);
        draft.error = None;
        draft.resolved = None;
        draft.detected = AdvancedValues::default();
        draft.advanced = false;
        draft.bound_build = configuration
            .map(|c| c.build_dir.clone())
            .or_else(|| context.map(|c| c.build_dir.clone()))
            .unwrap_or_default();
        let values = [
            (&draft.build, draft.bound_build.clone()),
            (
                &draft.domain,
                if let Some(configuration) = configuration {
                    configuration.domain.clone().unwrap_or_default()
                } else {
                    context.map(|c| c.domain.clone()).unwrap_or_default()
                },
            ),
            (
                &draft.elf,
                if let Some(context) = context {
                    context.elf_override.clone().unwrap_or_default()
                } else {
                    session.as_ref().map(|s| s.elf.clone()).unwrap_or_default()
                },
            ),
            (
                &draft.recording,
                session
                    .as_ref()
                    .and_then(|s| s.recording().map(str::to_owned))
                    .unwrap_or_default(),
            ),
            (
                &draft.runner,
                configuration
                    .and_then(|c| c.runner.clone())
                    .unwrap_or_default(),
            ),
            (
                &draft.target,
                configuration
                    .and_then(|c| c.target.clone())
                    .unwrap_or_default(),
            ),
            (
                &draft.clock,
                configuration
                    .and_then(|c| c.frequency)
                    .map(|v| v.to_string())
                    .unwrap_or_default(),
            ),
        ];
        for (input, value) in values {
            input.update(cx, |input, cx| input.set_value(value, window, cx));
        }
        if let (Some(context), Some(configuration)) = (context, configuration) {
            draft.set_advanced(
                &AdvancedValues::accepted(context, configuration),
                window,
                cx,
            );
        }
        let view = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, window, cx| {
            let Some(app) = view.upgrade() else {
                return dialog;
            };
            let (body, footer, pending) = app.update(cx, |app, cx| {
                (
                    app.session_form(cx),
                    app.session_actions(cx),
                    app.adapter.state.pending,
                )
            });
            let close = view.clone();
            let confirm = view.clone();
            dialog
                .title("Session configuration")
                .w(px(680.).min((window.viewport_size().width - px(32.)).max(px(1.))))
                .margin_top(px(32.))
                .overlay_closable(false)
                .keyboard(!pending)
                .close_button(!pending)
                .on_ok(move |_, window, cx| {
                    let _ = confirm.update(cx, |app, cx| {
                        if !app.adapter.draft.mode_focus.is_focused(window) {
                            app.open_session_draft(cx);
                        }
                    });
                    // Close only once the server has accepted the command.
                    false
                })
                .on_close(move |_, _, cx| {
                    let _ = close.update(cx, |app, cx| {
                        app.adapter.draft.dismiss();
                        cx.notify();
                    });
                })
                .child(body)
                .footer(footer)
        });
        self.schedule_session_build(cx);
        cx.notify();
    }

    fn session_form(&self, cx: &Context<Self>) -> AnyElement {
        let draft = &self.adapter.draft;
        let live = draft.mode == Mode::Live;
        let context = draft.context(cx);
        let capabilities = draft.capabilities(self.adapter.state.session.as_ref(), cx);
        div().debug_selector(|| "session-configuration".into())
            .flex().flex_col().gap_3().min_w_0()
            .child(div().flex().items_center().gap_2()
                .child(div().text_sm().child(if live { "Live settings" } else { "Replay settings" }))
                .child(if live {
                    layout::info("session-live-help", "About Live settings",
                        "Only the build directory is required. Advanced settings are filled automatically and can be edited. Collapsing Advanced keeps your values. Connect opens the probe; it does not flash firmware. Browse opens a system dialog; paths must stay inside the configured data root.").into_any_element()
                } else {
                    layout::info("session-replay-help", "About Replay settings",
                        "Choose a recording and a matching ELF, or a build that supplies the ELF. Replay decodes up to 10 frames/s, not original recording time. Last 256 frames retained. Browse opens a system dialog; paths must stay inside the configured data root.").into_any_element()
                }))
            .child(div().id("session-mode-keyboard")
                .relative()
                .child(div().absolute().inset_0().child({
                    let bounds = draft.mode_bounds.clone();
                    canvas(move |rect, _, _| bounds.set(rect), |_, _, _, _| {}).size_full()
                }))
                .track_focus(&draft.mode_focus).tab_stop(true)
                .aria_label("Session mode. Use Left and Right arrow keys to switch.")
                .on_key_down(cx.listener(|app, event: &KeyDownEvent, window, cx| {
                    if app.adapter.state.pending || app.adapter.draft.picker.is_some() { return; }
                    let mode = match event.keystroke.key.as_str() {
                        "left" | "home" => Mode::Live,
                        "right" | "end" => Mode::Replay,
                        _ => return,
                    };
                    app.adapter.draft.mode = mode;
                    app.adapter.draft.error = None;
                    app.session_draft_changed(window, cx);
                    cx.stop_propagation();
                    cx.notify();
                }))
                .child(TabBar::new("session-mode-tabs")
                .segmented()
                .selected_index(usize::from(!live))
                .child(Tab::new().label("Live").disabled(self.adapter.state.pending || draft.picker.is_some()))
                .child(Tab::new().label("Replay").disabled(self.adapter.state.pending || draft.picker.is_some()))
                .on_click(cx.listener(|app, index: &usize, window, cx| {
                    app.adapter.draft.mode = if *index == 0 { Mode::Live } else { Mode::Replay };
                    app.adapter.draft.error = None;
                    app.adapter.draft.mode_focus.focus(window, cx);
                    app.session_draft_changed(window, cx);
                    cx.notify();
                }))))
            .when(draft.picker.is_some(), |el| el.child(layout::note("session-path-help",
                "The system file dialog is open. Select a path or cancel it to continue.")))
            .child(self.path_field(if live { "Build directory" } else { "Build directory (optional)" }, crate::adapter_protocol::PickerKind::Directory, cx))
            .when(live, |el| el.child(self.session_advanced(cx)))
            .when(!live, |el| el
                .child(field("Firmware image (optional)", &draft.domain))
                .child(self.path_field("Matching ELF (required without a build)", crate::adapter_protocol::PickerKind::Elf, cx)))
            .when(!live, |el| el.child(self.path_field("Recording", crate::adapter_protocol::PickerKind::Recording, cx)))
            .when(!live, |el| el.child(self.session_detection_button(cx)))
            .when(live && draft.resolving, |el| el.child(
                layout::note("session-detection-progress", "Detecting settings from the build directory…")))
            .when_some(context, |el, context| {
                el.child(div().flex().flex_col().gap_1()
                    .child(div().debug_selector(|| "resolved-board".into()).flex().flex_wrap()
                        .child(Tag::color(gpui_kit::component::ColorName::Purple).small()
                            .child(format!("Board: {}", context.board.as_deref().unwrap_or("Unknown")))))
                    .when(context.domains.len() > 1, |el| {
                        el.child(div().flex().flex_wrap().gap_1().children(
                            context.domains.iter().enumerate().map(|(i, domain)| {
                                let name = domain.clone();
                                Button::new(SharedString::from(format!("firmware-image-{i}")))
                                    .label(name.clone()).toggled(context.domain == name)
                                    .on_click(cx.listener(move |app, _, window, cx| {
                                        if app.adapter.draft.mode == Mode::Live {
                                            app.adapter.draft.advanced = true;
                                        }
                                        app.adapter.draft.domain.update(cx, |input, cx| {
                                            input.set_value(name.clone(), window, cx)
                                        });
                                        app.resolve_session_build(false, cx);
                                    }))
                            })
                        ))
                    })
                    .child(provenance_note(context))
                    .children(context.warnings.iter().enumerate().map(|(i, warning)| layout::note(format!("build-warning-{i}"), warning.clone()))))
            })
            .when_some(draft.error.clone(), |el, error| el.child(layout::note("session-draft-error", error)))
            .when_some(self.adapter.state.session.as_ref().and_then(|s| s.last_notice.clone()),
                |el, notice| el.child(layout::note("adapter-notice", notice)))
            .when(live && capabilities.is_none(), |el| el.child(layout::note("discovery-pending",
                "Group availability is unknown for these settings. Connect to discover firmware groups.")))
            .when_some(capabilities,
                |el, capabilities| el
                    .child(div().flex().items_center().gap_2()
                        .child(div().text_sm().child("Session discovery"))
                        .child(if capabilities.is_waitq_walkable {
                            layout::info("wait-queue-available", "About firmware discovery",
                                "Groups marked available were discovered in the connected firmware; they are not necessarily being polled. Waiter inspection is available.").into_any_element()
                        } else {
                            layout::info("wait-queue-unavailable", "About firmware discovery",
                                "Groups marked available were discovered in the connected firmware; they are not necessarily being polled. Waiter inspection is unavailable; unknown waiter counts do not mean zero waiters.").into_any_element()
                        }))
                    .child(div().debug_selector(|| "live-groups".into()).flex().flex_wrap().gap_2()
                        .children(crate::session_settings::discovery_groups(capabilities).into_iter().map(|(group, available)| {
                            let name = crate::session_settings::discovery_name(&group);
                            let objects = self.adapter.state.session.as_ref()
                                .and_then(|s| s.inventory.as_ref())
                                .and_then(|inventory| inventory.get(&group))
                                .map(Vec::as_slice);
                            let text = crate::session_settings::discovery_tooltip(&name, available, objects);
                            layout::info_tag(format!("discovery-{group}-{available}"), name, text, available)
                        })))
                    .children(capabilities.missing.iter().enumerate().map(|(i, notice)|
                        layout::note(format!("capability-notice-{i}"), notice.clone()))))
            .into_any_element()
    }

    fn session_advanced(&self, cx: &Context<Self>) -> impl IntoElement {
        let draft = &self.adapter.draft;
        Collapsible::new()
            .open(draft.advanced)
            .border_1()
            .border_color(rgb(BORDER))
            .rounded_lg()
            .min_w_0()
            .child(
                Button::new("advanced-session-overrides")
                    .icon(if draft.advanced {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    })
                    .ghost()
                    .w_full()
                    .child(div().flex_1().child("Advanced"))
                    .accessibility_label(if draft.advanced {
                        "Collapse Advanced settings"
                    } else {
                        "Expand Advanced settings"
                    })
                    .toggled(draft.advanced)
                    .disabled(self.adapter.state.pending || draft.picker.is_some())
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.adapter.draft.advanced = !app.adapter.draft.advanced;
                        cx.notify();
                    })),
            )
            .content(
                div()
                    .debug_selector(|| "session-advanced-fields".into())
                    .flex()
                    .flex_col()
                    .gap_3()
                    .p_3()
                    .min_w_0()
                    .child(field("Firmware image", &draft.domain))
                    .child(self.path_field("ELF", crate::adapter_protocol::PickerKind::Elf, cx))
                    .child(field("Runner", &draft.runner))
                    .child(field("MCU / GDB server", &draft.target))
                    .child(field("Probe clock in Hz", &draft.clock))
                    .child(layout::note(
                        "advanced-settings-help",
                        "Detected values are editable. Manual edits are kept during background detection.",
                    ))
                    .child(self.session_detection_button(cx))
                    .child(
                        div()
                            .debug_selector(|| "refresh-settings-help".into())
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .child("Replaces advanced values with settings detected from the build directory. If detection fails, your values are kept."),
                    ),
            )
    }

    fn session_detection_button(&self, cx: &Context<Self>) -> impl IntoElement {
        let draft = &self.adapter.draft;
        Button::new("resolve-build")
            .label(if draft.resolving {
                "Detecting…"
            } else if draft.mode == Mode::Live {
                "Refresh detected settings"
            } else {
                "Resolve build context"
            })
            .disabled(
                draft.resolving
                    || Draft::value(&draft.build, cx).is_empty()
                    || self.adapter.state.pending
                    || draft.picker.is_some(),
            )
            .on_click(cx.listener(|app, _, _, cx| app.resolve_session_build(true, cx)))
    }

    fn session_actions(&self, cx: &Context<Self>) -> AnyElement {
        let draft = &self.adapter.draft;
        div()
            .flex()
            .flex_wrap()
            .gap_2()
            .justify_end()
            .child(
                Button::new("cancel-session")
                    .label("Cancel")
                    .disabled(self.adapter.state.pending)
                    .on_click(cx.listener(|app, _, window, cx| {
                        app.adapter.draft.dismiss();
                        window.close_dialog(cx);
                        cx.notify();
                    })),
            )
            .child(
                Button::new("adapter-open")
                    .label(if draft.mode == Mode::Live {
                        "Connect"
                    } else {
                        "Open replay"
                    })
                    .disabled(!draft.can_open(&self.adapter.state, cx))
                    .on_click(cx.listener(|app, _, _, cx| app.open_session_draft(cx))),
            )
            .into_any_element()
    }

    fn open_session_draft(&mut self, cx: &mut Context<Self>) {
        if !self.adapter.draft.can_open(&self.adapter.state, cx) {
            return;
        }
        let mut request = self.adapter.draft.request(cx);
        let data = request.as_object_mut().expect("draft request is an object");
        data.remove("mode");
        if Draft::value(&self.adapter.draft.build, cx).is_empty() {
            data.remove("build_dir");
        }
        let live = self.adapter.draft.mode == Mode::Live;
        if !live {
            data.insert(
                "recording".into(),
                json!(Draft::value(&self.adapter.draft.recording, cx)),
            );
        }
        self.adapter.draft.error = None;
        self.adapter_command(
            if live {
                "/api/session/live"
            } else {
                "/api/session/replay"
            },
            request,
            cx,
        );
    }

    pub(super) fn session_draft_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.adapter.draft.open {
            return;
        }
        let build = Draft::value(&self.adapter.draft.build, cx);
        if self.adapter.draft.mode == Mode::Live && build != self.adapter.draft.bound_build {
            self.adapter.draft.bound_build = build;
            self.adapter.draft.detected = AdvancedValues::default();
            // Overrides belong to a particular build, not the next folder chosen.
            let draft = &self.adapter.draft;
            for input in [
                draft.domain.clone(),
                draft.elf.clone(),
                draft.runner.clone(),
                draft.target.clone(),
                draft.clock.clone(),
            ] {
                input.update(cx, |input, cx| input.set_value("", window, cx));
            }
        }
        self.schedule_session_build(cx);
        cx.notify();
    }

    fn schedule_session_build(&mut self, cx: &mut Context<Self>) {
        let request = self.adapter.draft.request(cx);
        let draft = &mut self.adapter.draft;
        if draft.scheduled_request.as_ref() == Some(&request) {
            return;
        }
        draft.scheduled_request = Some(request.clone());
        draft.resolution_revision = draft.resolution_revision.wrapping_add(1);
        draft.resolving = false;
        draft.resolved = None;
        draft.error = None;
        // Replay keeps its explicit preview button; live discovery is automatic.
        if draft.mode != Mode::Live || Draft::value(&draft.build, cx).is_empty() {
            return;
        }
        let generation = draft.generation;
        let revision = draft.resolution_revision;
        draft.resolving = true;
        spawn_session_task(cx, async move |mut host| {
            adapter_delay(250).await;
            let _ = host.update(|app, _, cx| {
                let draft = &app.adapter.draft;
                if draft.open
                    && draft.generation == generation
                    && draft.resolution_revision == revision
                    && draft.request(cx) == request
                {
                    app.start_build_resolution(revision, false, cx);
                }
            });
        });
    }

    fn resolve_session_build(&mut self, refresh: bool, cx: &mut Context<Self>) {
        self.adapter.draft.scheduled_request = Some(self.adapter.draft.request(cx));
        self.adapter.draft.resolution_revision =
            self.adapter.draft.resolution_revision.wrapping_add(1);
        self.start_build_resolution(self.adapter.draft.resolution_revision, refresh, cx);
    }

    fn start_build_resolution(&mut self, revision: u64, refresh: bool, cx: &mut Context<Self>) {
        let snapshot = self.adapter.draft.request(cx);
        let request = if refresh && self.adapter.draft.mode == Mode::Live {
            json!({"mode": Mode::Live, "build_dir": Draft::value(&self.adapter.draft.build, cx)})
        } else {
            self.adapter.draft.detection_request(cx)
        };
        let generation = self.adapter.draft.generation;
        self.adapter.draft.resolving = true;
        self.adapter.draft.error = None;
        cx.notify();
        let url = self.adapter_url("/api/build/resolve");
        spawn_session_task(cx, async move |mut host| {
            let result = async {
                let response = Request::post(&url)
                    .json(&request)
                    .map_err(|e| e.to_string())?
                    .send()
                    .await
                    .map_err(|e| e.to_string())?;
                if !response.ok() {
                    let data = response
                        .json::<serde_json::Value>()
                        .await
                        .map_err(|e| e.to_string())?;
                    return Err(data["error"]
                        .as_str()
                        .unwrap_or("Build resolution failed.")
                        .to_owned());
                }
                response
                    .json::<BuildContext>()
                    .await
                    .map_err(|e| e.to_string())
            }
            .await;
            let _ = host.update(|app, window, cx| {
                if !app.adapter.draft.open
                    || app.adapter.draft.generation != generation
                    || app.adapter.draft.resolution_revision != revision
                {
                    return;
                }
                app.adapter.draft.resolving = false;
                if snapshot == app.adapter.draft.request(cx) {
                    match result {
                        Ok(context) => {
                            let draft = &mut app.adapter.draft;
                            if draft.mode == Mode::Live {
                                let detected = AdvancedValues::detected(&context);
                                let values = draft.advanced_values(cx).merge(
                                    &mut draft.detected,
                                    &detected,
                                    refresh,
                                );
                                draft.set_advanced(&values, window, cx);
                            }
                            let request = draft.request(cx);
                            draft.scheduled_request = Some(request.clone());
                            draft.resolved = Some((request, context));
                        }
                        Err(error) => {
                            app.adapter.draft.resolved = None;
                            app.adapter.draft.error = Some(error);
                        }
                    }
                }
                cx.notify();
            });
        });
    }
}

/// Opens the same GPUI dialog as the header control, including its draft reset.
#[cfg(target_family = "wasm")]
#[wasm_bindgen]
pub fn adapter_configure() -> Result<(), JsValue> {
    web::update_view(|app, window, cx| app.open_session_dialog(window, cx))
}
