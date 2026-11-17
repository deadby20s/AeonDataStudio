use aeon_shared::{JobState, JobStatus, WorkspaceInfo};
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api;
use crate::datasets::DatasetsPage;
use crate::pages::{JobsPage, SettingsPage, WelcomePage};
use crate::pipelines::{Draft, PipelinesPage};
use crate::exports::ExportsPage;
use crate::inspector::InspectorPage;
use crate::runs::RunsPage;
use crate::sql::SqlPage;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Datasets,
    Pipelines,
    Runs,
    Inspector,
    Sql,
    Exports,
    Jobs,
    Settings,
}

impl Page {
    const NAV: [Page; 8] = [
        Page::Datasets,
        Page::Pipelines,
        Page::Runs,
        Page::Inspector,
        Page::Sql,
        Page::Exports,
        Page::Jobs,
        Page::Settings,
    ];

    fn label(self) -> &'static str {
        match self {
            Page::Datasets => "Datasets",
            Page::Pipelines => "Pipelines",
            Page::Runs => "Runs",
            Page::Inspector => "Inspector",
            Page::Sql => "SQL",
            Page::Exports => "Exports",
            Page::Jobs => "Jobs",
            Page::Settings => "Settings",
        }
    }
}

/// App-wide state, provided as context.
#[derive(Clone, Copy)]
pub struct AppCtx {
    pub loaded: RwSignal<bool>,
    pub load_error: RwSignal<Option<String>>,
    pub version: RwSignal<String>,
    pub workspace: RwSignal<Option<WorkspaceInfo>>,
    pub recent: RwSignal<Vec<String>>,
    pub jobs: RwSignal<Vec<JobStatus>>,
    pub page: RwSignal<Page>,
    /// Dataset shown on the Datasets page; `None` shows the list.
    pub selected_dataset: RwSignal<Option<String>>,
    /// Run shown on the Runs page; `None` shows the list.
    pub selected_run: RwSignal<Option<String>>,
    /// Pipeline being edited; `None` shows the list of saved pipelines.
    pub draft: RwSignal<Option<Draft>>,
    /// Run shown in the Inspector; `None` picks the newest.
    pub inspect_run: RwSignal<Option<String>>,
    pub updates_configured: RwSignal<bool>,
    pub sql_available: RwSignal<bool>,
    /// Set when a newer version is available.
    pub update: RwSignal<Option<aeon_shared::UpdateInfo>>,
}

impl AppCtx {
    fn new() -> Self {
        Self {
            loaded: RwSignal::new(false),
            load_error: RwSignal::new(None),
            version: RwSignal::new(String::new()),
            workspace: RwSignal::new(None),
            recent: RwSignal::new(Vec::new()),
            jobs: RwSignal::new(Vec::new()),
            page: RwSignal::new(Page::Datasets),
            selected_dataset: RwSignal::new(None),
            selected_run: RwSignal::new(None),
            draft: RwSignal::new(None),
            inspect_run: RwSignal::new(None),
            updates_configured: RwSignal::new(false),
            sql_available: RwSignal::new(false),
            update: RwSignal::new(None),
        }
    }

    /// Inserts or replaces a job, keeping newest first.
    pub fn upsert_job(&self, status: JobStatus) {
        self.jobs.update(|jobs| {
            match jobs.iter_mut().find(|j| j.id == status.id) {
                Some(existing) => *existing = status,
                None => jobs.push(status),
            }
            jobs.sort_by_key(|j| std::cmp::Reverse(j.id));
        });
    }

    /// Re-reads app info (workspace and recent list) from the backend.
    pub async fn refresh(&self) -> api::ApiResult<()> {
        let info = api::get_app_info().await?;
        self.version.set(info.version);
        self.updates_configured.set(info.updates_configured);
        self.sql_available.set(info.sql_available);
        self.workspace.set(info.workspace);
        self.recent.set(info.recent_workspaces);
        Ok(())
    }
}

pub fn use_app() -> AppCtx {
    expect_context::<AppCtx>()
}

#[component]
pub fn App() -> impl IntoView {
    let ctx = AppCtx::new();
    provide_context(ctx);

    spawn_local(async move {
        let result: api::ApiResult<()> = async {
            ctx.refresh().await?;
            for job in api::list_jobs().await? {
                ctx.upsert_job(job);
            }
            api::on_job_update(move |status| ctx.upsert_job(status)).await?;
            if ctx.updates_configured.get_untracked() {
                // Quietly: a failed check (offline, …) just shows nothing.
                if let Ok(Some(u)) = api::check_for_update().await {
                    ctx.update.set(Some(u));
                }
            }
            Ok(())
        }
        .await;
        if let Err(e) = result {
            ctx.load_error.set(Some(e.to_string()));
        }
        ctx.loaded.set(true);
    });

    view! {
        {move || {
            if !ctx.loaded.get() {
                view! { <div class="splash">"Loading…"</div> }.into_any()
            } else if let Some(err) = ctx.load_error.get() {
                view! {
                    <div class="splash">
                        <p class="error">"Could not start: " {err}</p>
                    </div>
                }
                    .into_any()
            } else if ctx.workspace.with(Option::is_none) {
                view! { <WelcomePage/> }.into_any()
            } else {
                view! { <Shell/> }.into_any()
            }
        }}
    }
}

#[component]
fn Shell() -> impl IntoView {
    let ctx = use_app();
    let running = move || {
        ctx.jobs.with(|jobs| {
            jobs.iter().filter(|j| matches!(j.state, JobState::Queued | JobState::Running)).count()
        })
    };

    view! {
        <div class="shell">
            <nav class="sidebar">
                <div class="brand">
                    <img src="public/logo.svg" alt="" class="brand-mark"/>
                    <span>"AeonDataStudio"</span>
                </div>
                <div class="workspace-chip" title=move || ctx.workspace.with(|w| w.as_ref().map(|w| w.path.clone()).unwrap_or_default())>
                    <span class="muted small">"Workspace"</span>
                    <span class="workspace-name">
                        {move || ctx.workspace.with(|w| w.as_ref().map(|w| w.name.clone()).unwrap_or_default())}
                    </span>
                </div>
                <ul class="nav">
                    {Page::NAV
                        .into_iter()
                        .map(|p| {
                            view! {
                                <li>
                                    <button
                                        class="nav-item"
                                        class:active=move || ctx.page.get() == p
                                        on:click=move |_| {
                                            if p == Page::Datasets {
                                                ctx.selected_dataset.set(None);
                                            }
                                            if p == Page::Runs {
                                                ctx.selected_run.set(None);
                                            }
                                            ctx.page.set(p);
                                        }
                                    >
                                        <span>{p.label()}</span>
                                        {(p == Page::Jobs)
                                            .then(|| {
                                                view! {
                                                    <Show when=move || running() != 0>
                                                        <span class="badge">{running}</span>
                                                    </Show>
                                                }
                                            })}
                                    </button>
                                </li>
                            }
                        })
                        .collect_view()}
                </ul>
                {move || {
                    ctx.update.get().map(|u| view! {
                        <button class="update-pill" on:click=move |_| ctx.page.set(Page::Settings)>
                            {format!("Update {} available", u.version)}
                        </button>
                    })
                }}
                <div class="sidebar-footer muted small">"v" {move || ctx.version.get()} " alpha"</div>
            </nav>
            <main class="content">
                {move || match ctx.page.get() {
                    Page::Datasets => view! { <DatasetsPage/> }.into_any(),
                    Page::Pipelines => view! { <PipelinesPage/> }.into_any(),
                    Page::Runs => view! { <RunsPage/> }.into_any(),
                    Page::Inspector => view! { <InspectorPage/> }.into_any(),
                    Page::Exports => view! { <ExportsPage/> }.into_any(),
                    Page::Sql => view! { <SqlPage/> }.into_any(),
                    Page::Jobs => view! { <JobsPage/> }.into_any(),
                    Page::Settings => view! { <SettingsPage/> }.into_any(),
                }}
            </main>
        </div>
    }
}
