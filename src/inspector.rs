//! Inspector: look closely at what a run did. Removed samples grouped by reason, duplicate
//! pairs side by side, and before/after for documents normalization changed.

use aeon_shared::{ChangedPage, Document, RemovedDoc, RunReport, RunSummary, StepStats};
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api;
use crate::app::{Page, use_app};
use crate::diff::{self, Op};
use crate::format;
use crate::pipelines::StepFunnel;

/// Diffs with more edits than this are shown side by side without highlighting.
const MAX_DIFF_EDITS: usize = 4000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Reasons,
    Duplicates,
    Changes,
}

fn is_dedup(s: &StepStats) -> bool {
    s.kind.ends_with("_dedup")
}

#[component]
pub fn InspectorPage() -> impl IntoView {
    let ctx = use_app();
    let runs = RwSignal::new(None::<Result<Vec<RunSummary>, String>>);
    let report = RwSignal::new(None::<Result<RunReport, String>>);
    let tab = RwSignal::new(Tab::Reasons);

    spawn_local(async move {
        let list = api::list_runs().await.map(|l| l.into_iter().filter(|r| r.state == "completed").collect::<Vec<_>>());
        if let Ok(l) = &list
            && ctx.inspect_run.get_untracked().is_none() {
                ctx.inspect_run.set(l.first().map(|r| r.id.clone()));
            }
        runs.set(Some(list.map_err(|e| e.to_string())));
    });
    Effect::new(move |_| {
        let Some(id) = ctx.inspect_run.get() else { return };
        report.set(None);
        spawn_local(async move {
            report.set(Some(api::get_run(&id).await.map_err(|e| e.to_string())));
        });
    });

    view! {
        <header class="page-header">
            <h1>"Inspector"</h1>
            {move || match runs.get() {
                Some(Ok(list)) if !list.is_empty() => {
                    view! {
                        <select
                            aria-label="Run to inspect"
                            on:change=move |ev| ctx.inspect_run.set(Some(event_target_value(&ev)))
                        >
                            {list
                                .into_iter()
                                .map(|r| {
                                    let selected = ctx.inspect_run.get_untracked().as_deref() == Some(r.id.as_str());
                                    let label = format!(
                                        "{} on {} · {}",
                                        r.pipeline_name,
                                        r.input_name.clone().unwrap_or_else(|| "(deleted)".into()),
                                        format::datetime(r.started_at_ms),
                                    );
                                    view! { <option value=r.id.clone() selected=selected>{label}</option> }
                                })
                                .collect_view()}
                        </select>
                    }
                        .into_any()
                }
                _ => ().into_any(),
            }}
        </header>
        {move || match runs.get() {
            None => view! { <p class="muted">"Loading…"</p> }.into_any(),
            Some(Err(e)) => view! { <p class="error">{e}</p> }.into_any(),
            Some(Ok(list)) if list.is_empty() => {
                view! {
                    <div class="empty">
                        <h2>"Nothing to inspect yet"</h2>
                        <p class="muted">"Run a pipeline from the Pipelines page, then come back to see what it removed and changed."</p>
                    </div>
                }
                    .into_any()
            }
            Some(Ok(_)) => {
                view! {
                    {move || match report.get() {
                        None => view! { <p class="muted">"Loading…"</p> }.into_any(),
                        Some(Err(e)) => view! { <p class="error">{e}</p> }.into_any(),
                        Some(Ok(r)) => view! { <RunInspector report=r tab/> }.into_any(),
                    }}
                }
                    .into_any()
            }
        }}
    }
}

#[component]
fn RunInspector(report: RunReport, tab: RwSignal<Tab>) -> impl IntoView {
    let ctx = use_app();
    let report = StoredValue::new(report);
    let r = report.get_value();
    let has_dups = r.steps.iter().any(|s| is_dedup(s) && s.dropped > 0);
    let has_changes = r.steps.iter().any(|s| s.modified > 0);
    let run_id = r.id.clone();
    let tab_button = move |t: Tab, label: &'static str, count: Option<String>| {
        view! {
            <button class="tab" class:active=move || tab.get() == t role="tab" aria-selected=move || (tab.get() == t).to_string() on:click=move |_| tab.set(t)>
                {label}
                {count.map(|c| view! { <span class="muted small">" " {c}</span> })}
            </button>
        }
    };
    let removed = r.docs_in - r.docs_out;
    let dup_count: u64 = r.steps.iter().filter(|s| is_dedup(s)).map(|s| s.dropped).sum();

    view! {
        <div class="lineage">
            <span>
                <strong>{r.pipeline.name.clone()}</strong>
                <span class="muted">
                    {format!(
                        " · {} → {} documents ({} removed)",
                        format::count(r.docs_in),
                        format::count(r.docs_out),
                        format::percent(removed, r.docs_in),
                    )}
                </span>
            </span>
            <span class="spacer"></span>
            <button
                class="small-button"
                on:click=move |_| {
                    ctx.selected_run.set(Some(run_id.clone()));
                    ctx.page.set(Page::Runs);
                }
            >
                "Open run"
            </button>
        </div>
        <details class="card">
            <summary>
                <h2 class="inline">"Steps"</h2>
            </summary>
            <StepFunnel steps=r.steps.clone() on_rule=None/>
        </details>
        <div class="tabs" role="tablist">
            {tab_button(Tab::Reasons, "Removed by reason", Some(format::count(removed)))}
            {tab_button(Tab::Duplicates, "Duplicates", Some(format::count(dup_count)))}
            {tab_button(Tab::Changes, "Before / after", None)}
        </div>
        {move || match tab.get() {
            Tab::Reasons => view! { <ReasonsTab report=report.get_value()/> }.into_any(),
            Tab::Duplicates if has_dups => view! { <DuplicatesTab report=report.get_value()/> }.into_any(),
            Tab::Duplicates => view! {
                <p class="muted">"This run removed no duplicates. Add an exact or near-duplicate step to the pipeline to find them."</p>
            }.into_any(),
            Tab::Changes if has_changes => view! { <ChangesTab report=report.get_value()/> }.into_any(),
            Tab::Changes => view! { <p class="muted">"No step in this run changed any text."</p> }.into_any(),
        }}
    }
}

// ---------------------------------------------------------------------------------------------
// Removed by reason
// ---------------------------------------------------------------------------------------------

#[component]
fn ReasonsTab(report: RunReport) -> impl IntoView {
    let groups: Vec<(StepStats, String, u64)> = report
        .steps
        .iter()
        .flat_map(|s| s.drops_by_rule.iter().map(move |r| (s.clone(), r.rule.clone(), r.count)))
        .collect();
    if groups.is_empty() {
        return view! { <p class="muted">"This run removed nothing."</p> }.into_any();
    }
    let total_removed = report.docs_in - report.docs_out;
    view! {
        <p class="muted small">
            "Three random examples per reason. Use “Other examples” to draw again. Look for good documents being removed: that means the threshold is too strict."
        </p>
        <div class="reason-grid">
            {groups
                .into_iter()
                .map(|(step, rule, count)| {
                    view! {
                        <RuleSamples
                            run_id=report.id.clone()
                            step=step
                            rule=rule
                            count=count
                            total_removed=total_removed
                        />
                    }
                })
                .collect_view()}
        </div>
    }
        .into_any()
}

#[component]
fn RuleSamples(run_id: String, step: StepStats, rule: String, count: u64, total_removed: u64) -> impl IntoView {
    let seed = RwSignal::new(1u64);
    let samples = RwSignal::new(None::<Result<Vec<RemovedDoc>, String>>);
    let (run_id, rule_c, step_index) = (StoredValue::new(run_id), StoredValue::new(rule.clone()), step.index);
    Effect::new(move |_| {
        let s = seed.get();
        samples.set(None);
        spawn_local(async move {
            let result = api::sample_removed(&run_id.get_value(), step_index, &rule_c.get_value(), 3, s).await;
            samples.set(Some(result.map_err(|e| e.to_string())));
        });
    });
    view! {
        <section class="card reason">
            <div class="reason-head">
                <div>
                    <span class="rule">{rule}</span>
                    <div class="muted small">{format!("Step {} · {}", step.index + 1, step.label)}</div>
                </div>
                <div class="num">
                    <div class="strong">{format::count(count)}</div>
                    <div class="muted small">
                        {format!(
                            "{} of step input · {} of removals",
                            format::percent(count, step.docs_in),
                            format::percent(count, total_removed),
                        )}
                    </div>
                </div>
            </div>
            {move || match samples.get() {
                None => view! { <p class="muted small">"Loading…"</p> }.into_any(),
                Some(Err(e)) => view! { <p class="error small">{e}</p> }.into_any(),
                Some(Ok(docs)) => {
                    docs.into_iter()
                        .map(|d| {
                            view! {
                                <div class="sample">
                                    <div class="small">
                                        <span class="muted">{d.detail}</span>
                                        <span class="muted mono">" · " {d.id} " · row " {format::count(d.input_row + 1)}</span>
                                    </div>
                                    <ExpandableText text=d.text/>
                                </div>
                            }
                        })
                        .collect_view()
                        .into_any()
                }
            }}
            <div class="row-buttons end">
                <button class="small-button" disabled=move || count <= 3 on:click=move |_| seed.update(|s| *s += 1)>
                    "Other examples"
                </button>
            </div>
        </section>
    }
}

#[component]
fn ExpandableText(text: String) -> impl IntoView {
    let open = RwSignal::new(false);
    let long = text.chars().count() > 600;
    view! {
        <div class="doc-text sample-text" class:clamped=move || long && !open.get()>{text}</div>
        {long.then(|| view! {
            <button class="link small" on:click=move |_| open.update(|o| *o = !*o)>
                {move || if open.get() { "Show less" } else { "Show all" }}
            </button>
        })}
    }
}

// ---------------------------------------------------------------------------------------------
// Duplicates
// ---------------------------------------------------------------------------------------------

#[component]
fn DuplicatesTab(report: RunReport) -> impl IntoView {
    let choices: Vec<(u64, String, String, u64)> = report
        .steps
        .iter()
        .filter(|s| is_dedup(s))
        .flat_map(|s| s.drops_by_rule.iter().map(move |r| (s.index, s.label.clone(), r.rule.clone(), r.count)))
        .collect();
    let choice = RwSignal::new(0usize);
    let seed = RwSignal::new(1u64);
    let pairs = RwSignal::new(None::<Result<Vec<(RemovedDoc, Result<Document, String>)>, String>>);
    let run_id = StoredValue::new(report.id.clone());
    let input_id = StoredValue::new(report.input_dataset_id.clone());
    let choices_v = StoredValue::new(choices.clone());

    Effect::new(move |_| {
        let (c, s) = (choice.get(), seed.get());
        let Some((step_index, _, rule, _)) = choices_v.with_value(|v| v.get(c).cloned()) else { return };
        pairs.set(None);
        spawn_local(async move {
            let result = async {
                let removed = api::sample_removed(&run_id.get_value(), step_index, &rule, 5, s).await?;
                let mut out = Vec::new();
                for d in removed {
                    let kept = match d.related_row {
                        Some(row) => api::get_document(&input_id.get_value(), row).await.map_err(|e| e.to_string()),
                        None => Err("This run did not record which document was kept.".to_string()),
                    };
                    out.push((d, kept));
                }
                Ok::<_, api::ApiError>(out)
            }
            .await;
            pairs.set(Some(result.map_err(|e| e.to_string())));
        });
    });

    view! {
        <div class="row-buttons">
            <select on:change=move |ev| {
                choice.set(event_target_value(&ev).parse().unwrap_or(0));
                seed.set(1);
            }>
                {choices
                    .iter()
                    .enumerate()
                    .map(|(i, (idx, label, rule, count))| {
                        view! { <option value=i.to_string()>{format!("Step {} · {} · {rule} ({})", idx + 1, label, format::count(*count))}</option> }
                    })
                    .collect_view()}
            </select>
            <button class="small-button" on:click=move |_| seed.update(|s| *s += 1)>"Other examples"</button>
        </div>
        <p class="muted small">
            "Each pair shows the copy that was kept (the earliest) and the one that was removed. Highlighted words differ: "
            <span class="del">"only in the kept copy"</span> ", " <span class="ins">"only in the removed copy"</span> "."
        </p>
        {move || match pairs.get() {
            None => view! { <p class="muted">"Loading…"</p> }.into_any(),
            Some(Err(e)) => view! { <p class="error">{e}</p> }.into_any(),
            Some(Ok(list)) => list
                .into_iter()
                .map(|(removed, kept)| {
                    let header = format!(
                        "Removed row {} ({}) duplicates kept row {}",
                        format::count(removed.input_row + 1),
                        removed.id,
                        removed.related_row.map(|r| format::count(r + 1)).unwrap_or_else(|| "?".into()),
                    );
                    view! {
                        <section class="card pair">
                            <div class="small strong">{header}</div>
                            {match kept {
                                Ok(k) => view! { <DiffView before=k.text after=removed.text/> }.into_any(),
                                Err(e) => view! {
                                    <p class="error small">{e}</p>
                                    <ExpandableText text=removed.text/>
                                }.into_any(),
                            }}
                        </section>
                    }
                })
                .collect_view()
                .into_any(),
        }}
    }
}

// ---------------------------------------------------------------------------------------------
// Before / after
// ---------------------------------------------------------------------------------------------

#[component]
fn ChangesTab(report: RunReport) -> impl IntoView {
    let steps: Vec<StepStats> = report.steps.iter().filter(|s| s.modified > 0).cloned().collect();
    let step_filter = RwSignal::new(None::<u64>);
    let start = RwSignal::new(0u64);
    let history = RwSignal::new(Vec::<u64>::new());
    let page = RwSignal::new(None::<Result<ChangedPage, String>>);
    let selected = RwSignal::new(None::<u64>);
    let compare = RwSignal::new(None::<Result<(Document, Document), String>>);
    let run_id = StoredValue::new(report.id.clone());
    let input_id = StoredValue::new(report.input_dataset_id.clone());
    let output_id = StoredValue::new(report.output_dataset_id.clone());
    let all_steps = StoredValue::new(report.steps.clone());

    let load = move |from: u64| {
        let step = step_filter.get_untracked();
        spawn_local(async move {
            let result = api::browse_changed(&run_id.get_value(), step, from, 12).await;
            page.set(Some(result.map_err(|e| e.to_string())));
            start.set(from);
        });
    };
    Effect::new(move |_| {
        let _ = step_filter.get();
        history.set(Vec::new());
        selected.set(None);
        compare.set(None);
        load(0);
    });
    let open = move |output_row: u64, input_row: u64| {
        selected.set(Some(output_row));
        compare.set(None);
        spawn_local(async move {
            let result = async {
                let before = api::get_document(&input_id.get_value(), input_row).await?;
                let after = api::get_document(&output_id.get_value(), output_row).await?;
                Ok::<_, api::ApiError>((before, after))
            }
            .await;
            if selected.get_untracked() == Some(output_row) {
                compare.set(Some(result.map_err(|e| e.to_string())));
            }
        });
    };
    let step_name = move |idx: u64| {
        all_steps.with_value(|s| s.iter().find(|s| s.index == idx).map(|s| s.label.clone())).unwrap_or_default()
    };

    view! {
        <div class="row-buttons">
            <select on:change=move |ev| step_filter.set(event_target_value(&ev).parse().ok())>
                <option value="">"Changed by any step"</option>
                {steps
                    .iter()
                    .map(|s| view! {
                        <option value=s.index.to_string()>
                            {format!("Changed by step {} · {} ({})", s.index + 1, s.label, format::count(s.modified))}
                        </option>
                    })
                    .collect_view()}
            </select>
        </div>
        <div class="changes">
            <div class="changes-list">
                {move || match page.get() {
                    None => view! { <p class="muted small">"Loading…"</p> }.into_any(),
                    Some(Err(e)) => view! { <p class="error">{e}</p> }.into_any(),
                    Some(Ok(p)) => {
                        let status = format!("Searched {} of {} kept documents", format::count(p.scanned_to), format::count(p.total_kept));
                        let next = p.next_start;
                        view! {
                            <ul class="plain-list">
                                {p.items
                                    .into_iter()
                                    .map(|c| {
                                        let (o, i) = (c.output_row, c.input_row);
                                        let steps = c.step_indices.iter().map(|&s| step_name(s)).collect::<Vec<_>>().join(", ");
                                        view! {
                                            <li>
                                                <button class="list-item" class:active=move || selected.get() == Some(o) on:click=move |_| open(o, i)>
                                                    <span>"Output row " {format::count(o + 1)}</span>
                                                    <span class="muted small">{steps}</span>
                                                </button>
                                            </li>
                                        }
                                    })
                                    .collect_view()}
                            </ul>
                            <div class="pager">
                                <span class="muted small">{status}</span>
                                <span class="spacer"></span>
                                <button
                                    class="small-button"
                                    disabled=move || history.with(Vec::is_empty)
                                    on:click=move |_| {
                                        let mut back = None;
                                        history.update(|h| back = h.pop());
                                        if let Some(b) = back {
                                            load(b);
                                        }
                                    }
                                >
                                    "←"
                                </button>
                                <button
                                    class="small-button"
                                    disabled=next.is_none()
                                    on:click=move |_| {
                                        if let Some(n) = next {
                                            history.update(|h| h.push(start.get_untracked()));
                                            load(n);
                                        }
                                    }
                                >
                                    "→"
                                </button>
                            </div>
                        }
                            .into_any()
                    }
                }}
            </div>
            <div class="changes-view">
                {move || match (selected.get(), compare.get()) {
                    (None, _) => view! { <p class="muted">"Pick a document to compare the input with the output."</p> }.into_any(),
                    (Some(_), None) => view! { <p class="muted">"Loading…"</p> }.into_any(),
                    (Some(_), Some(Err(e))) => view! { <p class="error">{e}</p> }.into_any(),
                    (Some(_), Some(Ok((before, after)))) => view! {
                        <div class="small muted mono">{before.id.clone()} " · " {before.source.clone()}</div>
                        <DiffView before=before.text after=after.text/>
                    }.into_any(),
                }}
            </div>
        </div>
    }
}

// ---------------------------------------------------------------------------------------------
// Diff
// ---------------------------------------------------------------------------------------------

#[component]
fn DiffView(before: String, after: String) -> impl IntoView {
    let (ta, tb) = (diff::tokenize(&before), diff::tokenize(&after));
    match diff::diff(&ta, &tb, MAX_DIFF_EDITS) {
        Some(ops) => {
            let (dels, ins) = ops.iter().fold((0, 0), |(d, i), (op, t)| match op {
                Op::Delete => (d + t.chars().count(), i),
                Op::Insert => (d, i + t.chars().count()),
                Op::Equal => (d, i),
            });
            view! {
                <div class="small muted">
                    {if dels == 0 && ins == 0 {
                        "Identical text.".to_string()
                    } else {
                        format!("{} characters removed, {} added", format::count(dels as u64), format::count(ins as u64))
                    }}
                </div>
                <div class="doc-text diff-text">
                    {ops.into_iter()
                        .map(|(op, text)| match op {
                            Op::Equal => view! { <span>{text}</span> }.into_any(),
                            Op::Delete => view! { <del class="del">{diff::show_whitespace(&text)}</del> }.into_any(),
                            Op::Insert => view! { <ins class="ins">{diff::show_whitespace(&text)}</ins> }.into_any(),
                        })
                        .collect_view()}
                </div>
            }
                .into_any()
        }
        None => view! {
            <p class="muted small">"Too many differences to highlight; showing both texts."</p>
            <div class="diff">
                <div>
                    <div class="small-heading">"Before"</div>
                    <div class="doc-text">{before}</div>
                </div>
                <div>
                    <div class="small-heading">"After"</div>
                    <div class="doc-text">{after}</div>
                </div>
            </div>
        }
            .into_any(),
    }
}
