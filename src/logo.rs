//! The AeonDataStudio mark, drawn inline so its gradient follows the active theme's accent
//! colors (`public/logo.svg` is the same mark in the fixed brand colors, for the window and
//! app icons).

use leptos::prelude::*;

/// Data layers narrowing into one clean drop.
const MARK: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64" fill="none" aria-hidden="true">
  <defs>
    <linearGradient id="aeon-bg" x1="6" y1="4" x2="58" y2="60" gradientUnits="userSpaceOnUse">
      <stop offset="0" style="stop-color: var(--accent)"/>
      <stop offset="1" style="stop-color: var(--accent-2, var(--accent))"/>
    </linearGradient>
    <linearGradient id="aeon-gloss" x1="32" y1="2" x2="32" y2="34" gradientUnits="userSpaceOnUse">
      <stop offset="0" stop-color="#fff" stop-opacity=".32"/>
      <stop offset="1" stop-color="#fff" stop-opacity="0"/>
    </linearGradient>
    <radialGradient id="aeon-glow" cx="32" cy="50" r="10" gradientUnits="userSpaceOnUse">
      <stop offset="0" stop-color="#fff" stop-opacity=".9"/>
      <stop offset="1" stop-color="#fff" stop-opacity="0"/>
    </radialGradient>
  </defs>
  <rect x="2" y="2" width="60" height="60" rx="17" fill="url(#aeon-bg)"/>
  <rect x="2" y="2" width="60" height="60" rx="17" fill="url(#aeon-gloss)"/>
  <rect x="2.5" y="2.5" width="59" height="59" rx="16.5" stroke="#fff" stroke-opacity=".22"/>
  <rect x="14" y="16" width="36" height="6" rx="3" fill="#fff" fill-opacity=".95"/>
  <rect x="19" y="26" width="26" height="6" rx="3" fill="#fff" fill-opacity=".85"/>
  <rect x="24" y="36" width="16" height="6" rx="3" fill="#fff" fill-opacity=".75"/>
  <circle cx="32" cy="50" r="9" fill="url(#aeon-glow)"/>
  <circle cx="32" cy="50" r="3.6" fill="#fff"/>
</svg>"##;

#[component]
pub fn Logo(class: &'static str) -> impl IntoView {
    view! { <span class=format!("logo {class}") inner_html=MARK></span> }
}
