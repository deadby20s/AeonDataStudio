//! Runs page: list of pipeline runs, and a run's funnel and removed documents.

use aeon_shared::{JobKind, RemovedDoc, RemovedPage, RunReport, RunSummary};
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api;
use crate::app::{Page, use_app};
use crate::format;
use crate::pipelines::{Draft, StepFunnel};

const PAGE_SIZE: u64 = 25;

#[component]
pub fn RunsPage() -> impl IntoView {
    let ctx = use_app();
    move || match ctx.selected_run.get() {
        Some(id) => view! { <RunDetail id/> }.into_any(),
        None => view! { <RunList/> }.into_any(),
    }
}

fn state_class(state: &str) -> &'static str {
    match state {
        "completed" => "state ok",
        "failed" => "state failed",
        "running" => "state running",
        _ => "state muted",
    }
}

#[component]
fn RunList() -> impl IntoView {
    let ctx = use_app();
    let runs = RwSignal::new(None::<Result<Vec<RunSummary>, String>>);
    // Reload when a pipeline run finishes (read with get(): see DatasetList).
    let finished_runs = Memo::new(move |_| {
        ctx.jobs.with(|jobs| jobs.iter().filter(|j| j.kind == JobKind::PipelineRun && j.state.is_finished()).count())
    });
    Effect::new(move |_| {
        let _ = finished_runs.get();
        spawn_local(async move {
            runs.set(Some(api::list_runs().await.map_err(|e| e.to_string())));
        });
    });

    view! {
        <header class="page-header">
            <h1>"Runs"</h1>
        </header>
        {move || match runs.get() {
            None => view! { <p class="muted">"Loading…"</p> }.into_any(),
            Some(Err(e)) => view! { <p class="error">{e}</p> }.into_any(),
            Some(Ok(list)) if list.is_empty() => {
                view! {
                    <div class="empty">
                        <h2>"No runs yet"</h2>
                        <p class="muted">"Run a pipeline on a dataset from the Pipelines page."</p>
                    </div>
                }
                    .into_any()
            }
            Some(Ok(list)) => {
                view! {
                    <table class="table clickable">
                        <thead>
                            <tr>
                                <th>"Pipeline"</th>
                                <th>"Input → output"</th>
                                <th class="num">"Kept"</th>
                                <th>"State"</th>
                                <th>"Started"</th>
                                <th class="num">"Took"</th>
                            </tr>
                        </thead>
                        <tbody>
                            {list
                                .into_iter()
                                .map(|r| {
                                    let id = r.id.clone();
                                    let kept = match (r.docs_out, r.docs_in) {
                                        (Some(o), Some(i)) => format!("{} ({})", format::count(o), format::percent(o, i)),
                                        _ => "—".into(),
                                    };
                                    let took = r.finished_at_ms.map(|f| format::duration(r.started_at_ms, f)).unwrap_or_default();
                                    view! {
                                        <tr tabindex="0" on:click=move |_| ctx.selected_run.set(Some(id.clone()))>
                                            <td class="strong">{r.pipeline_name}</td>
                                            <td>
                                                {r.input_name.unwrap_or_else(|| "(deleted)".into())} " → "
                                                {r.output_name.unwrap_or_else(|| "—".into())}
                                            </td>
                                            <td class="num">{kept}</td>
                                            <td>
                                                <span class=state_class(&r.state)>{r.state.clone()}</span>
                                                {r.error.map(|e| view! { <div class="muted small">{e}</div> })}
                                            </td>
                                            <td>{format::datetime(r.started_at_ms)}</td>
                                            <td class="num">{took}</td>
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

#[component]
fn RunDetail(id: String) -> impl IntoView {
    let ctx = use_app();
    let report = RwSignal::new(None::<Result<RunReport, String>>);
    let summary = RwSignal::new(None::<RunSummary>);
    let confirm_delete = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let filter = RwSignal::new((None::<u64>, None::<String>));

    {
        let id = id.clone();
        spawn_local(async move {
            if let Ok(list) = api::list_runs().await {
                summary.set(list.into_iter().find(|r| r.id == id));
            }
            report.set(Some(api::get_run(&id).await.map_err(|e| e.to_string())));
        });
    }

    let open_dataset = move |dataset_id: String| {
        ctx.selected_dataset.set(Some(dataset_id));
        ctx.page.set(Page::Datasets);
    };
    let delete = {
        let id = id.clone();
        move |_| {
            let id = id.clone();
            spawn_local(async move {
                match api::delete_run(&id).await {
                    Ok(()) => ctx.selected_run.set(None),
                    Err(e) => error.set(Some(e.to_string())),
                }
            });
        }
    };

    view! {
        <div class="breadcrumb">
            <button class="link" on:click=move |_| ctx.selected_run.set(None)>"← Runs"</button>
        </div>
        {move || error.get().map(|e| view! { <p class="error">{e}</p> })}
        <header class="page-header">
            <div>
                <h1>
                    {move || {
                        summary.get().map(|s| s.pipeline_name).or_else(|| {
                            report.get().and_then(|r| r.ok()).map(|r| r.pipeline.name)
                        })
                    }}
                </h1>
                <p class="muted small">
                    <span class="mono">{id.clone()}</span>
                    {move || {
                        summary
                            .get()
                            .map(|s| {
                                let took = s.finished_at_ms.map(|f| format!(" · took {}", format::duration(s.started_at_ms, f)));
                                format!(" · {} · started {}{}", s.state, format::datetime(s.started_at_ms), took.unwrap_or_default())
                            })
                    }}
                </p>
            </div>
            <div class="actions">
                <Show
                    when=move || confirm_delete.get()
                    fallback=move || view! { <button class="danger-outline" on:click=move |_| confirm_delete.set(true)>"Delete run…"</button> }
                >
                    <span class="small">"Delete the run and its removed-documents log? The output dataset is kept."</span>
                    <button on:click=move |_| confirm_delete.set(false)>"Keep"</button>
                    <button class="danger" on:click=delete.clone()>"Delete"</button>
                </Show>
            </div>
        </header>

        {move || match report.get() {
            None => view! { <p class="muted">"Loading…"</p> }.into_any(),
            Some(Err(e)) => {
                let reason = summary.get().and_then(|s| s.error);
                view! {
                    <div class="callout">
                        <div>
                            <div>{e}</div>
                            {reason.map(|r| view! { <div class="error small">{r}</div> })}
                        </div>
                    </div>
                }
                    .into_any()
            }
            Some(Ok(r)) => {
                let removed = r.docs_in - r.docs_out;
                let input_id = r.input_dataset_id.clone();
                let output_id = r.output_dataset_id.clone();
                let spec = r.pipeline.clone();
                let steps_for_filter = r.steps.clone();
                let run_id = r.id.clone();
                view! {
                    <div class="lineage">
                        <button class="link" on:click=move |_| open_dataset(input_id.clone())>{r.input_name.clone()}</button>
                        <span class="muted">" → " {r.pipeline.name.clone()} " → "</span>
                        <button class="link" on:click=move |_| open_dataset(output_id.clone())>{r.output_name.clone()}</button>
                        <span class="spacer"></span>
                        <button
                            class="small-button"
                            title="Open this run's recipe in the pipeline editor as a new, unsaved pipeline"
                            on:click=move |_| {
                                let mut s = spec.clone();
                                s.name = format!("{} (copy)", s.name);
                                let mut d = Draft::from_spec(s, None);
                                d.dirty = true;
                                ctx.draft.set(Some(d));
                                ctx.page.set(Page::Pipelines);
                            }
                        >
                            "Open recipe in editor"
                        </button>
                    </div>
                    <div class="stat-grid">
                        <div class="stat">
                            <div class="stat-label">"Documents in"</div>
                            <div class="stat-value">{format::count(r.docs_in)}</div>
                        </div>
                        <div class="stat">
                            <div class="stat-label">"Kept"</div>
                            <div class="stat-value">{format::count(r.docs_out)}</div>
                            <div class="stat-note">{format::percent(r.docs_out, r.docs_in)}</div>
                        </div>
                        <div class="stat">
                            <div class="stat-label">"Removed"</div>
                            <div class="stat-value">{format::count(removed)}</div>
                            <div class="stat-note">{format::percent(removed, r.docs_in)}</div>
                        </div>
                        <div class="stat">
                            <div class="stat-label">"Characters kept"</div>
                            <div class="stat-value">{format::percent(r.chars_out, r.chars_in)}</div>
                            <div class="stat-note">
                                {format!("{} of {}", format::count(r.chars_out), format::count(r.chars_in))}
                            </div>
                        </div>
                    </div>
                    <section class="card">
                        <h2>"Steps"</h2>
                        <StepFunnel
                            steps=r.steps.clone()
                            on_rule=Some(Callback::new(move |(step, rule): (u64, String)| filter.set((Some(step), Some(rule)))))
                        />
                        <p class="muted small">"Click a rule to see the documents it removed."</p>
                    </section>
                    <RemovedBrowser run_id steps=steps_for_filter filter total=removed/>
                }
                    .into_any()
            }
        }}
    }
}

#[component]
fn RemovedBrowser(
    run_id: String,
    steps: Vec<aeon_shared::StepStats>,
    filter: RwSignal<(Option<u64>, Option<String>)>,
    total: u64,
) -> impl IntoView {
    let run_id = StoredValue::new(run_id);
    let steps = StoredValue::new(steps);
    let start = RwSignal::new(0u64);
    let history = RwSignal::new(Vec::<u64>::new());
    let page = RwSignal::new(None::<Result<RemovedPage, String>>);
    let loading = RwSignal::new(false);
    let expanded = RwSignal::new(None::<(u64, Option<Result<RemovedDoc, String>>)>);

    let load = move |from: u64| {
        loading.set(true);
        expanded.set(None);
        let (step, rule) = filter.get_untracked();
        spawn_local(async move {
            let result = api::browse_removed(&run_id.get_value(), from, PAGE_SIZE, step, rule.as_deref()).await;
            page.set(Some(result.map_err(|e| e.to_string())));
            start.set(from);
            loading.set(false);
        });
    };
    // Reload from the start whenever the filter changes (and once at first).
    Effect::new(move |_| {
        let _ = filter.get();
        history.set(Vec::new());
        load(0);
    });

    let step_label = move |index: u64| {
        steps.with_value(|s| s.iter().find(|s| s.index == index).map(|s| format!("{}. {}", s.index + 1, s.label)))
            .unwrap_or_else(|| format!("Step {}", index + 1))
    };
    let toggle = move |index: u64| {
        if expanded.with_untracked(|e| e.as_ref().is_some_and(|(i, _)| *i == index)) {
            expanded.set(None);
            return;
        }
        expanded.set(Some((index, None)));
        spawn_local(async move {
            let doc = api::get_removed(&run_id.get_value(), index).await.map_err(|e| e.to_string());
            expanded.update(|e| {
                if let Some((i, slot)) = e
                    && *i == index {
                        *slot = Some(doc);
                    }
            });
        });
    };
    let next = move |_| {
        if let Some(Ok(p)) = page.get_untracked()
            && let Some(n) = p.next_index {
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

    view! {
        <section class="card">
            <div class="browser-head">
                <h2 class="inline">"Removed documents"</h2>
                <div class="row-buttons">
                    <select
                        aria-label="Filter by step"
                        on:change=move |ev| {
                            let v = event_target_value(&ev);
                            filter.set((v.parse().ok(), None));
                        }
                    >
                        <option value="" selected=move || filter.get().0.is_none()>"All steps"</option>
                        {steps
                            .get_value()
                            .into_iter()
                            .filter(|s| s.dropped > 0)
                            .map(|s| {
                                let index = s.index;
                                view! {
                                    <option value=index.to_string() selected=move || filter.get().0 == Some(index)>
                                        {format!("{}. {} ({})", s.index + 1, s.label, format::count(s.dropped))}
                                    </option>
                                }
                            })
                            .collect_view()}
                    </select>
                    {move || {
                        let (step, rule) = filter.get();
                        let rules = step
                            .and_then(|i| steps.with_value(|s| s.iter().find(|s| s.index == i).map(|s| s.drops_by_rule.clone())))
                            .unwrap_or_default();
                        (rules.len() > 1)
                            .then(|| {
                                view! {
                                    <select
                                        aria-label="Filter by rule"
                                        on:change=move |ev| {
                                            let v = event_target_value(&ev);
                                            filter.set((step, (!v.is_empty()).then_some(v)));
                                        }
                                    >
                                        <option value="" selected=rule.is_none()>"All rules"</option>
                                        {rules
                                            .into_iter()
                                            .map(|r| {
                                                let sel = rule.as_deref() == Some(r.rule.as_str());
                                                view! {
                                                    <option value=r.rule.clone() selected=sel>
                                                        {format!("{} ({})", r.rule, format::count(r.count))}
                                                    </option>
                                                }
                                            })
                                            .collect_view()}
                                    </select>
                                }
                            })
                    }}
                </div>
            </div>
            <div class="pager">
                <span class="muted small" aria-live="polite">
                    {move || {
                        let Some(Ok(p)) = page.get() else { return String::new() };
                        let filtered = filter.with(|f| f.0.is_some() || f.1.is_some());
                        let mut s = format!("{} shown", p.docs.len());
                        if filtered {
                            s.push_str(&format!(" · searched {} of {} removed", format::count(p.scanned_to), format::count(total)));
                            if p.search_incomplete {
                                s.push_str(" · paused after 2 s, use Next to continue");
                            }
                        } else {
                            s.push_str(&format!(" of {} removed", format::count(total)));
                        }
                        s
                    }}
                    {move || loading.get().then_some(" · loading…")}
                </span>
                <span class="spacer"></span>
                <button disabled=move || loading.get() || history.with(Vec::is_empty) on:click=prev>"← Prev"</button>
                <button
                    disabled=move || loading.get() || !matches!(page.get(), Some(Ok(ref p)) if p.next_index.is_some())
                    on:click=next
                >
                    "Next →"
                </button>
            </div>
            {move || match page.get() {
                None => view! { <p class="muted">"Loading…"</p> }.into_any(),
                Some(Err(e)) => view! { <p class="error">{e}</p> }.into_any(),
                Some(Ok(p)) if p.docs.is_empty() => view! { <p class="muted">"Nothing removed here."</p> }.into_any(),
                Some(Ok(p)) => {
                    view! {
                        <ol class="doc-list">
                            {p.docs
                                .into_iter()
                                .map(|d| {
                                    let index = d.index;
                                    let is_open = move || expanded.with(|e| e.as_ref().is_some_and(|(i, _)| *i == index));
                                    view! {
                                        <li class="doc" class:open=is_open>
                                            <button class="doc-head" aria-expanded=move || is_open().to_string() on:click=move |_| toggle(index)>
                                                <span class="doc-row">"row " {format::count(d.input_row + 1)}</span>
                                                <span class="mono doc-id" title=d.id.clone()>{d.id.clone()}</span>
                                                <span class="muted small">{step_label(d.step_index)}</span>
                                                <span class="rule">{d.rule.clone()}</span>
                                                <span class="muted small">{d.detail.clone()}</span>
                                                <span class="spacer"></span>
                                                <span class="muted small">{format::count(d.chars)} " chars"</span>
                                            </button>
                                            <Show
                                                when=is_open
                                                fallback={
                                                    let preview = d.preview.clone();
                                                    let truncated = d.truncated;
                                                    move || view! { <div class="doc-text preview">{preview.clone()} {truncated.then_some("…")}</div> }
                                                }
                                            >
                                                {move || match expanded.get().and_then(|(_, d)| d) {
                                                    None => view! { <p class="muted small">"Loading…"</p> }.into_any(),
                                                    Some(Err(e)) => view! { <p class="error">{e}</p> }.into_any(),
                                                    Some(Ok(doc)) => view! {
                                                        <div class="muted small doc-source-line">{doc.source}</div>
                                                        <div class="doc-text">{doc.text}</div>
                                                    }.into_any(),
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
