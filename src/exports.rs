//! Exports page, the export form (on a dataset's page) and the mix form (on the datasets list).

use aeon_shared::{
    DatasetSummary, ExportCompression, ExportFormat, ExportRequest, ExportSummary, JobKind, JobState, MixRequest,
    MixSource, TIKTOKEN_ENCODINGS, TokenizerChoice,
};
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api;
use crate::app::use_app;
use crate::format;
use crate::pages::JobRow;

// ---------------------------------------------------------------------------------------------
// Exports page
// ---------------------------------------------------------------------------------------------

#[component]
pub fn ExportsPage() -> impl IntoView {
    let ctx = use_app();
    let list = RwSignal::new(None::<Result<Vec<ExportSummary>, String>>);
    let error = RwSignal::new(None::<String>);
    let confirm = RwSignal::new(None::<String>);
    // Reload when an export finishes (read with get(): see DatasetList).
    let finished = Memo::new(move |_| {
        ctx.jobs.with(|jobs| jobs.iter().filter(|j| j.kind == JobKind::Export && j.state == JobState::Completed).count())
    });
    let load = move || {
        spawn_local(async move {
            list.set(Some(api::list_exports().await.map_err(|e| e.to_string())));
        });
    };
    Effect::new(move |_| {
        let _ = finished.get();
        load();
    });
    let open = move |folder: String| {
        spawn_local(async move {
            if let Err(e) = api::open_export_folder(&folder).await {
                error.set(Some(e.to_string()));
            }
        });
    };
    let delete = move |folder: String| {
        spawn_local(async move {
            match api::delete_export(&folder).await {
                Ok(()) => load(),
                Err(e) => error.set(Some(e.to_string())),
            }
            confirm.set(None);
        });
    };

    view! {
        <header class="page-header">
            <h1>"Exports"</h1>
        </header>
        <p class="muted">
            "Training-ready files written to the workspace's exports folder. Each export has a manifest.json (source dataset, settings, file list) and a SHA256SUMS file. Start an export from a dataset's page."
        </p>
        {move || error.get().map(|e| view! { <p class="error">{e}</p> })}
        {move || match list.get() {
            None => view! { <p class="muted">"Loading…"</p> }.into_any(),
            Some(Err(e)) => view! { <p class="error">{e}</p> }.into_any(),
            Some(Ok(items)) if items.is_empty() => view! {
                <div class="empty">
                    <h2>"No exports yet"</h2>
                    <p class="muted">"Open a dataset and choose “Export…” to write JSONL, Parquet or tokenized shards."</p>
                </div>
            }.into_any(),
            Some(Ok(items)) => view! {
                <table class="table">
                    <thead>
                        <tr>
                            <th>"Name"</th>
                            <th>"Format"</th>
                            <th class="num">"Documents"</th>
                            <th class="num">"Tokens"</th>
                            <th class="num">"Size"</th>
                            <th>"Created"</th>
                            <th></th>
                        </tr>
                    </thead>
                    <tbody>
                        {items.into_iter().map(|e| {
                            let (f_open, f_del, f_confirm, f_cmp) = (e.folder.clone(), e.folder.clone(), e.folder.clone(), e.folder.clone());
                            view! {
                                <tr>
                                    <td>
                                        <div class="strong">{e.name}</div>
                                        <div class="muted small">"from " {e.dataset_name} <span class="mono">" · exports/" {e.folder.clone()}</span></div>
                                    </td>
                                    <td>{e.format.label()}</td>
                                    <td class="num">{format::count(e.docs)}</td>
                                    <td class="num">{e.tokens.map(format::count).unwrap_or_else(|| "—".into())}</td>
                                    <td class="num">{format!("{} · {} file{}", format::bytes(e.bytes), e.files, if e.files == 1 { "" } else { "s" })}</td>
                                    <td>{format::datetime(e.created_at_ms)}</td>
                                    <td class="num nowrap">
                                        <button class="small-button" on:click=move |_| open(f_open.clone())>"Open folder"</button>
                                        " "
                                        <Show
                                            when=move || confirm.get().as_deref() == Some(f_cmp.as_str())
                                            fallback=move || {
                                                let f = f_confirm.clone();
                                                view! { <button class="small-button danger-outline" on:click=move |_| confirm.set(Some(f.clone()))>"Delete…"</button> }
                                            }
                                        >
                                            {
                                                let f = f_del.clone();
                                                view! {
                                                    <button class="small-button" on:click=move |_| confirm.set(None)>"Keep"</button>
                                                    <button class="small-button danger" on:click=move |_| delete(f.clone())>"Delete files"</button>
                                                }
                                            }
                                        </Show>
                                    </td>
                                </tr>
                            }
                        }).collect_view()}
                    </tbody>
                </table>
            }.into_any(),
        }}
    }
}

// ---------------------------------------------------------------------------------------------
// Export form
// ---------------------------------------------------------------------------------------------

#[derive(Clone, PartialEq)]
enum TokChoice {
    Tiktoken(&'static str),
    HuggingFace,
}

#[component]
pub fn ExportForm(dataset_id: String, dataset_name: String, on_close: Callback<()>) -> impl IntoView {
    let name = RwSignal::new(format!("{dataset_name} export"));
    let fmt = RwSignal::new(ExportFormat::Jsonl);
    let compression = RwSignal::new(ExportCompression::Zstd);
    let shard_mb = RwSignal::new("256".to_string());
    let include_meta = RwSignal::new(true);
    let tok = RwSignal::new(TokChoice::Tiktoken("o200k_base"));
    let hf_path = RwSignal::new(None::<String>);
    let append_eos = RwSignal::new(true);
    let error = RwSignal::new(None::<String>);
    let job = RwSignal::new(None::<u64>);
    let dataset_id = StoredValue::new(dataset_id);

    let pick_tokenizer = move |_| {
        spawn_local(async move {
            match api::pick_tokenizer_file().await {
                Ok(Some(p)) => hf_path.set(Some(p)),
                Ok(None) => {}
                Err(e) => error.set(Some(e.to_string())),
            }
        });
    };
    let start = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        error.set(None);
        let Ok(mb) = shard_mb.get_untracked().trim().parse::<u64>() else {
            error.set(Some("Shard size must be a whole number of MB".into()));
            return;
        };
        let tokenizer = match (fmt.get_untracked(), tok.get_untracked()) {
            (ExportFormat::Tokens, TokChoice::Tiktoken(n)) => Some(TokenizerChoice::Tiktoken(n.to_string())),
            (ExportFormat::Tokens, TokChoice::HuggingFace) => match hf_path.get_untracked() {
                Some(p) => Some(TokenizerChoice::HuggingFace(p)),
                None => {
                    error.set(Some("Choose a tokenizer.json file".into()));
                    return;
                }
            },
            _ => None,
        };
        let request = ExportRequest {
            dataset_id: dataset_id.get_value(),
            name: name.get_untracked(),
            format: fmt.get_untracked(),
            compression: compression.get_untracked(),
            shard_mb: mb,
            include_meta: include_meta.get_untracked(),
            tokenizer,
            append_eos: append_eos.get_untracked(),
        };
        spawn_local(async move {
            match api::start_export(&request).await {
                Ok(id) => job.set(Some(id)),
                Err(e) => error.set(Some(e.to_string())),
            }
        });
    };

    view! {
        <Show
            when=move || job.get().is_none()
            fallback=move || view! {
                <section class="card form">
                    <h2>"Exporting"</h2>
                    <ul class="job-list">{move || job.get().map(|id| view! { <JobRow id error/> })}</ul>
                    <div class="row-buttons end"><button on:click=move |_| on_close.run(())>"Close"</button></div>
                </section>
            }
        >
            <form class="card form" on:submit=start>
                <h2>"Export"</h2>
                <div class="grid-2">
                    <label class="field">
                        <span class="label">"Name"</span>
                        <input type="text" prop:value=move || name.get() on:input=move |ev| name.set(event_target_value(&ev))/>
                        <span class="hint">"Becomes the folder name in exports/."</span>
                    </label>
                    <label class="field">
                        <span class="label">"Format"</span>
                        <select on:change=move |ev| {
                            let i: usize = event_target_value(&ev).parse().unwrap_or(0);
                            fmt.set(ExportFormat::ALL[i.min(2)]);
                        }>
                            {ExportFormat::ALL.iter().enumerate().map(|(i, f)| {
                                let f = *f;
                                view! { <option value=i.to_string() selected=move || fmt.get() == f>{f.label()}</option> }
                            }).collect_view()}
                        </select>
                    </label>
                    <Show when=move || fmt.get() == ExportFormat::Jsonl>
                        <label class="field">
                            <span class="label">"Compression"</span>
                            <select on:change=move |ev| {
                                let i: usize = event_target_value(&ev).parse().unwrap_or(0);
                                compression.set(ExportCompression::ALL[i.min(2)]);
                            }>
                                {ExportCompression::ALL.iter().enumerate().map(|(i, c)| {
                                    let c = *c;
                                    view! { <option value=i.to_string() selected=move || compression.get() == c>{c.label()}</option> }
                                }).collect_view()}
                            </select>
                        </label>
                    </Show>
                    <Show when=move || fmt.get() == ExportFormat::Tokens>
                        <label class="field">
                            <span class="label">"Tokenizer"</span>
                            <select on:change=move |ev| {
                                let v = event_target_value(&ev);
                                tok.set(match TIKTOKEN_ENCODINGS.iter().find(|(n, _)| *n == v) {
                                    Some((n, _)) => TokChoice::Tiktoken(n),
                                    None => TokChoice::HuggingFace,
                                });
                            }>
                                {TIKTOKEN_ENCODINGS.iter().map(|(n, label)| {
                                    let n: &'static str = n;
                                    view! { <option value=n selected=move || tok.get() == TokChoice::Tiktoken(n)>{*label}</option> }
                                }).collect_view()}
                                <option value="hf" selected=move || tok.get() == TokChoice::HuggingFace>"Hugging Face tokenizer.json…"</option>
                            </select>
                            <Show when=move || tok.get() == TokChoice::HuggingFace>
                                <div class="row-buttons">
                                    <button type="button" class="small-button" on:click=pick_tokenizer>"Choose file…"</button>
                                    <span class="mono small">{move || hf_path.get().unwrap_or_else(|| "no file chosen".into())}</span>
                                </div>
                            </Show>
                        </label>
                    </Show>
                    <label class="field">
                        <span class="label">"Shard size (MB)"</span>
                        <input type="number" min="1" step="1" prop:value=move || shard_mb.get() on:input=move |ev| shard_mb.set(event_target_value(&ev))/>
                        <span class="hint">{move || match fmt.get() {
                            ExportFormat::Tokens => "Size of each tokens-*.bin file.",
                            _ => "Before compression.",
                        }}</span>
                    </label>
                    <Show
                        when=move || fmt.get() == ExportFormat::Tokens
                        fallback=move || view! {
                            <label class="field check">
                                <input type="checkbox" prop:checked=move || include_meta.get() on:change=move |ev| include_meta.set(event_target_checked(&ev))/>
                                <span>"Include metadata" <span class="hint block">"Each document's extra fields from the source."</span></span>
                            </label>
                        }
                    >
                        <label class="field check">
                            <input type="checkbox" prop:checked=move || append_eos.get() on:change=move |ev| append_eos.set(event_target_checked(&ev))/>
                            <span>"Append end-of-text token" <span class="hint block">"After every document, so a training loop can tell documents apart."</span></span>
                        </label>
                    </Show>
                </div>
                <Show when=move || fmt.get() == ExportFormat::Tokens>
                    <p class="muted small">
                        "Writes tokens-NNNNN.bin (token ids, uint16 if the vocabulary fits, else uint32, little-endian) and tokens-NNNNN.idx (uint64 offsets: document i is tokens idx[i]..idx[i+1]). The manifest records the tokenizer, dtype and end-of-text id."
                    </p>
                </Show>
                {move || error.get().map(|e| view! { <p class="error">{e}</p> })}
                <div class="row-buttons end">
                    <button type="button" on:click=move |_| on_close.run(())>"Cancel"</button>
                    <button type="submit" class="primary">"Start export"</button>
                </div>
            </form>
        </Show>
    }
}

// ---------------------------------------------------------------------------------------------
// Mix form
// ---------------------------------------------------------------------------------------------

#[component]
pub fn MixForm(datasets: Vec<DatasetSummary>, on_close: Callback<()>) -> impl IntoView {
    // (selected, weight text) per dataset
    let rows = RwSignal::new(datasets.iter().map(|_| (false, "1".to_string())).collect::<Vec<_>>());
    let datasets = StoredValue::new(datasets);
    let name = RwSignal::new("Training mix".to_string());
    let shuffle = RwSignal::new(true);
    let seed = RwSignal::new("1".to_string());
    let error = RwSignal::new(None::<String>);
    let job = RwSignal::new(None::<u64>);

    let estimate = move || {
        let r = rows.get();
        datasets.with_value(|ds| {
            ds.iter().zip(&r).filter(|(_, (on, _))| *on).fold((0.0f64, 0.0f64), |(docs, chars), (d, (_, w))| {
                let w: f64 = w.trim().parse().unwrap_or(0.0);
                (docs + d.doc_count.unwrap_or(0) as f64 * w, chars + d.char_count.unwrap_or(0) as f64 * w)
            })
        })
    };
    let start = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        error.set(None);
        let r = rows.get_untracked();
        let mut sources = Vec::new();
        for (d, (on, w)) in datasets.get_value().iter().zip(&r) {
            if !*on {
                continue;
            }
            match w.trim().parse::<f64>() {
                Ok(weight) => sources.push(MixSource { dataset_id: d.id.clone(), weight }),
                Err(_) => {
                    error.set(Some(format!("Weight for “{}” is not a number", d.name)));
                    return;
                }
            }
        }
        let Ok(seed) = seed.get_untracked().trim().parse::<u64>() else {
            error.set(Some("Seed must be a whole number".into()));
            return;
        };
        let request = MixRequest { name: name.get_untracked(), description: None, sources, shuffle: shuffle.get_untracked(), seed };
        spawn_local(async move {
            match api::start_mix(&request).await {
                Ok(id) => job.set(Some(id)),
                Err(e) => error.set(Some(e.to_string())),
            }
        });
    };

    view! {
        <Show
            when=move || job.get().is_none()
            fallback=move || view! {
                <section class="card form">
                    <h2>"Mixing"</h2>
                    <ul class="job-list">{move || job.get().map(|id| view! { <JobRow id error/> })}</ul>
                    <div class="row-buttons end"><button on:click=move |_| on_close.run(())>"Close"</button></div>
                </section>
            }
        >
            <form class="card form" on:submit=start>
                <h2>"Mix datasets"</h2>
                <p class="muted small">
                    "Weight 1 takes every document once, 0.5 a random half, 2 every document twice. The same seed always gives the same mix. Shuffling interleaves the sources."
                </p>
                <table class="table compact">
                    <thead>
                        <tr>
                            <th></th>
                            <th>"Dataset"</th>
                            <th class="num">"Documents"</th>
                            <th class="num">"Weight"</th>
                        </tr>
                    </thead>
                    <tbody>
                        {move || datasets.get_value().into_iter().enumerate().map(|(i, d)| {
                            let w = rows.with_untracked(|r| r[i].1.clone());
                            view! {
                                <tr>
                                    <td>
                                        <input
                                            type="checkbox"
                                            aria-label=format!("Include {}", d.name)
                                            prop:checked=move || rows.with(|r| r[i].0)
                                            on:change=move |ev| { let c = event_target_checked(&ev); rows.update(|r| r[i].0 = c); }
                                        />
                                    </td>
                                    <td>{d.name.clone()}</td>
                                    <td class="num">{d.doc_count.map(format::count).unwrap_or_default()}</td>
                                    <td class="num">
                                        <input
                                            type="number"
                                            class="weight-input"
                                            min="0"
                                            step="0.1"
                                            prop:value=w
                                            on:change=move |ev| { let v = event_target_value(&ev); rows.update(|r| { r[i].1 = v; r[i].0 = true; }); }
                                        />
                                    </td>
                                </tr>
                            }
                        }).collect_view()}
                    </tbody>
                </table>
                <div class="grid-2">
                    <label class="field">
                        <span class="label">"Name"</span>
                        <input type="text" prop:value=move || name.get() on:input=move |ev| name.set(event_target_value(&ev))/>
                    </label>
                    <label class="field">
                        <span class="label">"Seed"</span>
                        <input type="number" min="0" step="1" prop:value=move || seed.get() on:input=move |ev| seed.set(event_target_value(&ev))/>
                    </label>
                    <label class="field check">
                        <input type="checkbox" prop:checked=move || shuffle.get() on:change=move |ev| shuffle.set(event_target_checked(&ev))/>
                        <span>"Shuffle" <span class="hint block">"Adds a second pass through temporary files."</span></span>
                    </label>
                </div>
                <p class="small">
                    {move || {
                        let (docs, chars) = estimate();
                        format!("About {} documents, {} characters.", format::count(docs.round() as u64), format::count(chars.round() as u64))
                    }}
                </p>
                {move || error.get().map(|e| view! { <p class="error">{e}</p> })}
                <div class="row-buttons end">
                    <button type="button" on:click=move |_| on_close.run(())>"Cancel"</button>
                    <button type="submit" class="primary" disabled=move || rows.with(|r| !r.iter().any(|(on, _)| *on))>"Create mix"</button>
                </div>
            </form>
        </Show>
    }
}
