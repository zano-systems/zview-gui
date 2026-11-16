//! Help & shortcuts and About dialogs, plus the ZView GUI brand mark.

use super::{BORDER, CARD, MUTED};
use gpui_kit::component::link::Link;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{Icon, IconName, Sizable, StyledExt, WindowExt};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use std::cell::Cell;
use std::rc::Rc;

/// The mark's two single-colour layers (lens, pulse), each tinted by its
/// element's text colour and stacked on a dark tile.
pub(crate) const MARK_LAYERS: [(&str, &[u8]); 2] = [
    (
        "brand/zview-mark-lens.svg",
        include_bytes!("../assets/brand/zview-mark-lens.svg"),
    ),
    (
        "brand/zview-mark-pulse.svg",
        include_bytes!("../assets/brand/zview-mark-pulse.svg"),
    ),
];
const MARK_BG: u32 = 0x080808;
const MARK_LENS: u32 = 0xffffff;
const MARK_PULSE: u32 = 0x00fff7;

pub(crate) const ZANO_GITHUB: &str = "https://github.com/zano-systems";
pub(crate) const ZANO_WEBSITE: &str = "https://zano.systems/";
const SOURCE: &str = "https://github.com/zano-systems/zview-gui";
const ZVIEW: &str = "https://github.com/wkhadgar/zview";

/// Key tokens drawn as the chart's own button icons instead of text.
const RESET_ICON: &str = "icon:reset";
const FIT_ICON: &str = "icon:fit";

/// One shortcut row: the keys (or gesture) and what they do.
pub(crate) struct Shortcut {
    pub keys: &'static [&'static str],
    pub action: &'static str,
}

/// Shortcut groups shown in Help, matching the bindings in `bind_keys` and
/// the chart and strip-map handlers.
pub(crate) const SHORTCUTS: &[(&str, &[Shortcut])] = &[
    (
        "Overview",
        &[
            Shortcut {
                keys: &["↑", "↓"],
                action: "Select a thread in the table",
            },
            Shortcut {
                keys: &["Enter"],
                action: "Open the selected thread",
            },
        ],
    ),
    (
        "Kernel objects",
        &[
            Shortcut {
                keys: &["Tab", "Shift Tab"],
                action: "Move focus between rows",
            },
            Shortcut {
                keys: &["Enter"],
                action: "Open the focused object",
            },
        ],
    ),
    (
        "Details",
        &[Shortcut {
            keys: &["Esc"],
            action: "Back to Overview or Kernel objects",
        }],
    ),
    (
        "Charts",
        &[
            Shortcut {
                keys: &["Scroll"],
                action: "Zoom the chart's time window",
            },
            Shortcut {
                keys: &[RESET_ICON],
                action: "Reset to the default window",
            },
            Shortcut {
                keys: &[FIT_ICON],
                action: "Fit the retained history",
            },
        ],
    ),
    (
        "Heap maps and work-queue strips",
        &[
            Shortcut {
                keys: &["←", "→"],
                action: "Step the visible range",
            },
            Shortcut {
                keys: &["Home", "End"],
                action: "Jump to the start or the end",
            },
            Shortcut {
                keys: &["Drag", "Scroll"],
                action: "Pan or zoom the range",
            },
        ],
    ),
];

/// The Guide tab: titled sections of short paragraphs.
pub(crate) fn guide(browser: bool) -> Vec<(&'static str, Vec<&'static str>)> {
    let sources = if browser {
        vec![
            "Open dump selects an already-decoded JSON snapshot or snapshot array from your device. Files are read locally without uploading. Select a changed file again to reload it.",
            "When served by west zview-gui or the session adapter, the header's session controls connect Live or replay a recording; Configure… edits the build directory, runner and target.",
            "A failed load leaves the current session intact. Arrays enable frame navigation and charts.",
        ]
    } else {
        vec![
            "Open dump loads zview dump --json output. Open recording asks for a .ndjson.gz recording and its matching ELF; Python with ZView dependencies is required (ZVIEW_PYTHON).",
            "Reload repeats the import. Cancel or a failed import leaves the current session intact. Arrays enable frame navigation and charts.",
        ]
    };
    vec![
        ("Data sources", sources),
        (
            "Navigation",
            vec![
                "Click a thread row to open details. Escape or Back to Overview returns without changing session time.",
                "Expand Kernel objects in the sidebar to filter by type; search filters by name/address. Escape and Back return with filters intact.",
                "Breadcrumbs open their named list with search cleared; Kernel objects shows all types. Previous/Next changes the shared snapshot.",
            ],
        ),
        (
            "Reading the views",
            vec![
                "CPU excludes idle. Stack watermark is not total device RAM. Options are raw target flags.",
                "Histories show sampled counts/depth, not individual operations. State bars show counts, occupancy, or lock/busy state—not CPU usage.",
                "Unknown wait queues are labelled unavailable; known empty queues are blank.",
                "Heap maps show captured chunks and relative offsets, not absolute block addresses.",
            ],
        ),
    ]
}

/// The Guide as plain text, for tests.
#[cfg(test)]
pub(crate) fn help_text(browser: bool) -> String {
    guide(browser)
        .into_iter()
        .flat_map(|(_, paragraphs)| paragraphs)
        .collect::<Vec<_>>()
        .join(" ")
}

/// The brand mark: a lens with a runtime pulse leaving it, on a dark tile.
pub(crate) fn mark(size: Pixels) -> Div {
    let inner = size * 0.82;
    let inset = (size - inner) / 2.;
    let layer = |path: &'static str, color: u32| {
        svg()
            .path(path)
            .absolute()
            .top(inset)
            .left(inset)
            .size(inner)
            .text_color(rgb(color))
    };
    div()
        .size(size)
        .flex_shrink_0()
        .relative()
        .rounded(size * 0.23)
        .bg(rgb(MARK_BG))
        .child(layer(MARK_LAYERS[0].0, MARK_LENS))
        .child(layer(MARK_LAYERS[1].0, MARK_PULSE))
}

fn keycap(key: &'static str) -> Div {
    div()
        .px_1p5()
        .min_w(px(22.))
        .flex()
        .justify_center()
        .rounded_sm()
        .border_1()
        .border_color(rgb(BORDER))
        .bg(rgb(CARD))
        .text_xs()
        .font_family("JetBrains Mono")
        .map(|el| match key {
            RESET_ICON => el
                .py_0p5()
                .child(Icon::new(assets::IconName::RotateCcw).xsmall()),
            FIT_ICON => el.py_0p5().child(Icon::new(IconName::Maximize).xsmall()),
            _ => el.child(key),
        })
}

fn section_title(title: &'static str) -> Div {
    div()
        .text_xs()
        .font_semibold()
        .text_color(rgb(MUTED))
        .child(title.to_uppercase())
}

fn shortcuts() -> Div {
    div()
        .flex()
        .flex_col()
        .gap_4()
        .children(SHORTCUTS.iter().map(|(group, rows)| {
            div()
                .flex()
                .flex_col()
                .gap_1p5()
                .child(section_title(group))
                .children(rows.iter().map(|row| {
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(
                            div()
                                .w(px(130.))
                                .flex_shrink_0()
                                .flex()
                                .gap_1()
                                .children(row.keys.iter().map(|key| keycap(key))),
                        )
                        .child(div().text_sm().child(row.action))
                }))
        }))
}

fn guide_view(browser: bool) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_4()
        .children(guide(browser).into_iter().map(|(title, paragraphs)| {
            div()
                .flex()
                .flex_col()
                .gap_1p5()
                .child(section_title(title))
                .children(
                    paragraphs
                        .into_iter()
                        .map(|text| div().text_sm().child(text)),
                )
        }))
}

/// Opens Help & shortcuts: Shortcuts and Guide tabs.
pub(crate) fn open_help(window: &mut Window, cx: &mut App) {
    let browser = cfg!(target_family = "wasm");
    let tab = Rc::new(Cell::new(0usize));
    window.open_dialog(cx, move |dialog, _, _| {
        let selected = tab.get();
        let tab = tab.clone();
        dialog.title("Help & shortcuts").w(px(620.)).child(
            div()
                .flex()
                .flex_col()
                .gap_4()
                .child(
                    TabBar::new("help-tabs")
                        .underline()
                        .selected_index(selected)
                        .child(Tab::new().label("Shortcuts"))
                        .child(Tab::new().label("Guide"))
                        .on_click(move |index, window, _| {
                            tab.set(*index);
                            window.refresh();
                        }),
                )
                .child(
                    div()
                        .id("help-body")
                        .max_h(px(540.))
                        .overflow_y_scroll()
                        .pr_2()
                        .map(|el| {
                            if selected == 0 {
                                el.child(shortcuts())
                            } else {
                                el.child(guide_view(browser))
                            }
                        }),
                ),
        )
    });
}

fn link_row(
    id: &'static str,
    icon: assets::IconName,
    label: &'static str,
    href: &'static str,
) -> Div {
    div()
        .flex()
        .items_center()
        .gap_2()
        .text_sm()
        .child(Icon::new(icon).text_color(rgb(MUTED)))
        .child(Link::new(id).href(href).child(label))
}

/// Opens About: version, Zano Systems backing, project links and license.
pub(crate) fn open_about(window: &mut Window, cx: &mut App) {
    window.open_dialog(cx, |dialog, _, _| {
        dialog.title("About").w(px(480.)).child(
            div()
                .flex()
                .flex_col()
                .gap_5()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_4()
                        .child(mark(px(56.)))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_0p5()
                                .child(div().text_lg().font_semibold().child("ZView GUI"))
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(rgb(MUTED))
                                        .child(format!("Version {}", env!("CARGO_PKG_VERSION"))),
                                ),
                        ),
                )
                .child(div().text_sm().child(
                    "Browser and desktop frontend for ZView, the Zephyr RTOS runtime visualizer.",
                ))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .p_3()
                        .rounded_md()
                        .border_1()
                        .border_color(rgb(BORDER))
                        .bg(rgb(CARD))
                        .child(section_title("Backed by"))
                        .child(div().font_semibold().child("Zano Systems"))
                        .child(
                            div()
                                .text_sm()
                                .text_color(rgb(MUTED))
                                .child("Zephyr® expertise. Built into your product."),
                        )
                        .child(link_row(
                            "about-zano-github",
                            assets::IconName::Github,
                            "github.com/zano-systems",
                            ZANO_GITHUB,
                        ))
                        .child(link_row(
                            "about-zano-site",
                            assets::IconName::Globe,
                            "zano.systems",
                            ZANO_WEBSITE,
                        )),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(link_row(
                            "about-source",
                            assets::IconName::Github,
                            "Source: zano-systems/zview-gui",
                            SOURCE,
                        ))
                        .child(link_row(
                            "about-zview",
                            assets::IconName::Github,
                            "Data acquisition: ZView (wkhadgar/zview)",
                            ZVIEW,
                        )),
                )
                .child(
                    div().text_xs().text_color(rgb(MUTED)).child(
                        "Apache License 2.0. Zephyr® is a trademark of the Linux Foundation.",
                    ),
                ),
        )
    });
}
