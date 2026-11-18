//! Small shared layout primitives for bounded, independently scrollable screens.
use crate::{GREEN, MUTED};
use gpui_kit::component::shimmer::ShimmerText;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::*;

/// Contextual explanations are available on hover and keyboard focus.
pub(super) fn info(
    id: &'static str,
    label: &'static str,
    text: impl Into<SharedString>,
) -> impl IntoElement {
    Info {
        id: id.into(),
        label: label.into(),
        text: text.into(),
        tag: None,
    }
}

/// A discovery tag uses the same hover/focus explanation as an info control.
#[cfg(any(test, target_family = "wasm"))]
pub(super) fn info_tag(
    id: String,
    name: String,
    text: String,
    available: bool,
) -> impl IntoElement {
    Info {
        id: id.into(),
        label: format!(
            "{name}: {}",
            if available {
                "available"
            } else {
                "unavailable"
            }
        )
        .into(),
        text: text.into(),
        tag: Some((name.into(), available)),
    }
}

#[derive(IntoElement)]
struct Info {
    id: SharedString,
    label: SharedString,
    text: SharedString,
    tag: Option<(SharedString, bool)>,
}

struct InfoState {
    id: SharedString,
    label: SharedString,
    text: SharedString,
    tag: Option<(SharedString, bool)>,
    focus: FocusHandle,
    tooltip: Entity<Tooltip>,
    dismissed: bool,
}

impl RenderOnce for Info {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.use_keyed_state(self.id.clone(), cx, |window, cx| {
            let focus = cx.focus_handle().tab_stop(true);
            cx.on_focus(&focus, window, |this: &mut InfoState, _, cx| {
                this.dismissed = false;
                cx.notify();
            })
            .detach();
            cx.on_blur(&focus, window, |_: &mut InfoState, _, cx| cx.notify())
                .detach();
            let tooltip = info_tooltip(self.id.clone(), self.text.clone(), cx);
            InfoState {
                id: self.id.clone(),
                label: self.label.clone(),
                text: self.text.clone(),
                tag: self.tag.clone(),
                focus,
                tooltip,
                dismissed: false,
            }
        });
        // Refresh live explanations without replacing focus or dismissal state.
        state.update(cx, |state, cx| {
            if state.text != self.text {
                state.tooltip = info_tooltip(self.id, self.text.clone(), cx);
                state.text = self.text;
                cx.notify();
            }
            state.label = self.label;
            state.tag = self.tag;
        });
        state
    }
}

fn info_tooltip(id: SharedString, text: SharedString, cx: &mut App) -> Entity<Tooltip> {
    cx.new(|_| {
        Tooltip::element(move |window, _| {
            let id = id.clone();
            div()
                .debug_selector(move || format!("{id}-tooltip"))
                .w(px(420.).min((window.viewport_size().width - px(48.)).max(px(1.))))
                .flex()
                .flex_col()
                .gap_1()
                .whitespace_normal()
                .children(text.lines().map(|line| div().child(line.to_owned())))
        })
    })
}

impl Render for InfoState {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use gpui_kit::component::{Icon, IconName};
        use gpui_kit::prelude::FluentBuilder;
        let focused = self.focus.is_focused(window);
        let id = self.id.clone();
        div()
            .id(self.id.clone())
            .debug_selector(move || id.to_string())
            .track_focus(&self.focus)
            .tab_stop(true)
            .key_context("ScreenInfo")
            .role(Role::Button)
            .aria_label(self.label.clone())
            .aria_description(self.text.clone())
            .cursor_pointer()
            .when(self.tag.is_none(), |el| el.size_6())
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .rounded_md()
            .text_color(rgb(MUTED))
            .border_1()
            .border_color(gpui_kit::transparent_black())
            .hover(|style| style.bg(rgb(crate::CARD)))
            .focus_visible(|style| style.border_color(rgb(crate::PURPLE)))
            .on_click(cx.listener(|this, _, window, cx| {
                this.dismissed = false;
                this.focus.focus(window, cx);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &crate::DismissScreenInfo, _, cx| {
                this.dismissed = true;
                cx.stop_propagation();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &crate::ShowScreenInfo, _, cx| {
                this.dismissed = false;
                cx.stop_propagation();
                cx.notify();
            }))
            .when(!focused, |el| {
                let tooltip = self.tooltip.clone();
                el.tooltip(move |_, _| tooltip.clone().into())
            })
            .child(if let Some((name, available)) = &self.tag {
                gpui_kit::component::tag::Tag::custom(
                    rgb(if *available { 0x16372e } else { crate::CARD }).into(),
                    rgb(if *available { GREEN } else { MUTED }).into(),
                    rgb(if *available { 0x376653 } else { crate::BORDER }).into(),
                )
                .child(name.clone())
                .into_any_element()
            } else {
                Icon::new(IconName::Info).size_4().into_any_element()
            })
            // Kit's tooltip API is hover-only. Reuse its view in a window-fitted
            // overlay for keyboard focus, without allocating another tab stop.
            .when(focused && !self.dismissed, |el| {
                el.child(
                    deferred(
                        anchored()
                            .offset(point(px(0.), px(28.)))
                            .snap_to_window_with_margin(px(12.))
                            .child(self.tooltip.clone()),
                    )
                    .with_priority(2),
                )
            })
    }
}

/// Keep supporting text on one line without losing the full explanation.
pub(super) fn note(
    id: impl Into<SharedString>,
    text: impl Into<SharedString>,
) -> impl IntoElement + Styled {
    note_with_shimmer(id, text, false)
}

/// Preserve the note's layout and tooltip while optionally highlighting its text.
pub(super) fn note_with_shimmer(
    id: impl Into<SharedString>,
    text: impl Into<SharedString>,
    animate: bool,
) -> impl IntoElement + Styled {
    let id = id.into();
    let text = text.into();
    let content = if animate {
        // Explicit identity and Kit's synced loop keep rate updates from
        // restarting the sweep. Kit also renders static text for reduced motion.
        ShimmerText::new(text.clone())
            .id(id.clone())
            .duration(std::time::Duration::from_secs(3))
            .highlight_color(rgb(GREEN))
            .into_any_element()
    } else {
        text.clone().into_any_element()
    };
    div()
        .id(id.clone())
        .debug_selector(move || id.to_string())
        .min_w_0()
        .flex_shrink_0()
        .text_xs()
        .text_color(rgb(MUTED))
        .truncate()
        .tooltip({
            let text = text.clone();
            move |window, cx| Tooltip::new(text.clone()).build(window, cx)
        })
        .child(content)
}

#[cfg(test)]
mod tests {
    use super::info_tag;
    use core::prelude::v1::test;
    use gpui::{
        AppContext, Context, IntoElement, Modifiers, Render, TestAppContext, VisualTestContext,
        Window, px, size,
    };
    use gpui_kit::component::Root;

    struct DiscoveryPreview(String);

    impl Render for DiscoveryPreview {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            info_tag(
                "discovery-preview".into(),
                "Heaps".into(),
                self.0.clone(),
                true,
            )
        }
    }

    #[gpui_kit::test]
    fn discovery_tag_focus_shows_and_updates_inventory(cx: &mut TestAppContext) {
        cx.update(|cx| {
            crate::init(cx);
            crate::bind_keys(cx);
            crate::apply_theme(cx);
        });
        let mut view = None;
        let window = cx.open_window(size(px(700.), px(500.)), |window, cx| {
            let preview = cx.new(|_| DiscoveryPreview("Inventory pending".into()));
            view = Some(preview.clone());
            Root::new(preview, window, cx)
        });
        let mut cx = VisualTestContext::from_window(*window, cx);
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let tag = cx.debug_bounds("discovery-preview").unwrap();
        cx.simulate_click(tag.center(), Modifiers::default());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let before = cx.debug_bounds("discovery-preview-tooltip").unwrap();
        cx.update(|window, cx| {
            view.as_ref().unwrap().update(cx, |preview, cx| {
                preview.0 =
                    "Heaps: 2 discovered objects\n• bench_heap · 0x1000\n• other_heap · 0x2000"
                        .into();
                cx.notify();
            });
            window.draw(cx).clear(cx);
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let after = cx.debug_bounds("discovery-preview-tooltip").unwrap();
        assert!(after.size.height > before.size.height);
        assert!(after.right() <= px(700.));
        cx.simulate_keystrokes("escape");
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.debug_bounds("discovery-preview-tooltip").is_none());
    }
}
