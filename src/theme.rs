//! Color themes: built-in palettes and user themes, applied as CSS variables on `<html>`.

use std::collections::BTreeMap;

use aeon_shared::{Theme, ThemeSettings};
use leptos::prelude::*;
use wasm_bindgen::JsCast;

/// The variables a theme can set, with labels for the editor.
pub const VARS: &[(&str, &str)] = &[
    ("--bg", "Background"),
    ("--surface", "Cards"),
    ("--surface-2", "Inputs and hover"),
    ("--border", "Borders"),
    ("--text", "Text"),
    ("--muted", "Secondary text"),
    ("--accent", "Accent"),
    ("--accent-text", "Text on accent"),
    ("--accent-soft", "Accent background"),
    ("--ok", "Success"),
    ("--danger", "Danger"),
];

type Palette = [&'static str; 11];

const LIGHT: Palette = [
    "#f7f7f8", "#ffffff", "#f0f0f2", "#e2e2e6", "#17171a", "#6b6b76", "#4f5bd5", "#ffffff", "#e8eafb", "#1e8a4c", "#c4372d",
];
const DARK: Palette = [
    "#141417", "#1c1c20", "#26262b", "#33333a", "#ececf1", "#9a9aa6", "#7c86f0", "#0f0f14", "#2a2d4a", "#4cc283", "#f0776d",
];

/// Built-in themes. "System" (no palette) follows the OS light/dark setting.
pub const BUILTIN: &[(&str, Option<Palette>)] = &[
    ("System", None),
    ("Light", Some(LIGHT)),
    ("Dark", Some(DARK)),
    (
        "Midnight",
        Some([
            "#0b1020", "#111831", "#18213f", "#26304f", "#e3e8f7", "#8e9abd", "#5fa8ff", "#071022", "#16284a", "#46c99a", "#ff7a85",
        ]),
    ),
    (
        "Forest",
        Some([
            "#10150f", "#171e16", "#1f281d", "#2e3a2b", "#e4ecdf", "#94a68d", "#7fc66a", "#0d150a", "#22351c", "#7fc66a", "#e9806b",
        ]),
    ),
    (
        "Ember",
        Some([
            "#171211", "#201918", "#2a2120", "#3b2e2c", "#f1e6e2", "#a8938d", "#ff8a4c", "#1a0d06", "#3d2418", "#5cc48a", "#ff6b6b",
        ]),
    ),
    (
        "Paper",
        Some([
            "#f4f1ea", "#fbf9f4", "#ece7dc", "#dcd5c5", "#2b2721", "#776f62", "#9c5a2c", "#ffffff", "#f1e3d6", "#3f7d4a", "#b43c2e",
        ]),
    ),
    (
        "High contrast",
        Some([
            "#000000", "#0a0a0a", "#1a1a1a", "#ffffff", "#ffffff", "#d0d0d0", "#ffd400", "#000000", "#3a3200", "#00ff7f", "#ff4d4d",
        ]),
    ),
];

fn palette_map(p: &Palette) -> BTreeMap<String, String> {
    VARS.iter().zip(p).map(|((k, _), v)| (k.to_string(), v.to_string())).collect()
}

/// Whether the OS prefers dark mode.
pub fn system_dark() -> bool {
    let w = window();
    js_sys::Reflect::get(&w, &"matchMedia".into())
        .ok()
        .and_then(|f| f.dyn_into::<js_sys::Function>().ok())
        .and_then(|f| f.call1(&w, &"(prefers-color-scheme: dark)".into()).ok())
        .and_then(|mql| js_sys::Reflect::get(&mql, &"matches".into()).ok())
        .and_then(|m| m.as_bool())
        .unwrap_or(false)
}

/// The variables a theme sets: a built-in palette, or a custom theme (empty for System).
pub fn vars_of(settings: &ThemeSettings, name: &str) -> BTreeMap<String, String> {
    if let Some(t) = settings.custom.iter().find(|t| t.name == name) {
        return t.vars.clone();
    }
    match BUILTIN.iter().find(|(n, _)| *n == name) {
        Some((_, Some(p))) => palette_map(p),
        _ => BTreeMap::new(),
    }
}

/// The colors on screen now: the active theme over the System palette.
pub fn effective_vars(settings: &ThemeSettings) -> BTreeMap<String, String> {
    let mut base = palette_map(if system_dark() { &DARK } else { &LIGHT });
    base.extend(vars_of(settings, &settings.active));
    base
}

/// Sets the active theme's variables on `<html>`.
pub fn apply(settings: &ThemeSettings) {
    let style: String = vars_of(settings, &settings.active)
        .iter()
        // The backend checks custom themes too; this keeps a bad value from breaking the page.
        .filter(|(k, v)| k.starts_with("--") && !v.contains([';', '{', '}']))
        .map(|(k, v)| format!("{k}: {v};"))
        .collect();
    if let Some(el) = document().document_element() {
        let _ = if style.is_empty() { el.remove_attribute("style") } else { el.set_attribute("style", &style) };
    }
}

/// A new custom theme starting from what is on screen.
pub fn new_custom(settings: &ThemeSettings) -> Theme {
    let mut n = 1;
    let name = loop {
        let candidate = if n == 1 { "My theme".to_string() } else { format!("My theme {n}") };
        if !settings.custom.iter().any(|t| t.name == candidate) && !BUILTIN.iter().any(|(b, _)| *b == candidate) {
            break candidate;
        }
        n += 1;
    };
    Theme { name, vars: effective_vars(settings) }
}
