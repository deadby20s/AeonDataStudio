//! Pipelines page: saved recipes, the step editor, and preview / run on a dataset.

use aeon_shared::{
    DatasetSummary, FieldKind, FieldSpec, PRESETS, PipelinePreview, PipelineSpec, PipelineStep, PipelineSummary,
    STEP_KINDS, StepCategory, StepConfig,
};
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::{Map, Value};

use crate::api;
use crate::app::{Page, use_app};
use crate::format;
use crate::pages::JobRow;

/// The recipe being edited. Lives in the app context so edits survive switching pages.
#[derive(Debug, Clone, PartialEq)]
pub struct Draft {
    /// Saved file this draft came from; `None` until first saved.
    pub file: Option<String>,
    pub name: String,
    pub description: String,
    /// Steps with stable keys for the UI list.
    pub steps: Vec<(u64, PipelineStep)>,
    pub next_key: u64,
    pub dirty: bool,
}

impl Draft {
    pub fn from_spec(spec: PipelineSpec, file: Option<String>) -> Self {
        let steps: Vec<_> = spec.steps.into_iter().enumerate().map(|(i, s)| (i as u64, s)).collect();
        Self {
            file,
            name: spec.name,
            description: spec.description.unwrap_or_default(),
            next_key: steps.len() as u64,
            steps,
            dirty: false,
        }
    }

    pub fn to_spec(&self) -> PipelineSpec {
        let d = self.description.trim();
        PipelineSpec {
            name: self.name.trim().to_string(),
            description: (!d.is_empty()).then(|| d.to_string()),
            steps: self.steps.iter().map(|(_, s)| s.clone()).collect(),
        }
    }
}

fn step_json(step: &PipelineStep) -> Map<String, Value> {
    match serde_json::to_value(&step.config) {
        Ok(Value::Object(m)) => m,
        _ => Map::new(),
    }
}

/// Sets one setting by key; `null` removes it (rule off).
fn set_field(step: &mut PipelineStep, key: &str, value: Value) -> Result<(), String> {
    let mut m = step_json(step);
    if value.is_null() {
        m.remove(key);
    } else {
        m.insert(key.to_string(), value);
    }
    step.config = serde_json::from_value::<StepConfig>(Value::Object(m)).map_err(|e| e.to_string())?;
    Ok(())
}

fn display_value(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(items)) => items.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n"),
        Some(other) => other.to_string(),
    }
}

/// Short summary of a step's settings, shown when its card is collapsed.
fn summary(step: &PipelineStep) -> String {
    let m = step_json(step);
    step.config
        .info()
        .fields
        .iter()
        .filter_map(|f| {
            let v = m.get(f.key)?;
            match (f.kind, v) {
                (FieldKind::Bool | FieldKind::LongText, _) => None,
                (FieldKind::Text, Value::String(s)) if s.is_empty() => None,
                (FieldKind::Text, Value::String(s)) => Some(format!("{} {s}", f.label.to_lowercase())),
                (FieldKind::List, Value::Array(a)) => Some(format!("{}: {}", f.label, a.len())),
                (FieldKind::Choice(opts), Value::String(s)) => {
                    opts.iter().find(|(val, _)| val == s).map(|(_, label)| (*label).to_string())
                }
                (_, Value::Null) => None,
                (_, v) => Some(format!("{} {v}", f.label.to_lowercase())),
            }
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

#[component]
pub fn PipelinesPage() -> impl IntoView {
    let ctx = use_app();
    // A memo, so typing in the editor doesn't rebuild the whole page.
    let editing = Memo::new(move |_| ctx.draft.with(Option::is_some));
    move || {
        if editing.get() {
            view! { <PipelineEditor/> }.into_any()
        } else {
            view! { <PipelineList/> }.into_any()
        }
    }
}

// ---------------------------------------------------------------------------------------------
// List
// ---------------------------------------------------------------------------------------------

#[component]
fn PipelineList() -> impl IntoView {
    let ctx = use_app();
    let show_presets = RwSignal::new(false);
    let list = RwSignal::new(None::<Result<Vec<PipelineSummary>, String>>);
    let error = RwSignal::new(None::<String>);
    let confirm_delete = RwSignal::new(None::<String>);

    let load = move || {
        spawn_local(async move {
            list.set(Some(api::list_pipelines().await.map_err(|e| e.to_string())));
        });
    };
    load();

    let open = move |file: String| {
        spawn_local(async move {
            match api::get_pipeline(&file).await {
                Ok(spec) => ctx.draft.set(Some(Draft::from_spec(spec, Some(file)))),
                Err(e) => error.set(Some(e.to_string())),
            }
        });
    };
    let delete = move |file: String| {
        spawn_local(async move {
            match api::delete_pipeline(&file).await {
                Ok(()) => load(),
                Err(e) => error.set(Some(e.to_string())),
            }
            confirm_delete.set(None);
        });
    };

    view! {
        <header class="page-header">
            <h1>"Pipelines"</h1>
            <button class="primary" on:click=move |_| show_presets.update(|s| *s = !*s)>
                "New pipeline…"
            </button>
        </header>
        <Show when=move || show_presets.get()>
            <section class="card">
                <h2>"Start from"</h2>
                <ul class="preset-list">
                    {PRESETS
                        .iter()
                        .map(|p| {
                            let build = p.build;
                            view! {
                                <li>
                                    <button
                                        class="list-item"
                                        on:click=move |_| {
                                            let mut d = Draft::from_spec(build(), None);
                                            d.dirty = true;
                                            ctx.draft.set(Some(d));
                                        }
                                    >
                                        <span class="strong">
                                            {p.label}
                                            {p.uses_llm.then(|| view! { " " <span class="badge">"needs an LLM server"</span> })}
                                        </span>
                                        <span class="muted small">{p.description}</span>
                                    </button>
                                </li>
                            }
                        })
                        .collect_view()}
                </ul>
            </section>
        </Show>
        <p class="muted">
            "A pipeline is a list of steps that clean documents (normalize) or remove them (filter). Try it on a sample of a dataset, then run it to create a cleaned dataset. Recipes are saved as TOML files in the workspace's pipelines folder."
        </p>
        {move || error.get().map(|e| view! { <p class="error">{e}</p> })}
        {move || match list.get() {
            None => view! { <p class="muted">"Loading…"</p> }.into_any(),
            Some(Err(e)) => view! { <p class="error">{e}</p> }.into_any(),
            Some(Ok(items)) if items.is_empty() => {
                view! {
                    <div class="empty">
                        <h2>"No saved pipelines"</h2>
                        <p class="muted">
                            "“New pipeline…” starts from a ready-made recipe: Gopher-style, FineWeb-style, C4-style or minimal. Every step can then be changed."
                        </p>
                    </div>
                }
                    .into_any()
            }
            Some(Ok(items)) => {
                view! {
                    <table class="table clickable">
                        <thead>
                            <tr>
                                <th>"Name"</th>
                                <th class="num">"Steps"</th>
                                <th>"Modified"</th>
                                <th></th>
                            </tr>
                        </thead>
                        <tbody>
                            {items
                                .into_iter()
                                .map(|p| {
                                    let file = p.file.clone();
                                    let file_open = p.file.clone();
                                    let file_del = p.file.clone();
                                    let file_confirm = p.file.clone();
                                    let file_cmp = p.file.clone();
                                    view! {
                                        <tr tabindex="0" on:click=move |_| open(file_open.clone())>
                                            <td>
                                                <div class="strong">{p.name}</div>
                                                <div class="muted small">
                                                    {p.description} <span class="mono">" " {file}</span>
                                                </div>
                                            </td>
                                            <td class="num">{p.step_count}</td>
                                            <td>{format::datetime(p.modified_ms)}</td>
                                            <td class="num" on:click=|ev| ev.stop_propagation()>
                                                <Show
                                                    when=move || confirm_delete.get().as_deref() == Some(file_cmp.as_str())
                                                    fallback=move || {
                                                        let f = file_confirm.clone();
                                                        view! {
                                                            <button
                                                                class="small-button danger-outline"
                                                                on:click=move |_| confirm_delete.set(Some(f.clone()))
                                                            >
                                                                "Delete…"
                                                            </button>
                                                        }
                                                    }
                                                >
                                                    {
                                                        let f = file_del.clone();
                                                        view! {
                                                            <button class="small-button" on:click=move |_| confirm_delete.set(None)>
                                                                "Keep"
                                                            </button>
                                                            <button
                                                                class="small-button danger"
                                                                on:click=move |_| delete(f.clone())
                                                            >
                                                                "Delete"
                                                            </button>
                                                        }
                                                    }
                                                </Show>
                                            </td>
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

// ---------------------------------------------------------------------------------------------
// Editor
// ---------------------------------------------------------------------------------------------

#[component]
fn PipelineEditor() -> impl IntoView {
    let ctx = use_app();
    let error = RwSignal::new(None::<String>);
    let saved_note = RwSignal::new(None::<String>);
    let confirm_leave = RwSignal::new(false);
    let add_kind = RwSignal::new(0usize);

    let edit = move |f: &dyn Fn(&mut Draft)| {
        ctx.draft.update(|d| {
            if let Some(d) = d {
                f(d);
                d.dirty = true;
            }
        });
        saved_note.set(None);
    };

    let name = Memo::new(move |_| ctx.draft.with(|d| d.as_ref().map(|d| d.name.clone()).unwrap_or_default()));
    let description =
        Memo::new(move |_| ctx.draft.with(|d| d.as_ref().map(|d| d.description.clone()).unwrap_or_default()));
    let dirty = Memo::new(move |_| ctx.draft.with(|d| d.as_ref().is_some_and(|d| d.dirty)));
    let file = Memo::new(move |_| ctx.draft.with(|d| d.as_ref().and_then(|d| d.file.clone())));
    let keys = Memo::new(move |_| {
        ctx.draft.with(|d| d.as_ref().map(|d| d.steps.iter().map(|(k, _)| *k).collect::<Vec<_>>()).unwrap_or_default())
    });

    let save = move |_| {
        let Some(draft) = ctx.draft.get_untracked() else { return };
        error.set(None);
        spawn_local(async move {
            let spec = draft.to_spec();
            match api::save_pipeline(&spec, draft.file.as_deref()).await {
                Ok(file) => {
                    ctx.draft.update(|d| {
                        if let Some(d) = d {
                            d.file = Some(file.clone());
                            d.dirty = false;
                        }
                    });
                    saved_note.set(Some(format!("Saved to pipelines/{file}")));
                }
                Err(e) => error.set(Some(e.to_string())),
            }
        });
    };

    let leave = move || {
        ctx.draft.set(None);
    };

    view! {
        <div class="breadcrumb">
            <Show
                when=move || confirm_leave.get()
                fallback=move || {
                    view! {
                        <button
                            class="link"
                            on:click=move |_| if dirty.get_untracked() { confirm_leave.set(true) } else { leave() }
                        >
                            "← Pipelines"
                        </button>
                    }
                }
            >
                <span class="small">"Discard unsaved changes? "</span>
                <button class="small-button" on:click=move |_| confirm_leave.set(false)>"Keep editing"</button>
                " "
                <button class="small-button danger" on:click=move |_| leave()>"Discard"</button>
            </Show>
        </div>

        <header class="page-header">
            <div class="grow-col">
                <input
                    class="title-input"
                    aria-label="Pipeline name"
                    placeholder="Pipeline name"
                    prop:value=move || name.get()
                    on:input=move |ev| {
                        let v = event_target_value(&ev);
                        edit(&|d| d.name = v.clone());
                    }
                />
                <input
                    class="subtle-input"
                    aria-label="Description"
                    placeholder="Description (optional)"
                    prop:value=move || description.get()
                    on:input=move |ev| {
                        let v = event_target_value(&ev);
                        edit(&|d| d.description = v.clone());
                    }
                />
            </div>
            <div class="actions">
                <span class="muted small">
                    {move || {
                        saved_note
                            .get()
                            .or_else(|| dirty.get().then(|| "Unsaved changes".to_string()))
                            .or_else(|| file.get().map(|f| format!("pipelines/{f}")))
                    }}
                </span>
                <button class="primary" on:click=save disabled=move || !dirty.get()>"Save"</button>
            </div>
        </header>
        {move || error.get().map(|e| view! { <p class="error">{e}</p> })}

        <ol class="step-list">
            <For each=move || keys.get() key=|k| *k children=move |k| view! { <StepCard key=k edit/> }/>
        </ol>

        <div class="add-step">
            <select aria-label="Step type" on:change=move |ev| add_kind.set(event_target_value(&ev).parse().unwrap_or(0))>
                {STEP_KINDS
                    .iter()
                    .enumerate()
                    .map(|(i, k)| {
                        let cat = match k.category {
                            StepCategory::Normalize => "Normalize",
                            StepCategory::Filter => "Filter",
                            StepCategory::Dedup => "Dedup",
                            StepCategory::External => "External",
                        };
                        view! { <option value=i.to_string()>{format!("{cat}: {}", k.label)}</option> }
                    })
                    .collect_view()}
            </select>
            <button on:click=move |_| {
                let kind = &STEP_KINDS[add_kind.get_untracked().min(STEP_KINDS.len() - 1)];
                let config = (kind.default)();
                edit(&|d| {
                    let key = d.next_key;
                    d.next_key += 1;
                    d.steps.push((key, PipelineStep { enabled: true, config: config.clone() }));
                });
            }>"Add step"</button>
        </div>

        <TryPanel/>
    }
}

#[component]
fn StepCard(key: u64, edit: impl Fn(&dyn Fn(&mut Draft)) + Copy + Send + Sync + 'static) -> impl IntoView {
    let ctx = use_app();
    let step = Memo::new(move |_| {
        ctx.draft.with(|d| d.as_ref().and_then(|d| d.steps.iter().find(|(k, _)| *k == key).map(|(_, s)| s.clone())))
    });
    let position = Memo::new(move |_| {
        ctx.draft.with(|d| {
            d.as_ref().map_or((0, 0), |d| (d.steps.iter().position(|(k, _)| *k == key).unwrap_or(0), d.steps.len()))
        })
    });
    let error = RwSignal::new(None::<String>);
    let open = RwSignal::new(false);

    // Applies a change to this step; a change that produces invalid settings is rejected and
    // reported, leaving the step as it was.
    let update_step = move |f: &dyn Fn(&mut PipelineStep) -> Result<(), String>| {
        edit(&|d| {
            if let Some((_, s)) = d.steps.iter_mut().find(|(k, _)| *k == key) {
                let mut copy = s.clone();
                match f(&mut copy) {
                    Ok(()) => {
                        *s = copy;
                        error.set(None);
                    }
                    Err(e) => error.set(Some(e)),
                }
            }
        });
    };

    let move_by = move |delta: isize| {
        edit(&|d| {
            if let Some(i) = d.steps.iter().position(|(k, _)| *k == key) {
                let j = i as isize + delta;
                if j >= 0 && (j as usize) < d.steps.len() {
                    d.steps.swap(i, j as usize);
                }
            }
        });
    };

    view! {
        {move || {
            let Some(s) = step.get() else { return view! { <li></li> }.into_any() };
            let info = s.config.info();
            let (pos, count) = position.get();
            let enabled = s.enabled;
            let cat_class = match info.category {
                StepCategory::Normalize => "badge-cat normalize",
                StepCategory::Filter => "badge-cat filter",
                StepCategory::Dedup => "badge-cat dedup",
                StepCategory::External => "badge-cat external",
            };
            let cat_label = match info.category {
                StepCategory::Normalize => "Normalize",
                StepCategory::Filter => "Filter",
                StepCategory::Dedup => "Dedup",
                StepCategory::External => "External",
            };
            let json = step_json(&s);
            let sum = summary(&s);
            view! {
                <li class="step-card" class:disabled=!enabled>
                    <div class="step-head">
                        <input
                            type="checkbox"
                            aria-label="Step enabled"
                            title="Enabled"
                            prop:checked=enabled
                            on:change=move |ev| {
                                let on = event_target_checked(&ev);
                                update_step(&|st| {
                                    st.enabled = on;
                                    Ok(())
                                });
                            }
                        />
                        <span class="step-num">{pos + 1}</span>
                        <button class="step-title" aria-expanded=move || open.get().to_string() on:click=move |_| open.update(|o| *o = !*o)>
                            <span class="strong">{info.label}</span>
                            <span class=cat_class>{cat_label}</span>
                            <span class="muted small step-summary">{sum}</span>
                        </button>
                        <span class="spacer"></span>
                        <button class="icon-button" title="Move up" aria-label="Move up" disabled={pos == 0} on:click=move |_| move_by(-1)>"↑"</button>
                        <button class="icon-button" title="Move down" aria-label="Move down" disabled={pos + 1 >= count} on:click=move |_| move_by(1)>"↓"</button>
                        <button
                            class="icon-button"
                            title="Remove step"
                            aria-label="Remove step"
                            on:click=move |_| edit(&|d| d.steps.retain(|(k, _)| *k != key))
                        >
                            "×"
                        </button>
                    </div>
                    <Show when=move || open.get()>
                        <div class="step-body">
                            <p class="muted small">{info.description}</p>
                            <div class="grid-2">
                                {info
                                    .fields
                                    .iter()
                                    .map(|f| {
                                        view! { <FieldInput field=*f value=json.get(f.key).cloned() update_step/> }
                                    })
                                    .collect_view()}
                            </div>
                            {move || error.get().map(|e| view! { <p class="error small">{e}</p> })}
                        </div>
                    </Show>
                </li>
            }
                .into_any()
        }}
    }
}

#[component]
fn FieldInput(
    field: FieldSpec,
    value: Option<Value>,
    update_step: impl Fn(&dyn Fn(&mut PipelineStep) -> Result<(), String>) + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let key = field.key;
    let set = move |v: Value| update_step(&|s| set_field(s, key, v.clone()));
    let text = display_value(value.as_ref());
    let help = (!field.help.is_empty()).then_some(field.help);

    let parse_number = move |raw: String, optional: bool, integer: bool, ratio: bool| {
        let raw = raw.trim().to_string();
        if raw.is_empty() {
            if optional {
                set(Value::Null);
            }
            return;
        }
        if integer {
            if let Ok(n) = raw.parse::<u64>() {
                set(Value::from(n));
            }
        } else if let Ok(x) = raw.parse::<f64>()
            && (!ratio || (0.0..=1.0).contains(&x)) {
                set(Value::from(x));
            }
    };

    let input = match field.kind {
        FieldKind::Bool => {
            let checked = value.as_ref().and_then(Value::as_bool).unwrap_or(false);
            return view! {
                <label class="field check">
                    <input type="checkbox" prop:checked=checked on:change=move |ev| set(Value::from(event_target_checked(&ev)))/>
                    <span>
                        {field.label}
                        {help.map(|h| view! { <span class="hint block">{h}</span> })}
                    </span>
                </label>
            }
                .into_any();
        }
        FieldKind::Choice(options) => view! {
            <select on:change=move |ev| set(Value::from(event_target_value(&ev)))>
                {options
                    .iter()
                    .map(|(v, label)| view! { <option value=*v selected={text == *v}>{*label}</option> })
                    .collect_view()}
            </select>
        }
            .into_any(),
        FieldKind::List => view! {
            <textarea
                rows="4"
                class="mono-input"
                prop:value=text.clone()
                on:change=move |ev| {
                    let items: Vec<Value> = event_target_value(&ev)
                        .lines()
                        .map(str::trim)
                        .filter(|l| !l.is_empty())
                        .map(Value::from)
                        .collect();
                    set(Value::Array(items));
                }
            ></textarea>
        }
            .into_any(),
        FieldKind::Text => view! {
            <input type="text" prop:value=text.clone() on:change=move |ev| set(Value::from(event_target_value(&ev).trim().to_string()))/>
        }
            .into_any(),
        FieldKind::LongText => view! {
            <textarea
                rows="8"
                class="mono-input prompt-input"
                prop:value=text.clone()
                on:change=move |ev| set(Value::from(event_target_value(&ev)))
            ></textarea>
        }
            .into_any(),
        kind => {
            let (optional, integer, ratio) = match kind {
                FieldKind::Count => (false, true, false),
                FieldKind::OptCount => (true, true, false),
                FieldKind::Ratio => (false, false, true),
                FieldKind::OptRatio => (true, false, true),
                FieldKind::Number => (false, false, false),
                _ => (true, false, false),
            };
            view! {
                <input
                    type="number"
                    min="0"
                    max=ratio.then_some("1")
                    step=if integer { "1" } else if ratio { "0.01" } else { "any" }
                    placeholder=if optional { "off" } else { "" }
                    prop:value=text.clone()
                    on:change=move |ev| parse_number(event_target_value(&ev), optional, integer, ratio)
                />
            }
                .into_any()
        }
    };

    view! {
        <label class="field">
            <span class="label">{field.label}</span>
            {input}
            {help.map(|h| view! { <span class="hint">{h}</span> })}
        </label>
    }
        .into_any()
}

// ---------------------------------------------------------------------------------------------
// Preview and run
// ---------------------------------------------------------------------------------------------

#[component]
fn TryPanel() -> impl IntoView {
    let ctx = use_app();
    let datasets = RwSignal::new(Vec::<DatasetSummary>::new());
    let dataset_id = RwSignal::new(String::new());
    let sample_size = RwSignal::new(2000u64);
    let output_name = RwSignal::new(String::new());
    let preview = RwSignal::new(None::<Result<PipelinePreview, String>>);
    let busy = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let run_job = RwSignal::new(None::<u64>);

    spawn_local(async move {
        if let Ok(list) = api::list_datasets().await {
            if dataset_id.get_untracked().is_empty()
                && let Some(first) = list.first() {
                    dataset_id.set(first.id.clone());
                }
            datasets.set(list);
        }
    });

    let spec = move || ctx.draft.get_untracked().map(|d| d.to_spec());

    let run_preview = move |_| {
        let Some(spec) = spec() else { return };
        let id = dataset_id.get_untracked();
        busy.set(true);
        error.set(None);
        spawn_local(async move {
            match api::preview_pipeline(&spec, &id, sample_size.get_untracked()).await {
                Ok(p) => preview.set(Some(Ok(p))),
                // A cancelled preview keeps showing the previous result.
                Err(e) if e.0 == "Cancelled" => {}
                Err(e) => preview.set(Some(Err(e.to_string()))),
            }
            busy.set(false);
        });
    };
    let run_full = move |_| {
        let Some(spec) = spec() else { return };
        let id = dataset_id.get_untracked();
        error.set(None);
        spawn_local(async move {
            match api::start_pipeline_run(&spec, &id, &output_name.get_untracked()).await {
                Ok(job) => run_job.set(Some(job)),
                Err(e) => error.set(Some(e.to_string())),
            }
        });
    };
    let no_dataset = move || datasets.with(Vec::is_empty);

    view! {
        <section class="card try-panel">
            <h2>"Try it"</h2>
            <Show
                when=move || !no_dataset()
                fallback=move || {
                    view! {
                        <p class="muted">
                            "Import a dataset first. "
                            <button class="link" on:click=move |_| ctx.page.set(Page::Datasets)>"Go to Datasets"</button>
                        </p>
                    }
                }
            >
                <div class="grid-2">
                    <label class="field">
                        <span class="label">"Dataset"</span>
                        <select on:change=move |ev| dataset_id.set(event_target_value(&ev))>
                            {move || {
                                datasets
                                    .get()
                                    .into_iter()
                                    .map(|d| {
                                        let selected = dataset_id.get_untracked() == d.id;
                                        view! {
                                            <option value=d.id.clone() selected=selected>
                                                {format!(
                                                    "{} ({} docs)",
                                                    d.name,
                                                    d.doc_count.map(format::count).unwrap_or_default(),
                                                )}
                                            </option>
                                        }
                                    })
                                    .collect_view()
                            }}
                        </select>
                    </label>
                    <label class="field">
                        <span class="label">"Preview sample"</span>
                        <select on:change=move |ev| sample_size.set(event_target_value(&ev).parse().unwrap_or(2000))>
                            {[1000u64, 2000, 5000, 10000, 20000]
                                .into_iter()
                                .map(|n| {
                                    view! {
                                        <option value=n.to_string() selected={n == 2000}>
                                            {format!("{} documents", format::count(n))}
                                        </option>
                                    }
                                })
                                .collect_view()}
                        </select>
                        <span class="hint">"Taken from 20 places spread across the dataset."</span>
                    </label>
                    <label class="field">
                        <span class="label">"Output dataset name " <span class="muted">"(optional)"</span></span>
                        <input
                            type="text"
                            placeholder="“<dataset> · <pipeline>”"
                            prop:value=move || output_name.get()
                            on:input=move |ev| output_name.set(event_target_value(&ev))
                        />
                    </label>
                </div>
                <div class="row-buttons">
                    <button on:click=run_preview disabled=move || busy.get()>
                        {move || if busy.get() { "Previewing…" } else { "Preview on sample" }}
                    </button>
                    <Show when=move || busy.get()>
                        <button on:click=move |_| {
                            spawn_local(async move {
                                let _ = api::cancel_preview().await;
                            });
                        }>"Cancel preview"</button>
                    </Show>
                    <button class="primary" on:click=run_full>"Run on full dataset"</button>
                </div>
            </Show>
            {move || error.get().map(|e| view! { <p class="error">{e}</p> })}
            {move || {
                run_job
                    .get()
                    .map(|id| {
                        view! {
                            <ul class="job-list">
                                <JobRow id error/>
                            </ul>
                        }
                    })
            }}
            {move || match preview.get() {
                None => ().into_any(),
                Some(Err(e)) => view! { <p class="error">{e}</p> }.into_any(),
                Some(Ok(p)) => view! { <PreviewResult preview=p/> }.into_any(),
            }}
        </section>
    }
}

#[component]
fn PreviewResult(preview: PipelinePreview) -> impl IntoView {
    let p = preview;
    let removed = p.sample_docs - p.docs_out;
    let has_dedup = p.steps.iter().any(|s| s.kind.ends_with("_dedup"));
    view! {
        <div class="preview">
            {has_dedup
                .then(|| {
                    view! {
                        <p class="muted small">
                            "Dedup steps only compare documents within the sample here; a full run compares the whole dataset and usually finds more."
                        </p>
                    }
                })}
            <p>
                <strong>{format!("{} of {} sample documents kept", format::count(p.docs_out), format::count(p.sample_docs))}</strong>
                <span class="muted">
                    {format!(" ({}), {} removed", format::percent(p.docs_out, p.sample_docs), format::count(removed))}
                </span>
            </p>
            <StepFunnel steps=p.steps.clone() on_rule=None/>
            {p.steps
                .iter()
                .filter(|s| s.dropped > 0 || s.modified > 0)
                .map(|s| {
                    let drops: Vec<_> = p.drop_examples.iter().filter(|e| e.step_index == s.index).cloned().collect();
                    let changes: Vec<_> = p.change_examples.iter().filter(|e| e.step_index == s.index).cloned().collect();
                    view! {
                        <details class="examples">
                            <summary>
                                {format!("Step {} · {}: examples", s.index + 1, s.label)}
                            </summary>
                            {drops
                                .into_iter()
                                .map(|e| {
                                    view! {
                                        <div class="example">
                                            <div class="small">
                                                <span class="rule">{e.rule}</span> " " <span class="muted">{e.detail}</span>
                                                <span class="muted mono">" · " {e.id}</span>
                                            </div>
                                            <div class="doc-text preview">{e.preview}</div>
                                        </div>
                                    }
                                })
                                .collect_view()}
                            {changes
                                .into_iter()
                                .map(|e| {
                                    view! {
                                        <div class="example">
                                            <div class="small muted mono">{e.id}</div>
                                            <div class="diff">
                                                <div>
                                                    <div class="small-heading">"Before"</div>
                                                    <div class="doc-text">{e.before}</div>
                                                </div>
                                                <div>
                                                    <div class="small-heading">"After"</div>
                                                    <div class="doc-text">{e.after}</div>
                                                </div>
                                            </div>
                                        </div>
                                    }
                                })
                                .collect_view()}
                        </details>
                    }
                })
                .collect_view()}
        </div>
    }
}

/// Per-step table: documents in, removed (with rules), changed. `on_rule` makes rule names
/// clickable (used on the run page to filter removed documents).
#[component]
pub fn StepFunnel(steps: Vec<aeon_shared::StepStats>, on_rule: Option<Callback<(u64, String)>>) -> impl IntoView {
    view! {
        <table class="table compact funnel">
            <thead>
                <tr>
                    <th>"Step"</th>
                    <th class="num">"In"</th>
                    <th class="num">"Removed"</th>
                    <th class="num">"Changed"</th>
                    <th>"Removed by rule"</th>
                </tr>
            </thead>
            <tbody>
                {steps
                    .into_iter()
                    .map(|s| {
                        let max = s.docs_in.max(1);
                        let width = s.dropped as f64 / max as f64 * 100.0;
                        let index = s.index;
                        view! {
                            <tr>
                                <td>
                                    <span class="muted">{s.index + 1} ". "</span>
                                    {s.label}
                                </td>
                                <td class="num">{format::count(s.docs_in)}</td>
                                <td class="num">
                                    <div class="bar-cell">
                                        <span>{format::count(s.dropped)}</span>
                                        <span class="muted small">{format::percent(s.dropped, s.docs_in)}</span>
                                    </div>
                                    <div class="mini-bar" aria-hidden="true">
                                        <div style=format!("width: {width:.1}%")></div>
                                    </div>
                                </td>
                                <td class="num">
                                    {if s.modified > 0 { format::count(s.modified) } else { "—".into() }}
                                </td>
                                <td>
                                    <div class="rules">
                                    {s.drops_by_rule
                                        .into_iter()
                                        .map(|r| {
                                            let label = format!("{} {}", r.rule, format::count(r.count));
                                            match on_rule {
                                                Some(cb) => {
                                                    let rule = r.rule.clone();
                                                    view! {
                                                        <button
                                                            class="chip"
                                                            title="Show these documents"
                                                            on:click=move |_| cb.run((index, rule.clone()))
                                                        >
                                                            {label}
                                                        </button>
                                                    }
                                                        .into_any()
                                                }
                                                None => view! { <span class="chip">{label}</span> }.into_any(),
                                            }
                                        })
                                        .collect_view()}
                                    </div>
                                </td>
                            </tr>
                        }
                    })
                    .collect_view()}
            </tbody>
        </table>
    }
}
