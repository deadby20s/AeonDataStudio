//! SQL explorer page: DuckDB queries over datasets and run logs.

use aeon_shared::{SqlResult, SqlTable};
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api;
use crate::app::use_app;
use crate::format;
use crate::sql_editor::SqlEditor;

const LIMIT: u64 = 1000;

#[component]
pub fn SqlPage() -> impl IntoView {
    let ctx = use_app();
    let tables = RwSignal::new(None::<Result<Vec<SqlTable>, String>>);
    let sql = RwSignal::new(String::new());
    let result = RwSignal::new(None::<Result<SqlResult, String>>);
    let running = RwSignal::new(false);

    spawn_local(async move {
        let t = api::sql_tables().await.map_err(|e| e.to_string());
        if let Ok(list) = &t
            && sql.get_untracked().is_empty()
                && let Some(first) = list.iter().find(|t| t.kind == "dataset") {
                    sql.set(format!("SELECT id, source, length(text) AS chars, left(text, 120) AS start\nFROM {}\nLIMIT 100;", first.name));
                }
        tables.set(Some(t));
    });

    let table_list = Signal::derive(move || tables.with(|t| t.as_ref().and_then(|r| r.as_ref().ok()).cloned().unwrap_or_default()));

    let run = move || {
        if running.get_untracked() {
            return;
        }
        running.set(true);
        let q = sql.get_untracked();
        spawn_local(async move {
            match api::run_sql(&q, LIMIT).await {
                Err(e) if e.0 == "Cancelled" => result.set(Some(Err("Query cancelled.".into()))),
                r => result.set(Some(r.map_err(|e| e.to_string()))),
            }
            running.set(false);
        });
    };

    view! {
        <header class="page-header">
            <h1>"SQL"</h1>
        </header>
        <Show
            when=move || ctx.sql_available.get()
            fallback=|| view! { <p class="muted">"This build of the app does not include the SQL explorer."</p> }
        >
            <p class="muted small">
                "Query your datasets and run logs with DuckDB SQL. Each dataset is a table with columns id, text, source and meta (JSON: use meta->>'field'); each run's removed documents are a table with input_row, id, source, step_index, rule, detail, related_row and text. Up to 1,000 rows are shown. Queries can read and write files on this computer, like any SQL client."
            </p>
            <div class="sql-layout">
                <div class="sql-tables">
                    <div class="small-heading">"Tables"</div>
                    {move || match tables.get() {
                        None => view! { <p class="muted small">"Loading…"</p> }.into_any(),
                        Some(Err(e)) => view! { <p class="error small">{e}</p> }.into_any(),
                        Some(Ok(list)) if list.is_empty() => view! { <p class="muted small">"No datasets yet."</p> }.into_any(),
                        Some(Ok(list)) => view! {
                            <ul class="plain-list">
                                {list.clone().into_iter().map(|t| {
                                    let name = t.name.clone();
                                    view! {
                                        <li>
                                            <button
                                                class="list-item"
                                                title="Start a query on this table"
                                                on:click=move |_| sql.set(format!("SELECT *\nFROM {name}\nLIMIT 100;"))
                                            >
                                                <span class="mono small">{t.name.clone()}</span>
                                                <span class="muted small">{t.description}</span>
                                            </button>
                                        </li>
                                    }
                                }).collect_view()}
                            </ul>
                        }.into_any(),
                    }}
                </div>
                <div class="sql-main">
                    <SqlEditor sql=sql tables=table_list on_run=run/>
                    <div class="row-buttons">
                        <button class="primary" disabled=move || running.get() on:click=move |_| run()>
                            {move || if running.get() { "Running…" } else { "Run (Ctrl+Enter)" }}
                        </button>
                        <Show when=move || running.get()>
                            <button on:click=move |_| {
                                spawn_local(async move {
                                    let _ = api::cancel_sql().await;
                                });
                            }>"Cancel"</button>
                        </Show>
                    </div>
                    {move || match result.get() {
                        None => ().into_any(),
                        Some(Err(e)) => view! { <pre class="error small pre">{e}</pre> }.into_any(),
                        Some(Ok(r)) => {
                            let status = match &r.message {
                                Some(m) => format!("{m} ({} ms)", r.elapsed_ms),
                                None => format!(
                                    "{} row{}{} · {} ms",
                                    format::count(r.rows.len() as u64),
                                    if r.rows.len() == 1 { "" } else { "s" },
                                    if r.truncated { " shown (more exist; add LIMIT or aggregate)" } else { "" },
                                    r.elapsed_ms,
                                ),
                            };
                            view! {
                                <div class="muted small" aria-live="polite">{status}</div>
                                {(!r.columns.is_empty()).then(|| view! {
                                    <div class="sql-result">
                                        <table class="table compact">
                                            <thead>
                                                <tr>{r.columns.iter().map(|c| view! { <th>{c.clone()}</th> }).collect_view()}</tr>
                                            </thead>
                                            <tbody>
                                                {r.rows.into_iter().map(|row| view! {
                                                    <tr>
                                                        {row.into_iter().map(|v| match v {
                                                            Some(v) => view! { <td>{v}</td> }.into_any(),
                                                            None => view! { <td class="null">"null"</td> }.into_any(),
                                                        }).collect_view()}
                                                    </tr>
                                                }).collect_view()}
                                            </tbody>
                                        </table>
                                    </div>
                                })}
                            }
                                .into_any()
                        }
                    }}
                </div>
            </div>
        </Show>
    }
}
