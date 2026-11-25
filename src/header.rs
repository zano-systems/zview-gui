//! Shared application header; native supplies OS controls through its TitleBar.
use super::*;

pub(super) const HEIGHT: Pixels = px(32.);

impl ThreadsApp {
    pub(super) fn header_content(
        &self,
        controls_inset: Pixels,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .debug_selector(|| "app-header-content".into())
            .flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(
                div()
                    .debug_selector(|| "app-header-leading".into())
                    .when(!self.collapsed, |el| el.w(self.sidebar_width()))
                    .h_full()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .pl(controls_inset)
                    .child(
                        div()
                            .pr_2()
                            .flex_shrink_0()
                            .child(self.header_sidebar_toggle(cx)),
                    )
                    .child(div().w_0().flex_1())
                    .child(help::header_actions(
                        self.navigation_bounds.about.clone(),
                        self.navigation_bounds.help.clone(),
                    ))
                    .child(
                        div()
                            .debug_selector(|| "app-header-separator".into())
                            .w(px(1.))
                            .h_full()
                            .flex_shrink_0()
                            .bg(rgb(BORDER)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .w_0()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .items_center()
                    .px_4()
                    .overflow_hidden()
                    .child(self.breadcrumbs(cx)),
            )
    }

    fn header_sidebar_toggle(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let label = if self.collapsed {
            "Show sidebar"
        } else {
            "Hide sidebar"
        };
        let bounds = self.navigation_bounds.sidebar_toggle.clone();
        div()
            .id("header-sidebar-toggle")
            .debug_selector(|| "header-sidebar-toggle".into())
            .relative()
            // Interactive regions must not drag or double-click the native window.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(|_, _, cx| cx.stop_propagation())
            .child(
                Button::new("header-sidebar-toggle")
                    .ghost()
                    .with_size(px(24.))
                    // Keep the glyph independent of the button's hit target.
                    .child(
                        Icon::new(if self.collapsed {
                            IconName::PanelLeftOpen
                        } else {
                            IconName::PanelLeftClose
                        })
                        .size(px(14.)),
                    )
                    .accessibility_label(label)
                    .tooltip(label)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.collapsed = !this.collapsed;
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .child(canvas(move |rect, _, _| bounds.set(rect), |_, _, _, _| {}).size_full()),
            )
    }

    #[cfg(target_family = "wasm")]
    pub(super) fn web_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .h(HEIGHT)
            .flex_shrink_0()
            .flex()
            .bg(rgb(CARD))
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(self.header_content(px(12.), cx))
    }
}
