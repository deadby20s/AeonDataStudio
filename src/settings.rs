//! Settings cards: appearance (themes), plugins and the local API.

use aeon_shared::{ApiStatus, PluginInfo, Theme, ThemeSettings};
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api;
use crate::theme;

// ---------------------------------------------------------------------------------------------
// Appearance
// ---------------------------------------------------------------------------------------------

#[component]
pub fn AppearanceCard() -> impl IntoView {
    let settings = RwSignal::new(ThemeSettings::default());
    let error = RwSignal::new(None::<String>);
    let import_text = RwSignal::new(String::new());
    let show_import = RwSignal::new(false);

    spawn_local(async move {
        if let Ok(s) = api::get_theme_settings().await {
            settings.set(s);
        }
    });

    // Applies and saves. Saving is quick, so every change is kept.
    let save = move |s: ThemeSettings| {
        theme::apply(&s);
        settings.set(s.clone());
        spawn_local(async move {
            match api::set_theme_settings(&s).await {
                Ok(()) => error.set(None),
                Err(e) => error.set(Some(e.to_string())),
            }
        });
    };

    let active = move || settings.with(|s| s.active.clone());
    let custom_active = move || settings.with(|s| s.custom.iter().position(|t| t.name == s.active));

    let pick = move |name: String| {
        let mut s = settings.get_untracked();
        s.active = if name == "System" { String::new() } else { name };
        save(s);
    };

    let new_theme = move |_| {
        let mut s = settings.get_untracked();
        let t = theme::new_custom(&s);
        s.active = t.name.clone();
        s.custom.push(t);
        save(s);
    };

    let edit_custom = move |f: &dyn Fn(&mut Theme)| {
        let mut s = settings.get_untracked();
        if let Some(i) = s.custom.iter().position(|t| t.name == s.active) {
            f(&mut s.custom[i]);
            s.active = s.custom[i].name.clone();
            save(s);
        }
    };

    let delete_custom = move |_| {
        let mut s = settings.get_untracked();
        s.custom.retain(|t| t.name != s.active);
        s.active = String::new();
        save(s);
    };

    let import = move |_| {
        let text = import_text.get_untracked();
        match serde_json::from_str::<Theme>(&text) {
            Ok(mut t) if !t.name.trim().is_empty() => {
                let mut s = settings.get_untracked();
                while s.custom.iter().any(|c| c.name == t.name) {
                    t.name.push_str(" (imported)");
                }
                s.active = t.name.clone();
                s.custom.push(t);
                save(s);
                import_text.set(String::new());
                show_import.set(false);
            }
            Ok(_) => error.set(Some("The theme needs a name".into())),
            Err(e) => error.set(Some(format!("Not a theme: {e}"))),
        }
    };

    view! {
        <section class="card">
            <h2>"Appearance"</h2>
            <div class="theme-grid">
                {theme::BUILTIN
                    .iter()
                    .map(|(label, palette)| {
                        let label: &'static str = label;
                        let name = label.to_string();
                        let n2 = name.clone();
                        let swatch = palette.map(|p| (p[0], p[1], p[6]));
                        let is_active = move || {
                            let a = active();
                            a == n2 || (a.is_empty() && n2 == "System")
                        };
                        view! {
                            <button class="theme-chip" class:active=is_active on:click=move |_| pick(name.clone())>
                                {match swatch {
                                    Some((bg, surface, accent)) => view! {
                                        <span class="swatch" style=format!("background:{bg}")>
                                            <span style=format!("background:{surface}")></span>
                                            <span style=format!("background:{accent}")></span>
                                        </span>
                                    }
                                        .into_any(),
                                    None => view! { <span class="swatch system"></span> }.into_any(),
                                }}
                                {label}
                            </button>
                        }
                    })
                    .collect_view()}
                {move || {
                    settings
                        .get()
                        .custom
                        .into_iter()
                        .map(|t| {
                            let name = t.name.clone();
                            let n2 = name.clone();
                            let get = |k: &str| t.vars.get(k).cloned().unwrap_or_default();
                            let (bg, surface, accent) = (get("--bg"), get("--surface"), get("--accent"));
                            view! {
                                <button class="theme-chip" class:active=move || active() == n2 on:click=move |_| pick(name.clone())>
                                    <span class="swatch" style=format!("background:{bg}")>
                                        <span style=format!("background:{surface}")></span>
                                        <span style=format!("background:{accent}")></span>
                                    </span>
                                    {t.name.clone()}
                                </button>
                            }
                        })
                        .collect_view()
                }}
            </div>
            <div class="row-buttons">
                <button on:click=new_theme>"New custom theme"</button>
                <button on:click=move |_| show_import.update(|v| *v = !*v)>"Import theme…"</button>
            </div>
            {move || {
                show_import
                    .get()
                    .then(|| {
                        view! {
                            <div class="field">
                                <span class="label">"Paste a theme (JSON)"</span>
                                <textarea
                                    rows="5"
                                    class="mono-input"
                                    placeholder=r##"{"name": "My theme", "vars": {"--bg": "#101418", "--accent": "#ff8a4c"}}"##
                                    prop:value=move || import_text.get()
                                    on:input=move |ev| import_text.set(event_target_value(&ev))
                                ></textarea>
                                <div class="row-buttons">
                                    <button class="primary" on:click=import>"Import"</button>
                                </div>
                            </div>
                        }
                    })
            }}
            {move || {
                custom_active()
                    .map(|i| {
                        let t = settings.with(|s| s.custom[i].clone());
                        let json = serde_json::to_string_pretty(&t).unwrap_or_default();
                        view! {
                            <div class="theme-editor">
                                <label class="field">
                                    <span class="label">"Theme name"</span>
                                    <input
                                        prop:value=t.name.clone()
                                        on:change=move |ev| {
                                            let name = event_target_value(&ev).trim().to_string();
                                            let taken = settings.with_untracked(|s| s.custom.iter().any(|c| c.name == name))
                                                || theme::BUILTIN.iter().any(|(b, _)| *b == name);
                                            if name.is_empty() || taken {
                                                error.set(Some("Choose a name no other theme uses".into()));
                                            } else {
                                                edit_custom(&|t: &mut Theme| t.name = name.clone());
                                            }
                                        }
                                    />
                                </label>
                                <div class="color-grid">
                                    {theme::VARS
                                        .iter()
                                        .map(|(key, label)| {
                                            let value = t.vars.get(*key).cloned().unwrap_or_default();
                                            view! {
                                                <label class="color-field">
                                                    <input
                                                        type="color"
                                                        prop:value=value
                                                        on:change=move |ev| {
                                                            let v = event_target_value(&ev);
                                                            edit_custom(&|t: &mut Theme| {
                                                                t.vars.insert(key.to_string(), v.clone());
                                                            });
                                                        }
                                                    />
                                                    <span>{*label}</span>
                                                </label>
                                            }
                                        })
                                        .collect_view()}
                                </div>
                                <details>
                                    <summary class="small">"Share this theme (JSON)"</summary>
                                    <textarea rows="6" class="mono-input" readonly prop:value=json></textarea>
                                </details>
                                <div class="row-buttons">
                                    <button class="danger-outline" on:click=delete_custom>"Delete theme"</button>
                                </div>
                            </div>
                        }
                    })
            }}
            {move || error.get().map(|e| view! { <p class="error">{e}</p> })}
        </section>
    }
}

// ---------------------------------------------------------------------------------------------
// Plugins
// ---------------------------------------------------------------------------------------------

#[component]
pub fn PluginsCard() -> impl IntoView {
    let plugins = RwSignal::new(None::<Result<Vec<PluginInfo>, String>>);
    let load = move || {
        spawn_local(async move {
            plugins.set(Some(api::list_plugins().await.map_err(|e| e.to_string())));
        });
    };
    load();

    view! {
        <section class="card">
            <h2>"Plugins"</h2>
            <p class="muted small">
                "Plugins are programs in this workspace's plugins folder that a pipeline's Plugin step runs on each document: quality classifiers (fastText, FineWeb-Edu), custom filters, anything that reads JSON lines. Each plugin is a folder with a plugin.toml. See docs/plugins.md; examples are in the repository's examples/plugins folder."
            </p>
            {move || match plugins.get() {
                None => view! { <p class="muted small">"Loading…"</p> }.into_any(),
                Some(Err(e)) => view! { <p class="error">{e}</p> }.into_any(),
                Some(Ok(list)) if list.is_empty() => {
                    view! { <p class="muted small">"No plugins installed."</p> }.into_any()
                }
                Some(Ok(list)) => view! {
                    <ul class="plugin-list">
                        {list
                            .into_iter()
                            .map(|p| {
                                view! {
                                    <li>
                                        <div>
                                            <strong>{p.name.clone()}</strong>
                                            " "
                                            <span class="mono muted small">{p.id.clone()}</span>
                                        </div>
                                        {p.description.clone().map(|d| view! { <div class="small">{d}</div> })}
                                        {p.error.clone().map(|e| view! { <div class="error small">{e}</div> })}
                                        {(!p.command.is_empty())
                                            .then(|| view! { <div class="mono muted small">{p.command.join(" ")}</div> })}
                                        {(!p.options.is_empty())
                                            .then(|| {
                                                view! {
                                                    <dl class="props small">
                                                        {p.options
                                                            .iter()
                                                            .map(|o| {
                                                                view! {
                                                                    <dt class="mono">{format!("{}={}", o.key, o.default)}</dt>
                                                                    <dd>{o.help.clone()}</dd>
                                                                }
                                                            })
                                                            .collect_view()}
                                                    </dl>
                                                }
                                            })}
                                    </li>
                                }
                            })
                            .collect_view()}
                    </ul>
                }
                    .into_any(),
            }}
            <div class="row-buttons">
                <button on:click=move |_| {
                    spawn_local(async move {
                        if let Err(e) = api::open_plugins_folder().await {
                            plugins.set(Some(Err(e.to_string())));
                        }
                    });
                }>"Open plugins folder"</button>
                <button on:click=move |_| load()>"Refresh"</button>
            </div>
        </section>
    }
}

// ---------------------------------------------------------------------------------------------
// Local API
// ---------------------------------------------------------------------------------------------

#[component]
pub fn ApiCard() -> impl IntoView {
    let status = RwSignal::new(ApiStatus::default());
    let error = RwSignal::new(None::<String>);
    let port = RwSignal::new(String::new());
    let show_token = RwSignal::new(false);

    spawn_local(async move {
        if let Ok(s) = api::api_status().await {
            status.set(s);
        }
    });

    let set = move |enabled: bool| {
        let p = port.get_untracked().trim().parse::<u16>().ok();
        spawn_local(async move {
            match api::set_api_enabled(enabled, p).await {
                Ok(s) => {
                    error.set(None);
                    status.set(s);
                }
                Err(e) => error.set(Some(e.to_string())),
            }
        });
    };

    view! {
        <section class="card">
            <h2>"Local API"</h2>
            <p class="muted small">
                "Lets scripts and other programs on this computer use AeonDataStudio over HTTP: list and browse datasets, preview and run pipelines, export, query with SQL. It listens on 127.0.0.1 only and every request needs the token. Jobs started through it appear in Jobs. The same commands are available without the app through the aeon command-line tool (aeon serve, aeon mcp). See docs/api.md."
            </p>
            {move || {
                let s = status.get();
                if s.running {
                    let url = s.url.clone().unwrap_or_default();
                    let token = s.token.clone().unwrap_or_default();
                    let example = format!("curl -H \"Authorization: Bearer {token}\" {url}/api/v1/datasets");
                    view! {
                        <dl class="props">
                            <dt>"Address"</dt>
                            <dd class="mono">{format!("{url}/api/v1")}</dd>
                            <dt>"Token"</dt>
                            <dd class="mono">
                                {move || if show_token.get() { token.clone() } else { "••••••••••••".to_string() }}
                                " "
                                <button class="link" on:click=move |_| show_token.update(|v| *v = !*v)>
                                    {move || if show_token.get() { "Hide" } else { "Show" }}
                                </button>
                            </dd>
                        </dl>
                        {move || show_token.get().then(|| view! { <p class="mono small pre">{example.clone()}</p> })}
                        <div class="row-buttons">
                            <button on:click=move |_| set(false)>"Turn off"</button>
                            <button on:click=move |_| {
                                spawn_local(async move {
                                    match api::new_api_token().await {
                                        Ok(s) => status.set(s),
                                        Err(e) => error.set(Some(e.to_string())),
                                    }
                                });
                            }>"New token"</button>
                        </div>
                    }
                        .into_any()
                } else {
                    view! {
                        <div class="row-buttons">
                            <label class="field inline">
                                <span class="label">"Port"</span>
                                <input
                                    type="number"
                                    min="1024"
                                    max="65535"
                                    placeholder="7878"
                                    prop:value=move || port.get()
                                    on:input=move |ev| port.set(event_target_value(&ev))
                                />
                            </label>
                            <button class="primary" on:click=move |_| set(true)>"Turn on"</button>
                        </div>
                        {s.error.map(|e| view! { <p class="error">{e}</p> })}
                    }
                        .into_any()
                }
            }}
            {move || error.get().map(|e| view! { <p class="error">{e}</p> })}
        </section>
    }
}
