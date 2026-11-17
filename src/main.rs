mod api;
mod app;
mod datasets;
mod diff;
mod exports;
mod format;
mod inspector;
mod pages;
mod pipelines;
mod runs;
mod sql;

use app::App;
use leptos::prelude::*;

fn main() {
    console_error_panic_hook::set_once();
    mount_to_body(|| view! { <App/> })
}
