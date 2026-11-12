//! Datasets: list, import form, detail page with stats, and document browser.

use std::collections::HashSet;

use aeon_shared::{
    DatasetManifest, DatasetSummary, Document, DocumentPage, HistogramBin, ImportFormat, ImportRequest, JobKind,
    JobState, TextMode,
};
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api;
use crate::app::use_app;
use crate::format;
use crate::pages::JobRow;

const PAGE_SIZE: u64 = 25;

#[component]
pub fn DatasetsPage() -> impl IntoView {
    let ctx = use_app();
    move || match ctx.selected_dataset.get() {
        Some(id) => view! { <DatasetDetail id/> }.into_any(),
        None => view! { <DatasetList/> }.into_any(),
    }
}

// ---------------------------------------------------------------------------------------------
// List
// ---------------------------------------------------------------------------------------------

#[component]
fn DatasetList() -> impl IntoView {
    let ctx = use_app();
    let datasets = RwSignal::new(None::<Result<Vec<DatasetSummary>, String>>);
    let show_import = RwSignal::new(false);
    let dismissed = RwSignal::new(HashSet::<u64>::new());
    let error = RwSignal::new(None::<String>);

    // Reload whenever an import finishes successfully (and once on mount).
    // Note: the memo must be read with `get()`. A memo that is only `track()`ed is never
    // re-evaluated, so the effect would never see the count change.
    let completed_imports = Memo::new(move |_| {
        ctx.jobs.with(|jobs| jobs.iter().filter(|j| j.kind == JobKind::Import && j.state == JobState::Completed).count())
    });
    Effect::new(move |_| {
        let _ = completed_imports.get();
        spawn_local(async move {
            datasets.set(Some(api::list_datasets().await.map_err(|e| e.to_string())));
        });
    });

    let active_imports = Memo::new(move |_| {
        ctx.jobs.with(|jobs| {
            jobs.iter()
                .filter(|j| j.kind == JobKind::Import && !j.state.is_finished())
                .map(|j| j.id)
                .collect::<Vec<_>>()
        })
    });
    let failed_imports = move || {
        let dismissed = dismissed.get();
        ctx.jobs.with(|jobs| {
            jobs.iter()
                .filter(|j| j.kind == JobKind::Import && j.state == JobState::Failed && !dismissed.contains(&j.id))
                .cloned()
                .collect::<Vec<_>>()
        })
    };

    view! {
        <header class="page-header">
            <h1>"Datasets"</h1>
            <Show when=move || !show_import.get()>
                <button class="primary" on:click=move |_| show_import.set(true)>
                    "Import…"
                </button>
            </Show>
        </header>

        <Show when=move || show_import.get()>
            <ImportForm on_close=Callback::new(move |_| show_import.set(false))/>
        </Show>

        {move || error.get().map(|e| view! { <p class="error">{e}</p> })}

        <Show when=move || active_imports.with(|ids| !ids.is_empty())>
            <ul class="job-list">
                <For each=move || active_imports.get() key=|id| *id children=move |id| view! { <JobRow id error/> }/>
            </ul>
        </Show>

        {move || {
            failed_imports()
                .into_iter()
                .map(|job| {
                    let id = job.id;
                    view! {
                        <div class="callout error-callout">
                            <div>
                                <strong>{job.title} " failed"</strong>
                                <div class="small">{job.message.unwrap_or_default()}</div>
                            </div>
                            <button
                                class="icon-button"
                                title="Dismiss"
                                aria-label="Dismiss"
                                on:click=move |_| dismissed.update(|d| { d.insert(id); })
                            >
                                "×"
                            </button>
                        </div>
                    }
                })
                .collect_view()
        }}

        {move || match datasets.get() {
            None => view! { <p class="muted">"Loading…"</p> }.into_any(),
            Some(Err(e)) => view! { <p class="error">{e}</p> }.into_any(),
            Some(Ok(list)) if list.is_empty() => {
                view! {
                    <div class="empty">
                        <h2>"No datasets yet"</h2>
                        <p class="muted">
                            "Import JSONL, JSON, text, CSV/TSV or Parquet files (optionally .gz or .zst compressed) to create one."
                        </p>
                    </div>
                }
                    .into_any()
            }
            Some(Ok(list)) => {
                view! {
                    <table class="table clickable">
                        <thead>
                            <tr>
                                <th>"Name"</th>
                                <th class="num">"Documents"</th>
                                <th class="num">"Characters"</th>
                                <th class="num">"On disk"</th>
                                <th>"Created"</th>
                            </tr>
                        </thead>
                        <tbody>
                            {list
                                .into_iter()
                                .map(|d| {
                                    let id = d.id.clone();
                                    let subtitle = d.description.clone().or(d.source_desc.clone());
                                    view! {
                                        <tr
                                            tabindex="0"
                                            on:click={
                                                let id = id.clone();
                                                move |_| ctx.selected_dataset.set(Some(id.clone()))
                                            }
                                            on:keydown=move |ev| {
                                                if ev.key() == "Enter" {
                                                    ctx.selected_dataset.set(Some(id.clone()));
                                                }
                                            }
                                        >
                                            <td>
                                                <div class="strong">{d.name}</div>
                                                <div class="muted small">{subtitle}</div>
                                            </td>
                                            <td class="num">{opt_count(d.doc_count)}</td>
                                            <td class="num">{opt_count(d.char_count)}</td>
                                            <td class="num">
                                                {d.byte_size.map(format::bytes).unwrap_or_else(|| "—".into())}
                                            </td>
                                            <td>{format::datetime(d.created_at_ms)}</td>
                                        </tr>
                                    }
                                })
                                .collect_view()}
                        </tbody>
                    </table>
                }
                    .into_any()
            }
        }}
    }
}

fn opt_count(n: Option<u64>) -> String {
    n.map(format::count).unwrap_or_else(|| "—".into())
}

// ---------------------------------------------------------------------------------------------
// Import form
// ---------------------------------------------------------------------------------------------

/// File name without directories and data/compression extensions, for a default dataset name.
fn default_name(path: &str) -> String {
    let name = path.rsplit(['/', '\\']).find(|s| !s.is_empty()).unwrap_or(path);
    let mut stem = name;
    for ext in [".gz", ".zst", ".jsonl", ".ndjson", ".json", ".txt", ".md", ".csv", ".tsv", ".parquet"] {
        if stem.len() > ext.len() && stem.to_lowercase().ends_with(ext) {
            stem = &stem[..stem.len() - ext.len()];
        }
    }
    stem.to_string()
}

#[component]
fn ImportForm(on_close: Callback<()>) -> impl IntoView {
    let name = RwSignal::new(String::new());
    let description = RwSignal::new(String::new());
    let paths = RwSignal::new(Vec::<String>::new());
    let typed_path = RwSignal::new(String::new());
    let format = RwSignal::new(ImportFormat::Auto);
    let text_field = RwSignal::new("text".to_string());
    let id_field = RwSignal::new("id".to_string());
    let text_mode = RwSignal::new(TextMode::WholeFile);
    let error = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);

    let add_paths = move |new: Vec<String>| {
        if let Some(first) = new.first()
            && name.with_untracked(|n| n.trim().is_empty()) {
                name.set(default_name(first));
            }
        paths.update(|p| {
            for path in new {
                if !p.contains(&path) {
                    p.push(path);
                }
            }
        });
    };

    let add_files = move |_| {
        error.set(None);
        spawn_local(async move {
            match api::pick_import_files().await {
                Ok(files) => add_paths(files),
                Err(e) => error.set(Some(e.to_string())),
            }
        });
    };
    let add_folder = move |_| {
        error.set(None);
        spawn_local(async move {
            match api::pick_import_folder().await {
                Ok(Some(folder)) => add_paths(vec![folder]),
                Ok(None) => {}
                Err(e) => error.set(Some(e.to_string())),
            }
        });
    };

    let uses_fields = move || !matches!(format.get(), ImportFormat::Text);
    let uses_text_mode = move || matches!(format.get(), ImportFormat::Auto | ImportFormat::Text);
    let can_start = move || !busy.get() && !paths.with(Vec::is_empty) && !name.with(|n| n.trim().is_empty());

    let start = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let id = id_field.get_untracked();
        let desc = description.get_untracked();
        let request = ImportRequest {
            name: name.get_untracked(),
            description: (!desc.trim().is_empty()).then_some(desc),
            paths: paths.get_untracked(),
            format: format.get_untracked(),
            text_field: text_field.get_untracked(),
            id_field: (!id.trim().is_empty()).then_some(id),
            text_mode: text_mode.get_untracked(),
        };
        busy.set(true);
        error.set(None);
        spawn_local(async move {
            match api::start_import(&request).await {
                Ok(_) => on_close.run(()),
                Err(e) => error.set(Some(e.to_string())),
            }
            busy.set(false);
        });
    };

    view! {
        <form class="card form" on:submit=start>
            <h2>"Import data"</h2>

            <div class="field">
                <span class="label">"Sources"</span>
                <div class="row-buttons">
                    <button type="button" on:click=add_files>"Add files…"</button>
                    <button type="button" on:click=add_folder>"Add folder…"</button>
                    <input
                        type="text"
                        class="grow"
                        placeholder="…or paste a file or folder path and press Enter"
                        aria-label="File or folder path"
                        prop:value=move || typed_path.get()
                        on:input=move |ev| typed_path.set(event_target_value(&ev))
                        on:keydown=move |ev| {
                            if ev.key() == "Enter" {
                                ev.prevent_default();
                                let p = typed_path.get_untracked().trim().trim_matches('"').to_string();
                                if !p.is_empty() {
                                    add_paths(vec![p]);
                                    typed_path.set(String::new());
                                }
                            }
                        }
                    />
                </div>
                <Show
                    when=move || !paths.with(Vec::is_empty)
                    fallback=|| {
                        view! {
                            <p class="muted small">
                                "Folders are searched recursively; files of unknown type in them are skipped."
                            </p>
                        }
                    }
                >
                    <ul class="path-list">
                        {move || {
                            paths
                                .get()
                                .into_iter()
                                .map(|p| {
                                    let remove = p.clone();
                                    view! {
                                        <li>
                                            <span class="mono">{p}</span>
                                            <button
                                                type="button"
                                                class="icon-button"
                                                title="Remove"
                                                aria-label="Remove"
                                                on:click=move |_| paths.update(|ps| ps.retain(|x| *x != remove))
                                            >
                                                "×"
                                            </button>
                                        </li>
                                    }
                                })
                                .collect_view()
                        }}
                    </ul>
                </Show>
            </div>

            <div class="grid-2">
                <label class="field">
                    <span class="label">"Dataset name"</span>
                    <input
                        type="text"
                        prop:value=move || name.get()
                        on:input=move |ev| name.set(event_target_value(&ev))
                    />
                </label>
                <label class="field">
                    <span class="label">"Description " <span class="muted">"(optional)"</span></span>
                    <input
                        type="text"
                        prop:value=move || description.get()
                        on:input=move |ev| description.set(event_target_value(&ev))
                    />
                </label>
                <label class="field">
                    <span class="label">"Format"</span>
                    <select on:change=move |ev| {
                        let i: usize = event_target_value(&ev).parse().unwrap_or(0);
                        format.set(ImportFormat::ALL[i.min(ImportFormat::ALL.len() - 1)]);
                    }>
                        {ImportFormat::ALL
                            .iter()
                            .enumerate()
                            .map(|(i, f)| {
                                let f = *f;
                                view! {
                                    <option value=i.to_string() selected=move || format.get() == f>
                                        {f.label()}
                                    </option>
                                }
                            })
                            .collect_view()}
                    </select>
                </label>
                <Show when=uses_text_mode>
                    <label class="field">
                        <span class="label">"Plain-text files"</span>
                        <select on:change=move |ev| {
                            let i: usize = event_target_value(&ev).parse().unwrap_or(0);
                            text_mode.set(TextMode::ALL[i.min(TextMode::ALL.len() - 1)]);
                        }>
                            {TextMode::ALL
                                .iter()
                                .enumerate()
                                .map(|(i, m)| {
                                    let m = *m;
                                    view! {
                                        <option value=i.to_string() selected=move || text_mode.get() == m>
                                            {m.label()}
                                        </option>
                                    }
                                })
                                .collect_view()}
                        </select>
                    </label>
                </Show>
                <Show when=uses_fields>
                    <label class="field">
                        <span class="label">"Text field"</span>
                        <input
                            type="text"
                            class="mono-input"
                            prop:value=move || text_field.get()
                            on:input=move |ev| text_field.set(event_target_value(&ev))
                        />
                        <span class="hint">"JSON key or column holding the document text."</span>
                    </label>
                    <label class="field">
                        <span class="label">"ID field " <span class="muted">"(optional)"</span></span>
                        <input
                            type="text"
                            class="mono-input"
                            prop:value=move || id_field.get()
                            on:input=move |ev| id_field.set(event_target_value(&ev))
                        />
                        <span class="hint">"When missing, ids are “file:record number”."</span>
                    </label>
                </Show>
            </div>

            {move || error.get().map(|e| view! { <p class="error">{e}</p> })}

            <div class="row-buttons end">
                <button type="button" on:click=move |_| on_close.run(())>"Cancel"</button>
                <button type="submit" class="primary" disabled=move || !can_start()>"Start import"</button>
            </div>
        </form>
    }
}

// ---------------------------------------------------------------------------------------------
// Detail
// ---------------------------------------------------------------------------------------------

#[component]
fn DatasetDetail(id: String) -> impl IntoView {
    let ctx = use_app();
    let manifest = RwSignal::new(None::<Result<DatasetManifest, String>>);
    let confirm_delete = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);

    {
        let id = id.clone();
        spawn_local(async move {
            manifest.set(Some(api::get_dataset(&id).await.map_err(|e| e.to_string())));
        });
    }

    let delete = {
        let id = id.clone();
        move |_| {
            let id = id.clone();
            spawn_local(async move {
                match api::delete_dataset(&id).await {
                    Ok(()) => {
                        ctx.selected_dataset.set(None);
                        let _ = ctx.refresh().await;
                    }
                    Err(e) => error.set(Some(e.to_string())),
                }
            });
        }
    };

    view! {
        <div class="breadcrumb">
            <button class="link" on:click=move |_| ctx.selected_dataset.set(None)>"← Datasets"</button>
        </div>
        {move || error.get().map(|e| view! { <p class="error">{e}</p> })}
        {move || match manifest.get() {
            None => view! { <p class="muted">"Loading…"</p> }.into_any(),
            Some(Err(e)) => view! { <p class="error">{e}</p> }.into_any(),
            Some(Ok(m)) => {
                let delete = delete.clone();
                let s = m.stats.clone();
                let mean = s.mean_chars().map(|v| format::count(v.round() as u64)).unwrap_or_else(|| "—".into());
                let range = match (s.min_chars, s.max_chars) {
                    (Some(lo), Some(hi)) => format!("{} – {}", format::count(lo), format::count(hi)),
                    _ => "—".into(),
                };
                let on_disk = m.byte_size();
                let ratio = if on_disk > 0 { s.text_bytes as f64 / on_disk as f64 } else { 0.0 };
                let skipped = m.import.as_ref().map(|i| i.skipped);
                view! {
                    <header class="page-header">
                        <div>
                            <h1>{m.name.clone()}</h1>
                            <p class="muted small">
                                {m.description.clone()} {m.description.is_some().then_some(" · ")}
                                <span class="mono">{m.id.clone()}</span> " · created "
                                {format::datetime(m.created_at_ms)}
                            </p>
                        </div>
                        <div class="actions">
                            <Show
                                when=move || confirm_delete.get()
                                fallback=move || {
                                    view! {
                                        <button class="danger-outline" on:click=move |_| confirm_delete.set(true)>
                                            "Delete…"
                                        </button>
                                    }
                                }
                            >
                                <span class="small">"Delete this dataset and its files?"</span>
                                <button on:click=move |_| confirm_delete.set(false)>"Keep"</button>
                                <button class="danger" on:click=delete.clone()>"Delete"</button>
                            </Show>
                        </div>
                    </header>

                    <div class="stat-grid">
                        <Stat label="Documents" value=format::count(s.doc_count)/>
                        <Stat label="Characters" value=format::count(s.char_count)/>
                        <Stat label="Mean length" value=format!("{mean} chars")/>
                        <Stat label="Shortest – longest" value=range/>
                        <Stat
                            label="On disk"
                            value=format::bytes(on_disk)
                            note=format!("{} of text, {:.1}× compression", format::bytes(s.text_bytes), ratio)
                        />
                        {skipped
                            .map(|k| {
                                view! {
                                    <Stat
                                        label="Skipped at import"
                                        value=format::count(k.records())
                                        note=format!(
                                            "{} no text field · {} empty · {} unreadable",
                                            format::count(k.missing_text),
                                            format::count(k.empty_text),
                                            format::count(k.parse_errors),
                                        )
                                    />
                                }
                            })}
                    </div>

                    <section class="card">
                        <h2>"Document length"</h2>
                        <Histogram bins=s.length_histogram.clone()/>
                    </section>

                    {m.import.clone().map(|imp| {
                        let ignored = imp.skipped.ignored_files;
                        view! {
                            <details class="card">
                                <summary>
                                    <h2 class="inline">"Sources"</h2>
                                    <span class="muted small">
                                        {format!(
                                            " {} file{}",
                                            imp.files.len(),
                                            if imp.files.len() == 1 { "" } else { "s" },
                                        )}
                                        {(ignored > 0).then(|| format!(", {ignored} ignored (unknown type)"))}
                                    </span>
                                </summary>
                                <table class="table compact">
                                    <thead>
                                        <tr>
                                            <th>"File"</th>
                                            <th>"Format"</th>
                                            <th class="num">"Documents"</th>
                                            <th class="num">"Skipped"</th>
                                        </tr>
                                    </thead>
                                    <tbody>
                                        {imp.files.into_iter().map(|f| view! {
                                            <tr>
                                                <td class="mono">{f.source}</td>
                                                <td>{f.format.short()}</td>
                                                <td class="num">{format::count(f.docs)}</td>
                                                <td class="num">{format::count(f.skipped.records())}</td>
                                            </tr>
                                        }).collect_view()}
                                    </tbody>
                                </table>
                                {(!imp.sample_errors.is_empty()).then(|| view! {
                                    <h3 class="small-heading">"Example problems"</h3>
                                    <ul class="error-list mono">
                                        {imp.sample_errors.into_iter().map(|e| view! { <li>{e}</li> }).collect_view()}
                                    </ul>
                                })}
                            </details>
                        }
                    })}

                    <DocumentBrowser dataset_id=m.id.clone() total=s.doc_count/>
                }
                    .into_any()
            }
        }}
    }
}

#[component]
fn Stat(label: &'static str, value: String, #[prop(optional, into)] note: Option<String>) -> impl IntoView {
    view! {
        <div class="stat">
            <div class="stat-label">{label}</div>
            <div class="stat-value">{value}</div>
            {note.map(|n| view! { <div class="stat-note">{n}</div> })}
        </div>
    }
}

fn compact(n: u64) -> String {
    if n >= 1024 && n.is_multiple_of(1024) { format!("{}K", n / 1024) } else { n.to_string() }
}

#[component]
fn Histogram(bins: Vec<HistogramBin>) -> impl IntoView {
    // Trim empty bins at both ends so the chart focuses on where the data is.
    let first = bins.iter().position(|b| b.count > 0).unwrap_or(0);
    let last = bins.iter().rposition(|b| b.count > 0).unwrap_or(0);
    let bins: Vec<_> = if bins.is_empty() { vec![] } else { bins[first..=last].to_vec() };
    let max = bins.iter().map(|b| b.count).max().unwrap_or(0).max(1);
    let total: u64 = bins.iter().map(|b| b.count).sum::<u64>().max(1);

    view! {
        <div class="histogram" role="img" aria-label="Histogram of document length in characters">
            {bins
                .into_iter()
                .map(|b| {
                    let label = match b.hi {
                        Some(hi) => format!("{}–{}", compact(b.lo), compact(hi)),
                        None => format!("≥{}", compact(b.lo)),
                    };
                    let pct_of_max = b.count as f64 / max as f64 * 100.0;
                    let share = b.count as f64 / total as f64 * 100.0;
                    let title = format!("{label} chars: {} documents ({share:.1}%)", format::count(b.count));
                    view! {
                        <div class="hist-col" title=title>
                            <div class="hist-count">{format::count(b.count)}</div>
                            <div class="hist-track">
                                <div
                                    class="hist-bar"
                                    style=format!("height: {:.1}%", if b.count > 0 { pct_of_max.max(1.5) } else { 0.0 })
                                ></div>
                            </div>
                            <div class="hist-label">{label}</div>
                        </div>
                    }
                })
                .collect_view()}
        </div>
        <p class="muted small">"Characters per document. Hover a bar for its share."</p>
    }
}

// ---------------------------------------------------------------------------------------------
// Document browser
// ---------------------------------------------------------------------------------------------

#[component]
fn DocumentBrowser(dataset_id: String, total: u64) -> impl IntoView {
    let dataset_id = StoredValue::new(dataset_id);
    let query_input = RwSignal::new(String::new());
    let active_query = RwSignal::new(None::<String>);
    let start = RwSignal::new(0u64);
    let history = RwSignal::new(Vec::<u64>::new());
    let page = RwSignal::new(None::<Result<DocumentPage, String>>);
    let loading = RwSignal::new(false);
    let expanded = RwSignal::new(None::<(u64, Option<Result<Document, String>>)>);

    let load = move |from: u64| {
        loading.set(true);
        expanded.set(None);
        let query = active_query.get_untracked();
        spawn_local(async move {
            let id = dataset_id.get_value();
            let result = api::browse_documents(&id, from, PAGE_SIZE, query.as_deref()).await;
            page.set(Some(result.map_err(|e| e.to_string())));
            start.set(from);
            loading.set(false);
        });
    };
    load(0);

    let search = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let q = query_input.get_untracked().trim().to_string();
        active_query.set((!q.is_empty()).then_some(q));
        history.set(Vec::new());
        load(0);
    };
    let clear = move |_| {
        query_input.set(String::new());
        active_query.set(None);
        history.set(Vec::new());
        load(0);
    };
    let next = move |_| {
        if let Some(Ok(p)) = page.get_untracked()
            && let Some(n) = p.next_row {
                history.update(|h| h.push(start.get_untracked()));
                load(n);
            }
    };
    let prev = move |_| {
        let mut back = None;
        history.update(|h| back = h.pop());
        if let Some(b) = back {
            load(b);
        }
    };
    let toggle = move |row: u64| {
        if expanded.with_untracked(|e| e.as_ref().is_some_and(|(r, _)| *r == row)) {
            expanded.set(None);
            return;
        }
        expanded.set(Some((row, None)));
        spawn_local(async move {
            let doc = api::get_document(&dataset_id.get_value(), row).await.map_err(|e| e.to_string());
            expanded.update(|e| {
                if let Some((r, slot)) = e
                    && *r == row {
                        *slot = Some(doc);
                    }
            });
        });
    };

    let status = move || {
        let Some(Ok(p)) = page.get() else { return String::new() };
        if active_query.with(Option::is_some) {
            let mut s = format!(
                "{} match{} on this page · searched {} of {} documents",
                p.docs.len(),
                if p.docs.len() == 1 { "" } else { "es" },
                format::count(p.scanned_to),
                format::count(total)
            );
            if p.search_incomplete {
                s.push_str(" · paused after 2 s, use Next to keep searching");
            }
            s
        } else if p.docs.is_empty() {
            "No documents".into()
        } else {
            format!(
                "Documents {}–{} of {}",
                format::count(p.docs[0].row + 1),
                format::count(p.docs.last().map_or(0, |d| d.row + 1)),
                format::count(total)
            )
        }
    };

    view! {
        <section class="card">
            <div class="browser-head">
                <h2 class="inline">"Documents"</h2>
                <form class="search" on:submit=search role="search">
                    <input
                        type="search"
                        placeholder="Search text (case-insensitive)"
                        aria-label="Search document text"
                        prop:value=move || query_input.get()
                        on:input=move |ev| query_input.set(event_target_value(&ev))
                    />
                    <button type="submit">"Search"</button>
                    <Show when=move || active_query.with(Option::is_some)>
                        <button type="button" on:click=clear>"Clear"</button>
                    </Show>
                </form>
            </div>

            <div class="pager">
                <span class="muted small" aria-live="polite">
                    {status}
                    {move || loading.get().then_some(" · loading…")}
                </span>
                <span class="spacer"></span>
                <button disabled=move || loading.get() || history.with(Vec::is_empty) on:click=prev>"← Prev"</button>
                <button
                    disabled=move || {
                        loading.get() || !matches!(page.get(), Some(Ok(ref p)) if p.next_row.is_some())
                    }
                    on:click=next
                >
                    "Next →"
                </button>
            </div>

            {move || match page.get() {
                None => view! { <p class="muted">"Loading…"</p> }.into_any(),
                Some(Err(e)) => view! { <p class="error">{e}</p> }.into_any(),
                Some(Ok(p)) if p.docs.is_empty() => {
                    view! {
                        <p class="muted">
                            {if active_query.with(Option::is_some) {
                                "No matches in the part searched so far."
                            } else {
                                "This dataset has no documents."
                            }}
                        </p>
                    }
                        .into_any()
                }
                Some(Ok(p)) => {
                    view! {
                        <ol class="doc-list">
                            {p.docs
                                .into_iter()
                                .map(|d| {
                                    let row = d.row;
                                    let is_open = move || expanded.with(|e| e.as_ref().is_some_and(|(r, _)| *r == row));
                                    view! {
                                        <li class="doc" class:open=is_open>
                                            <button
                                                class="doc-head"
                                                aria-expanded=move || is_open().to_string()
                                                on:click=move |_| toggle(row)
                                            >
                                                <span class="doc-row">"#" {format::count(row + 1)}</span>
                                                <span class="mono doc-id" title=d.id.clone()>{d.id.clone()}</span>
                                                <span class="muted small doc-source">{d.source.clone()}</span>
                                                <span class="spacer"></span>
                                                <span class="muted small">{format::count(d.chars)} " chars"</span>
                                            </button>
                                            <Show
                                                when=is_open
                                                fallback={
                                                    let preview = d.preview.clone();
                                                    let truncated = d.truncated;
                                                    move || {
                                                        view! {
                                                            <div class="doc-text preview">
                                                                {preview.clone()}
                                                                {truncated.then_some("…")}
                                                            </div>
                                                        }
                                                    }
                                                }
                                            >
                                                {move || match expanded.get().and_then(|(_, d)| d) {
                                                    None => view! { <p class="muted small">"Loading…"</p> }.into_any(),
                                                    Some(Err(e)) => view! { <p class="error">{e}</p> }.into_any(),
                                                    Some(Ok(doc)) => {
                                                        view! {
                                                            <div class="doc-text">{doc.text}</div>
                                                            {(doc.meta_json != "{}")
                                                                .then(|| {
                                                                    view! {
                                                                        <div class="doc-meta">
                                                                            <div class="small-heading">"Metadata"</div>
                                                                            <pre class="mono">{doc.meta_json}</pre>
                                                                        </div>
                                                                    }
                                                                })}
                                                        }
                                                            .into_any()
                                                    }
                                                }}
                                            </Show>
                                        </li>
                                    }
                                })
                                .collect_view()}
                        </ol>
                    }
                        .into_any()
                }
            }}
        </section>
    }
}
