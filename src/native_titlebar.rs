//! Native window chrome. Browser composition remains in `lib.rs`.
use super::*;
use crate::header::HEIGHT;
use gpui_kit::component::TitleBar;

// Match Kit's native control inset, but keep it outside sidebar sizing.
const WINDOW_CONTROLS_INSET: Pixels = px(if cfg!(target_os = "macos") { 80. } else { 12. });

impl ThreadsApp {
    pub(super) fn native_titlebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        TitleBar::new()
            .h(HEIGHT)
            // The leading section reserves the same OS-control inset in both states.
            .pl_0()
            .bg(rgb(CARD))
            .border_color(rgb(BORDER))
            .on_close_window(cx.listener(|this, _, _, cx| this.close_native(cx)))
            .child(self.header_content(WINDOW_CONTROLS_INSET, cx))
    }

    /// Keep the event loop alive until startup and owned-process cleanup finish.
    pub(super) fn close_native(&mut self, cx: &mut Context<Self>) {
        if self.adapter.closing {
            return;
        }
        self.adapter.closing = true;
        self.adapter.connection_task.take();
        let startup = self.adapter.startup.take();
        let process = self.adapter.process.take();
        self.adapter.shutdown = Some(native_adapter::Shutdown::new(startup, process));
        cx.spawn(async move |this, cx| {
            loop {
                let finished = this.update(cx, |app, _| {
                    app.adapter
                        .shutdown
                        .as_ref()
                        .is_none_or(|worker| worker.finished())
                });
                match finished {
                    Ok(true) => break,
                    Err(_) => return,
                    Ok(false) => {
                        smol::Timer::after(std::time::Duration::from_millis(25)).await;
                    }
                }
            }
            cx.update(|cx| cx.quit());
        })
        .detach();
    }
}

pub(super) fn window_options() -> WindowOptions {
    WindowOptions {
        titlebar: Some(TitlebarOptions {
            title: Some("ZView".into()),
            appears_transparent: true,
            traffic_light_position: Some(point(px(9.), (HEIGHT - px(14.)) / 2.)),
        }),
        ..TitleBar::window_options()
    }
}

#[cfg(test)]
mod tests {
    use super::HEIGHT;
    use crate::Screen;
    use gpui::{Modifiers, TestAppContext, px, size};

    #[gpui_kit::test]
    fn native_header_tracks_sidebar_width_and_keeps_navigation_in_one_row(cx: &mut TestAppContext) {
        let (app, mut cx) = crate::tests::setup(cx);
        let mut fixed_toggle_bounds = None;
        for width in [1480., 1000.] {
            cx.simulate_resize(size(px(width), px(680.)));
            for adapter in [false, true] {
                for collapsed in [false, true, false] {
                    cx.update(|window, cx| {
                        app.update(cx, |app, cx| {
                            app.adapter.available = adapter;
                            app.collapsed = collapsed;
                            let thread = &mut app.frames[app.frame].threads[0];
                            thread.name = "very_long_thread_name_".repeat(30);
                            app.screen = Screen::Thread(thread.key());
                            cx.notify();
                        });
                        window.draw(cx).clear(cx);
                    });
                    let bar = cx.debug_bounds("app-header-content").unwrap();
                    let sidebar = cx.debug_bounds("sidebar-pane").unwrap();
                    let toggle = cx.debug_bounds("header-sidebar-toggle").unwrap();
                    let leading = cx.debug_bounds("app-header-leading").unwrap();
                    let separator = cx.debug_bounds("app-header-separator").unwrap();
                    let crumbs = cx.debug_bounds("breadcrumbs").unwrap();
                    let current = cx.debug_bounds("breadcrumb-current").unwrap();
                    let actions = cx.debug_bounds("header-actions").unwrap();
                    if let Some(expected) = fixed_toggle_bounds {
                        assert_eq!(
                            toggle, expected,
                            "Toggle must not move when the sidebar changes"
                        );
                    } else {
                        fixed_toggle_bounds = Some(toggle);
                    }
                    assert_eq!(bar.size.height + px(1.), HEIGHT);
                    assert_eq!(bar.bottom() + px(1.), sidebar.top());
                    assert_eq!(separator.size.width, px(1.));
                    assert_eq!(separator.right(), leading.right());
                    assert_eq!(separator.top(), bar.top());
                    assert_eq!(separator.bottom(), bar.bottom());
                    assert!(toggle.right() < separator.left());
                    assert!(actions.left() >= toggle.right() + px(8.));
                    assert_eq!(actions.right(), separator.left());
                    assert!(crumbs.left() >= separator.right());
                    if collapsed {
                        assert_eq!(sidebar.size.width, px(48.));
                        assert_eq!(actions.left(), toggle.right() + px(8.));
                    } else {
                        assert_eq!(separator.right(), sidebar.right());
                    }
                    assert!(crumbs.right() <= px(width));
                    assert!(current.top() >= bar.top() && current.bottom() <= bar.bottom());
                    assert!(cx.debug_bounds("app-toolbar").is_none());
                }
            }
        }
    }

    #[gpui_kit::test]
    fn native_toggle_is_keyboard_accessible_and_preserves_session(cx: &mut TestAppContext) {
        let (app, mut cx) = crate::tests::setup(cx);
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let toggle = cx.debug_bounds("header-sidebar-toggle").unwrap().center();
        cx.simulate_click(toggle, Modifiers::default());
        app.read_with(&cx, |app, _| {
            assert!(app.collapsed);
            assert_eq!(app.frame, 1);
            assert_eq!(app.screen, Screen::Overview);
        });
        crate::tests::activate_breadcrumb_with_keyboard(&mut cx, "header-sidebar-toggle", "space");
        app.read_with(&cx, |app, _| assert!(!app.collapsed));
    }

    #[gpui_kit::test]
    fn titlebar_help_and_about_work_in_both_sidebar_states(cx: &mut TestAppContext) {
        let (app, mut cx) = crate::tests::setup(cx);
        for collapsed in [false, true] {
            cx.update(|window, cx| {
                app.update(cx, |app, cx| {
                    app.collapsed = collapsed;
                    cx.notify();
                });
                window.draw(cx).clear(cx);
            });
            let bar = cx.debug_bounds("app-header-content").unwrap();
            let about = cx.debug_bounds("header-about").unwrap();
            let help = cx.debug_bounds("header-help").unwrap();
            assert!(about.right() < help.left());
            assert!(help.top() >= bar.top() && help.bottom() <= bar.bottom());
            cx.simulate_click(about.center(), Modifiers::default());
            cx.update(|window, cx| window.draw(cx).clear(cx));
            assert!(cx.debug_bounds("about-dialog-content").is_some());
            cx.simulate_keystrokes("escape");
            cx.update(|window, cx| window.draw(cx).clear(cx));
            assert!(cx.debug_bounds("about-dialog-content").is_none());
            crate::tests::activate_breadcrumb_with_keyboard(&mut cx, "header-help", "enter");
            assert!(cx.debug_bounds("help-dialog-content").is_some());
            cx.simulate_keystrokes("escape");
            cx.update(|window, cx| window.draw(cx).clear(cx));
            assert!(cx.debug_bounds("help-dialog-content").is_none());
            app.read_with(&cx, |app, _| {
                assert_eq!(app.frame, 1);
                assert_eq!(app.screen, Screen::Overview);
            });
        }
    }

    #[gpui_kit::test]
    fn native_sidebar_starts_with_kernel_types_expanded_and_backing_above_session(
        cx: &mut TestAppContext,
    ) {
        let (app, mut cx) = crate::tests::setup(cx);
        for height in [940., 680.] {
            cx.simulate_resize(size(px(1000.), px(height)));
            for collapsed in [false, true, false] {
                cx.update(|window, cx| {
                    app.update(cx, |app, cx| {
                        app.adapter.available = true;
                        app.collapsed = collapsed;
                        cx.notify();
                    });
                    window.draw(cx).clear(cx);
                });
                let backing = cx.debug_bounds("sidebar-backing").unwrap();
                let rail = cx.debug_bounds("sidebar-pane").unwrap();
                assert!(backing.left() >= rail.left() && backing.right() <= rail.right());
                if collapsed {
                    let session = cx.debug_bounds("sidebar-session-toggle").unwrap();
                    assert!(backing.bottom() <= session.top());
                    assert!((backing.center().x - session.center().x).abs() <= px(0.5));
                    assert!((session.bottom() - (px(height) - px(12.))).abs() <= px(1.));
                } else {
                    let session = cx.debug_bounds("session-header-controls").unwrap();
                    assert!(backing.bottom() <= session.top());
                    assert!(session.bottom() <= px(height));
                    assert!(cx.debug_bounds("object-type-Heap").is_some());
                }
            }
        }
    }

    #[gpui_kit::test]
    fn sidebar_backing_opens_company_site_with_mouse_and_keyboard(cx: &mut TestAppContext) {
        for collapsed in [false, true] {
            for activation in ["click", "enter", "space"] {
                let (app, mut cx) = crate::tests::setup(cx);
                cx.update(|window, cx| {
                    app.update(cx, |app, cx| {
                        app.collapsed = collapsed;
                        cx.notify();
                    });
                    window.draw(cx).clear(cx);
                });
                // TestPlatform records URL opens without launching a real browser.
                cx.update(|_, cx| cx.open_url("about:blank"));
                if activation == "click" {
                    let backing = cx.debug_bounds("sidebar-backing").unwrap();
                    cx.simulate_click(backing.center(), Modifiers::default());
                } else {
                    cx.update(|window, cx| window.blur(cx));
                    let mut focused = false;
                    for _ in 0..64 {
                        focused = cx.update(|window, cx| {
                            window.focus_next(cx);
                            window.draw(cx).clear(cx);
                            app.read(cx).backing_focus.is_focused(window)
                        });
                        if focused {
                            break;
                        }
                    }
                    assert!(focused, "Company link must be reachable with Tab");
                    cx.simulate_keystrokes(activation);
                }
                assert_eq!(cx.opened_url().as_deref(), Some(crate::help::ZANO_WEBSITE));
                app.read_with(&cx, |app, _| {
                    assert_eq!(app.collapsed, collapsed);
                    assert_eq!(app.frame, 1);
                    assert_eq!(app.screen, Screen::Overview);
                });
            }
        }
    }

    #[gpui_kit::test]
    fn collapsed_session_button_expands_sidebar_without_changing_session(cx: &mut TestAppContext) {
        let (app, mut cx) = crate::tests::setup(cx);
        for height in [940., 680.] {
            cx.simulate_resize(size(px(1000.), px(height)));
            cx.update(|window, cx| {
                app.update(cx, |app, cx| {
                    app.adapter.available = true;
                    app.collapsed = true;
                    cx.notify();
                });
                window.draw(cx).clear(cx);
            });
            let rail = cx.debug_bounds("sidebar-pane").unwrap();
            let mark = cx.debug_bounds("sidebar-brand-mark").unwrap();
            let session = cx.debug_bounds("sidebar-session-toggle").unwrap();
            assert_eq!(rail.size.width, px(48.));
            assert!((mark.center().x - rail.center().x).abs() <= px(1.));
            assert!((session.center().x - mark.center().x).abs() <= px(0.5));
            assert!(session.left() >= rail.left() && session.right() <= rail.right());
            cx.simulate_click(session.center(), Modifiers::default());
            cx.update(|window, cx| window.draw(cx).clear(cx));
            app.read_with(&cx, |app, _| {
                assert!(!app.collapsed);
                assert_eq!(app.frame, 1);
                assert_eq!(app.screen, Screen::Overview);
            });
            assert!(cx.debug_bounds("sidebar-session-toggle").is_none());
            assert!(cx.debug_bounds("session-header-controls").is_some());
            for key in ["enter", "space"] {
                cx.update(|window, cx| {
                    app.update(cx, |app, cx| {
                        app.collapsed = true;
                        cx.notify();
                    });
                    window.draw(cx).clear(cx);
                });
                cx.update(|window, cx| window.blur(cx));
                let mut focused = false;
                for _ in 0..64 {
                    focused = cx.update(|window, cx| {
                        window.focus_next(cx);
                        window.draw(cx).clear(cx);
                        app.read(cx).adapter.session_focus.is_focused(window)
                    });
                    if focused {
                        break;
                    }
                }
                assert!(focused, "Session button must be reachable with Tab");
                cx.simulate_keystrokes(key);
                cx.update(|window, cx| window.draw(cx).clear(cx));
                app.read_with(&cx, |app, _| {
                    assert!(!app.collapsed);
                    assert_eq!(app.frame, 1);
                });
                cx.update(|window, cx| {
                    assert!(app.read(cx).adapter.session_focus.is_focused(window));
                });
            }
        }
    }

    #[gpui_kit::test]
    fn native_startup_error_keeps_adapter_controls_and_empty_session(cx: &mut TestAppContext) {
        let (app, mut cx) = crate::tests::setup(cx);
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                *app = crate::ThreadsApp::from_native_options(
                    Err("Python adapter is required".into()),
                    window,
                    cx,
                );
                cx.notify();
            });
            window.draw(cx).clear(cx);
        });
        app.read_with(&cx, |app, _| {
            assert!(app.adapter.available);
            assert_eq!(app.error.as_deref(), Some("Python adapter is required"));
            assert!(app.frames.iter().all(|frame| frame.threads.is_empty()));
            assert!(app.source.is_none());
        });
        assert!(cx.debug_bounds("session-menu-trigger").is_none());
        assert!(cx.debug_bounds("session-header-controls").is_some());
    }
}
