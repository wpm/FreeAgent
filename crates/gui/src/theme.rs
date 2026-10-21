//! The page's theme: light, dark, or the system's.

use leptos::prelude::*;

/// Hold the page to `theme`, `light` or `dark`, or leave it to the system.
pub fn set_theme(theme: Option<&str>) {
    if let Some(root) = document().document_element() {
        let _ = match theme {
            Some(theme) => root.set_attribute("data-theme", theme),
            None => root.remove_attribute("data-theme"),
        };
    }
}
