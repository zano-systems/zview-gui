use gpui_kit::{AssetSource, Result, SharedString, assets};
use std::borrow::Cow;

assets::icon_assets!(
    ExtraIcons,
    [
        LayoutDashboard,
        Boxes,
        Activity,
        CircleDashed,
        RotateCcw,
        Keyboard,
        Github,
        Globe
    ]
);

// Shared controls need the same embedded icons on native and web. The native
// toolkit's default bundle does not include every session action (e.g. Unplug).
assets::icon_assets!(
    ControlIcons,
    [
        ArrowDown,
        ArrowLeft,
        ArrowRight,
        ArrowUp,
        Asterisk,
        Calendar,
        Check,
        ChevronDown,
        ChevronLeft,
        ChevronRight,
        ChevronUp,
        ChevronsUpDown,
        CircleAlert,
        CircleCheck,
        CircleX,
        Close,
        Copy,
        Dash,
        Delete,
        Ellipsis,
        EllipsisVertical,
        ExternalLink,
        Eye,
        EyeOff,
        File,
        Folder,
        Inbox,
        Info,
        Loader,
        LoaderCircle,
        Maximize,
        Minimize,
        Minus,
        PanelLeftClose,
        PanelLeftOpen,
        Pause,
        Play,
        Plug,
        Plus,
        RefreshCw,
        RotateCw,
        Search,
        Settings,
        TriangleAlert,
        Unplug,
    ]
);

pub(super) struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some((_, bytes)) = super::help::MARK_LAYERS.iter().find(|(p, _)| *p == path) {
            return Ok(Some(Cow::Borrowed(bytes)));
        }
        if let Some(bytes) = ExtraIcons.load(path)? {
            return Ok(Some(bytes));
        }
        if let Some(bytes) = ControlIcons.load(path)? {
            return Ok(Some(bytes));
        }
        #[cfg(not(target_family = "wasm"))]
        {
            assets::Assets.load(path)
        }
        #[cfg(target_family = "wasm")]
        {
            Ok(None)
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        #[cfg(not(target_family = "wasm"))]
        let mut paths = assets::Assets.list(path)?;
        #[cfg(target_family = "wasm")]
        let mut paths = Vec::new();
        paths.extend(ControlIcons.list(path)?);
        paths.extend(ExtraIcons.list(path)?);
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}

#[cfg(test)]
mod tests {
    use super::{AppAssets, AssetSource, ControlIcons};

    #[test]
    fn web_search_controls_and_empty_state_icons_are_bundled() {
        for path in [
            "icons/search.svg",
            "icons/chevron-down.svg",
            "icons/check.svg",
            "icons/close.svg",
            "icons/inbox.svg",
            "icons/folder.svg",
            "icons/file.svg",
            "icons/arrow-up.svg",
            "icons/settings.svg",
            "icons/plug.svg",
            "icons/unplug.svg",
            "icons/rotate-cw.svg",
            "icons/loader-circle.svg",
            "icons/circle-alert.svg",
        ] {
            assert!(ControlIcons.load(path).unwrap().is_some(), "{path}");
        }
    }

    #[test]
    fn session_action_and_status_icons_load_through_the_application_asset_source() {
        for path in [
            "icons/plug.svg",
            "icons/unplug.svg",
            "icons/rotate-cw.svg",
            "icons/folder.svg",
            "icons/pause.svg",
            "icons/settings.svg",
            "icons/loader-circle.svg",
            "icons/circle-alert.svg",
            "icons/circle-dashed.svg",
        ] {
            let bytes = AppAssets.load(path).unwrap().unwrap_or_else(|| {
                panic!("Session icon missing from the application asset source: {path}")
            });
            assert!(!bytes.is_empty(), "{path}");
            assert!(
                AppAssets.list("icons/").unwrap().iter().any(|p| p == path),
                "{path}"
            );
        }
    }

    #[test]
    fn navigation_thread_and_default_control_icons_are_bundled() {
        for path in [
            "icons/layout-dashboard.svg",
            "icons/boxes.svg",
            "icons/activity.svg",
            "icons/circle-dashed.svg",
            "icons/play.svg",
            "icons/pause.svg",
            "icons/plus.svg",
            "icons/minus.svg",
            "icons/maximize.svg",
            "icons/rotate-ccw.svg",
        ] {
            assert!(AppAssets.load(path).unwrap().is_some(), "{path}");
        }
        assert!(
            AppAssets
                .list("icons/")
                .unwrap()
                .iter()
                .any(|p| p == "icons/layout-dashboard.svg")
        );
    }
}
