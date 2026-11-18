//! Codes of the short notices the core asks the Android layer to show.
//!
//! The core never produces user-facing text: it emits `toast\t<code>\t<arg>` and the
//! Java layer looks up the string resource `msg_<code>` in the current language
//! (`arg` fills its `%1$s`). Adding a code here without adding `msg_<code>` to both
//! `values/strings.xml` and `values-ru/strings.xml` fails `tools/check_l10n.py`.

pub const NOTHING_ON_CANVAS: &str = "nothing_on_canvas";
pub const NO_OTHER_RECENT: &str = "no_other_recent";
pub const ALREADY_IN_DOCK: &str = "already_in_dock";
pub const DOCK_FULL: &str = "dock_full";
pub const APP_NOT_AVAILABLE: &str = "app_not_available";
/// arg: the app's label.
pub const CARD_NOT_INSTALLED: &str = "card_not_installed";
pub const DOCK_APP_NOT_INSTALLED: &str = "dock_app_not_installed";
/// arg: the folder's display name is not known to the core; no arg.
pub const FOLDER_ALREADY_HAS_APP: &str = "folder_already_has_app";
pub const FOLDER_GONE: &str = "folder_gone";

pub const ALL: &[&str] = &[
    NOTHING_ON_CANVAS,
    NO_OTHER_RECENT,
    ALREADY_IN_DOCK,
    DOCK_FULL,
    APP_NOT_AVAILABLE,
    CARD_NOT_INSTALLED,
    DOCK_APP_NOT_INSTALLED,
    FOLDER_ALREADY_HAS_APP,
    FOLDER_GONE,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_unique_lowercase_identifiers() {
        let mut seen = std::collections::HashSet::new();
        for c in ALL {
            assert!(seen.insert(*c), "duplicate code {c}");
            assert!(c.chars().all(|ch| ch.is_ascii_lowercase() || ch == '_'), "bad code {c}");
        }
    }
}
