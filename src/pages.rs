use aeon_shared::{JobId, JobLink, JobState, JobStatus};
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api;
use crate::app::{AppCtx, Page, use_app};
use crate::format;

/// Opens (or initializes) a workspace and refreshes app state. Returns the error message on failure.
async fn open_workspace(ctx: AppCtx, path: String) -> Result<(), String> {
    api::open_workspace(&path).await.map_err(|e| e.to_string())?;
    ctx.page.set(Page::Datasets);
    ctx.selected_dataset.set(None);
    ctx.selected_run.set(None);
    ctx.draft.set(None);
    ctx.inspect_run.set(None);
    ctx.refresh().await.map_err(|e| e.to_string())
}

#[component]
pub fn WelcomePage() -> impl IntoView {
    let ctx = use_app();
    let error = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);
    let typed_path = RwSignal::new(String::new());

    let open = move |path: String| {
        busy.set(true);
        error.set(None);
        spawn_local(async move {
            if let Err(e) = open_workspace(ctx, path).await {
                error.set(Some(e));
            }
            busy.set(false);
        });
    };

    let pick = move |_| {
        busy.set(true);
        error.set(None);
        spawn_local(async move {
            match api::pick_folder().await {
                Ok(Some(path)) => {
                    if let Err(e) = open_workspace(ctx, path).await {
                        error.set(Some(e));
                    }
                }
                Ok(None) => {}
                Err(e) => error.set(Some(e.to_string())),
            }
            busy.set(false);
        });
    };

    let forget = move |path: String| {
        spawn_local(async move {
            match api::forget_recent_workspace(&path).await {
                Ok(list) => ctx.recent.set(list),
                Err(e) => error.set(Some(e.to_string())),
            }
        });
    };

    view! {
        <div class="welcome">
            <div class="welcome-card">
                <img src="public/logo.svg" alt="" class="welcome-logo"/>
                <h1>"AeonDataStudio"</h1>
                <p class="muted">
                    "Prepare text data for pretraining: import, clean, deduplicate, inspect and export. Everything stays on this machine."
                </p>

                <button class="primary" on:click=pick disabled=move || busy.get()>
                    "Open or create workspace…"
                </button>
                <p class="muted small">
                    "Pick a folder that already holds a workspace, or an empty folder to create a new one."
                </p>

                <form
                    class="path-form"
                    on:submit=move |ev| {
                        ev.prevent_default();
                        let path = typed_path.get_untracked();
                        if !path.trim().is_empty() {
                            open(path);
                        }
                    }
                >
                    <input
                        type="text"
                        placeholder="…or paste a folder path"
                        prop:value=move || typed_path.get()
                        on:input=move |ev| typed_path.set(event_target_value(&ev))
                    />
                    <button type="submit" disabled=move || busy.get()>"Open"</button>
                </form>

                {move || error.get().map(|e| view! { <p class="error">{e}</p> })}

                <Show when=move || ctx.recent.with(|r| !r.is_empty())>
                    <h2 class="section-title">"Recent"</h2>
                    <ul class="recent-list">
                        {move || {
                            ctx.recent
                                .get()
                                .into_iter()
                                .map(|path| {
                                    let open_path = path.clone();
                                    let forget_path = path.clone();
                                    let title = path.clone();
                                    view! {
                                        <li>
                                            <button
                                                class="link recent-path"
                                                title=title
                                                disabled=move || busy.get()
                                                on:click=move |_| open(open_path.clone())
                                            >
                                                {path}
                                            </button>
                                            <button
                                                class="icon-button"
                                                title="Remove from recent"
                                                aria-label="Remove from recent"
                                                on:click=move |_| forget(forget_path.clone())
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
        </div>
    }
}

#[component]
pub fn JobsPage() -> impl IntoView {
    let ctx = use_app();
    let error = RwSignal::new(None::<String>);
    // Only the list of ids drives which rows exist; each row updates itself in place.
    // Rebuilding rows on every progress event would replace the Cancel button mid-click.
    let job_ids = Memo::new(move |_| ctx.jobs.with(|jobs| jobs.iter().map(|j| j.id).collect::<Vec<_>>()));

    let run_self_test = move |_| {
        spawn_local(async move {
            if let Err(e) = api::start_self_test_job(60).await {
                error.set(Some(e.to_string()));
            }
        });
    };

    let clear_finished = move |_| {
        spawn_local(async move {
            match api::clear_finished_jobs().await {
                Ok(()) => ctx.jobs.update(|jobs| jobs.retain(|j| !j.state.is_finished())),
                Err(e) => error.set(Some(e.to_string())),
            }
        });
    };

    view! {
        <header class="page-header">
            <h1>"Jobs"</h1>
            <div class="actions">
                <button on:click=clear_finished>"Clear finished"</button>
                <button on:click=run_self_test>"Run self-test"</button>
            </div>
        </header>
        {move || error.get().map(|e| view! { <p class="error">{e}</p> })}
        <Show
            when=move || job_ids.with(|ids| !ids.is_empty())
            fallback=|| {
                view! {
                    <div class="empty">
                        <h2>"No jobs"</h2>
                        <p class="muted">
                            "Imports, pipeline runs and exports will show their progress here. "
                            "Run self-test starts a 3-second job that only reports progress."
                        </p>
                    </div>
                }
            }
        >
            <ul class="job-list">
                <For each=move || job_ids.get() key=|id| *id children=move |id| view! { <JobRow id error/> }/>
            </ul>
        </Show>
    }
}

#[component]
pub fn JobRow(id: JobId, error: RwSignal<Option<String>>) -> impl IntoView {
    let ctx = use_app();
    let job = Memo::new(move |_| ctx.jobs.with(|jobs| jobs.iter().find(|j| j.id == id).cloned()));
    let get = move |f: fn(&JobStatus) -> String| job.with(|j| j.as_ref().map(f).unwrap_or_default());
    let active = Memo::new(move |_| job.with(|j| j.as_ref().is_some_and(|j| !j.state.is_finished())));
    let fraction = move || job.with(|j| j.as_ref().and_then(|j| j.progress.fraction()));

    let state_class = move || {
        job.with(|j| match j.as_ref().map(|j| j.state) {
            Some(JobState::Queued | JobState::Running) | None => "state running",
            Some(JobState::Completed) => "state ok",
            Some(JobState::Failed) => "state failed",
            Some(JobState::Cancelled) => "state muted",
        })
    };

    let cancel = move |_| {
        spawn_local(async move {
            if let Err(e) = api::cancel_job(id).await {
                error.set(Some(e.to_string()));
            }
        });
    };

    view! {
        <li class="job">
            <div class="job-head">
                <span class="job-title">{move || get(|j| j.title.clone())}</span>
                <span class="muted small">{move || get(|j| j.kind.label().to_string())}</span>
                <span class=state_class>{move || get(|j| j.state.label().to_string())}</span>
                <span class="spacer"></span>
                <Show when=move || active.get()>
                    <button class="small-button" on:click=cancel>"Cancel"</button>
                </Show>
                {move || {
                    job.with(|j| j.as_ref().and_then(|j| j.link.clone()))
                        .map(|link| {
                            let label = match &link {
                                JobLink::Dataset(_) => "Open dataset",
                                JobLink::Run(_) => "Open run",
                                JobLink::Export(_) => "Open exports",
                            };
                            view! {
                                <button
                                    class="small-button"
                                    on:click=move |_| match link.clone() {
                                        JobLink::Dataset(id) => {
                                            ctx.selected_dataset.set(Some(id));
                                            ctx.page.set(Page::Datasets);
                                        }
                                        JobLink::Run(id) => {
                                            ctx.selected_run.set(Some(id));
                                            ctx.page.set(Page::Runs);
                                        }
                                        JobLink::Export(_) => ctx.page.set(Page::Exports),
                                    }
                                >
                                    {label}
                                </button>
                            }
                        })
                }}
            </div>
            <div
                class="progress"
                class:indeterminate=move || fraction().is_none() && active.get()
                role="progressbar"
                aria-valuemin="0"
                aria-valuemax="100"
                aria-valuenow=move || fraction().map(|f| ((f * 100.0).round() as u32).to_string())
            >
                <div
                    class="progress-bar"
                    style:width=move || format!("{:.1}%", fraction().unwrap_or(0.0) * 100.0)
                ></div>
            </div>
            <div class="job-foot muted small">
                <span>
                    {move || {
                        get(|j| match j.progress.total {
                            Some(total) => format!("{} / {}", format::count(j.progress.done), format::count(total)),
                            None => format::count(j.progress.done),
                        })
                    }}
                </span>
                <span>{move || get(|j| j.message.clone().unwrap_or_default())}</span>
                <span class="spacer"></span>
                <span>{move || get(|j| format::datetime(j.started_at_ms))}</span>
            </div>
        </li>
    }
}

#[component]
pub fn SettingsPage() -> impl IntoView {
    let ctx = use_app();
    let error = RwSignal::new(None::<String>);
    let log_note = RwSignal::new(None::<String>);
    let update_note = RwSignal::new(None::<String>);
    let update_busy = RwSignal::new(false);

    let close = move |_| {
        spawn_local(async move {
            let result = async {
                api::close_workspace().await?;
                ctx.refresh().await
            }
            .await;
            if let Err(e) = result {
                error.set(Some(e.to_string()));
            }
        });
    };

    view! {
        <header class="page-header">
            <h1>"Settings"</h1>
        </header>
        {move || error.get().map(|e| view! { <p class="error">{e}</p> })}
        <section class="card">
            <h2>"Workspace"</h2>
            {move || {
                ctx.workspace
                    .get()
                    .map(|w| {
                        view! {
                            <dl class="props">
                                <dt>"Name"</dt>
                                <dd>{w.name}</dd>
                                <dt>"Folder"</dt>
                                <dd class="mono">{w.path}</dd>
                                <dt>"Created"</dt>
                                <dd>{format::datetime(w.created_at_ms)}</dd>
                                <dt>"Datasets"</dt>
                                <dd>{format::count(w.dataset_count)}</dd>
                                <dt>"Runs"</dt>
                                <dd>{format::count(w.run_count)}</dd>
                            </dl>
                        }
                    })
            }}
            <button on:click=close>"Close workspace"</button>
        </section>
        <section class="card">
            <h2>"Updates"</h2>
            {move || {
                if !ctx.updates_configured.get() {
                    return view! {
                        <p class="muted small">
                            "This build can't update itself. Download new versions from the Releases page on GitHub."
                        </p>
                    }
                        .into_any();
                }
                view! {
                    {move || match ctx.update.get() {
                        Some(u) => view! {
                            <p>{format!("Version {} is available.", u.version)}</p>
                            {u.notes.map(|n| view! { <p class="muted small pre">{n}</p> })}
                            <button class="primary" disabled=move || update_busy.get() on:click=move |_| {
                                update_busy.set(true);
                                update_note.set(Some("Downloading and installing… the app restarts when done.".into()));
                                spawn_local(async move {
                                    if let Err(e) = api::install_update().await {
                                        update_note.set(Some(e.to_string()));
                                        update_busy.set(false);
                                    }
                                });
                            }>"Install and restart"</button>
                        }
                            .into_any(),
                        None => view! {
                            <button disabled=move || update_busy.get() on:click=move |_| {
                                update_busy.set(true);
                                update_note.set(None);
                                spawn_local(async move {
                                    match api::check_for_update().await {
                                        Ok(Some(u)) => ctx.update.set(Some(u)),
                                        Ok(None) => update_note.set(Some("You have the latest version.".into())),
                                        Err(e) => update_note.set(Some(e.to_string())),
                                    }
                                    update_busy.set(false);
                                });
                            }>"Check for updates"</button>
                        }
                            .into_any(),
                    }}
                    {move || update_note.get().map(|n| view! { <p class="muted small">{n}</p> })}
                }
                    .into_any()
            }}
        </section>
        <section class="card">
            <h2>"Demo data"</h2>
            <p class="muted small">
                "Adds “Demo: mixed web text” (3,000 generated documents, each tagged with what kind of problem it contains in meta.category) and a “Demo cleanup” pipeline to this workspace."
            </p>
            <button on:click=move |_| {
                spawn_local(async move {
                    match api::create_demo_dataset().await {
                        Ok(_) => log_note.set(Some("Demo dataset is being created; see Datasets or Jobs.".into())),
                        Err(e) => error.set(Some(e.to_string())),
                    }
                });
            }>"Create demo dataset"</button>
        </section>
        <section class="card">
            <h2>"Logs"</h2>
            <p class="muted small">
                "The app writes AeonDataStudio.log (jobs started, finished, failed). Attach it to bug reports."
            </p>
            <button on:click=move |_| {
                spawn_local(async move {
                    match api::open_log_folder().await {
                        Ok(path) => log_note.set(Some(format!("Opened {path}"))),
                        Err(e) => error.set(Some(e.to_string())),
                    }
                });
            }>"Open log folder"</button>
            {move || log_note.get().map(|n| view! { <p class="muted small mono">{n}</p> })}
        </section>
        <section class="card">
            <h2>"About"</h2>
            <dl class="props">
                <dt>"Version"</dt>
                <dd>{move || ctx.version.get()}</dd>
            </dl>
        </section>
    }
}
