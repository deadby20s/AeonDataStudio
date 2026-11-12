//! Display helpers.

use wasm_bindgen::JsValue;

/// Formats a Unix timestamp in milliseconds using the user's locale.
pub fn datetime(ms: i64) -> String {
    let date = js_sys::Date::new(&JsValue::from_f64(ms as f64));
    String::from(date.to_locale_string("default", &JsValue::UNDEFINED))
}

pub fn bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 { format!("{n} B") } else { format!("{value:.1} {}", UNITS[unit]) }
}

/// Formats an integer with thousands separators, e.g. `1234567` → `1,234,567`.
pub fn count(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// `part / whole` as a percentage with one decimal, e.g. `12.5%`; `—` when `whole` is 0.
pub fn percent(part: u64, whole: u64) -> String {
    if whole == 0 { "—".into() } else { format!("{:.1}%", part as f64 / whole as f64 * 100.0) }
}

/// Duration between two timestamps, e.g. `4.2 s` or `3 min 05 s`.
pub fn duration(start_ms: i64, end_ms: i64) -> String {
    let ms = (end_ms - start_ms).max(0);
    if ms < 60_000 {
        format!("{:.1} s", ms as f64 / 1000.0)
    } else {
        format!("{} min {:02} s", ms / 60_000, (ms % 60_000) / 1000)
    }
}
