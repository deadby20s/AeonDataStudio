use aeon_shared::{JobState, JobStatus, WorkspaceInfo};
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api;
use crate::pages::{
    DatasetsPage, JobsPage, PlaceholderPage, SettingsPage, WelcomePage,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Datasets,
    Pipelines,
    Runs,
    Inspector,
    Jobs,
    Settings,
}

impl Page {
    const NAV: [Page; 6] =
        [Page::Datasets, Page::Pipelines, Page::Runs, Page::Inspector, Page::Jobs, Page::Settings];

    fn label(self) -> &'static str {
        match self {
            Page::Datasets => "Datasets",
            Page::Pipelines => "Pipelines",
            Page::Runs => "Runs",
            Page::Inspector => "Inspector",
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
        }
    }

    /// Inserts or replaces a job, keeping newest first.
    pub fn upsert_job(&self, status: JobStatus) {
        self.jobs.update(|jobs| {
            match jobs.iter_mut().find(|j| j.id == status.id) {
                Some(existing) => *existing = status,
                None => jobs.push(status),
            }
            jobs.sort_by(|a, b| b.id.cmp(&a.id));
        });
    }

    /// Re-reads app info (workspace and recent list) from the backend.
    pub async fn refresh(&self) -> api::ApiResult<()> {
        let info = api::get_app_info().await?;
        self.version.set(info.version);
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
                                        on:click=move |_| ctx.page.set(p)
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
                <div class="sidebar-footer muted small">"v" {move || ctx.version.get()}</div>
            </nav>
            <main class="content">
                {move || match ctx.page.get() {
                    Page::Datasets => view! { <DatasetsPage/> }.into_any(),
                    Page::Pipelines => {
                        view! {
                            <PlaceholderPage
                                title="Pipelines"
                                milestone="M2"
                                body="Build cleaning pipelines from normalization, filter and dedup steps, and save them as TOML recipes."
                            />
                        }
                            .into_any()
                    }
                    Page::Runs => {
                        view! {
                            <PlaceholderPage
                                title="Runs"
                                milestone="M2"
                                body="Each pipeline run will be listed here with per-step counts and the documents it removed."
                            />
                        }
                            .into_any()
                    }
                    Page::Inspector => {
                        view! {
                            <PlaceholderPage
                                title="Inspector"
                                milestone="M3"
                                body="Browse removed documents by reason, compare before and after, and tune thresholds."
                            />
                        }
                            .into_any()
                    }
                    Page::Jobs => view! { <JobsPage/> }.into_any(),
                    Page::Settings => view! { <SettingsPage/> }.into_any(),
                }}
            </main>
        </div>
    }
}
