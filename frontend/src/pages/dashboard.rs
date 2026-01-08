use crate::utils;
use crate::Route;
use gloo_net::http::Request;
use shared::SessionInfo;
use uuid::Uuid;
use yew::prelude::*;
use yew_router::prelude::*;

#[function_component(DashboardPage)]
pub fn dashboard_page() -> Html {
    let sessions = use_state(|| Vec::<SessionInfo>::new());
    let loading = use_state(|| true);
    let refresh_trigger = use_state(|| 0u32);

    // Fetch sessions on mount and when refresh_trigger changes
    {
        let sessions = sessions.clone();
        let loading = loading.clone();
        let refresh = *refresh_trigger;

        use_effect_with(refresh, move |_| {
            wasm_bindgen_futures::spawn_local(async move {
                let api_endpoint = utils::api_url("/api/sessions");
                match Request::get(&api_endpoint).send().await {
                    Ok(response) => {
                        if let Ok(data) = response.json::<serde_json::Value>().await {
                            if let Some(session_list) = data.get("sessions") {
                                if let Ok(parsed) =
                                    serde_json::from_value::<Vec<SessionInfo>>(session_list.clone())
                                {
                                    sessions.set(parsed);
                                }
                            }
                        }
                    }
                    Err(e) => {
                        log::error!("Failed to fetch sessions: {:?}", e);
                    }
                }
                loading.set(false);
            });
            || ()
        });
    }

    let on_delete = {
        let refresh_trigger = refresh_trigger.clone();
        Callback::from(move |session_id: Uuid| {
            let refresh_trigger = refresh_trigger.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let api_endpoint = utils::api_url(&format!("/api/sessions/{}", session_id));
                match Request::delete(&api_endpoint).send().await {
                    Ok(response) if response.status() == 204 => {
                        refresh_trigger.set(*refresh_trigger + 1);
                    }
                    Ok(response) => {
                        log::error!("Failed to delete session: status {}", response.status());
                    }
                    Err(e) => {
                        log::error!("Failed to delete session: {:?}", e);
                    }
                }
            });
        })
    };

    // Separate sessions by status
    let active_sessions: Vec<_> = sessions
        .iter()
        .filter(|s| s.status.as_str() == "active")
        .cloned()
        .collect();
    let inactive_sessions: Vec<_> = sessions
        .iter()
        .filter(|s| s.status.as_str() != "active")
        .cloned()
        .collect();

    html! {
        <div class="dashboard-container">
            <header class="dashboard-header">
                <h1>{ "Your Claude Code Sessions" }</h1>
                <button class="logout-button">{ "Logout" }</button>
            </header>

            if *loading {
                <div class="loading">
                    <div class="spinner"></div>
                    <p>{ "Loading sessions..." }</p>
                </div>
            } else if sessions.is_empty() {
                <div class="empty-state">
                    <h2>{ "No Sessions" }</h2>
                    <p>{ "Start a Claude Code session on your remote machine:" }</p>
                    <pre class="code-block">
                        { "claude-proxy --backend-url ws://localhost:3000" }
                    </pre>
                </div>
            } else {
                <>
                    if !active_sessions.is_empty() {
                        <section class="session-section">
                            <h2 class="section-title">{ "Active Sessions" }</h2>
                            <div class="sessions-grid">
                                {
                                    active_sessions.iter().map(|session| {
                                        html! {
                                            <SessionPortal
                                                key={session.id.to_string()}
                                                session={session.clone()}
                                                on_delete={on_delete.clone()}
                                            />
                                        }
                                    }).collect::<Html>()
                                }
                            </div>
                        </section>
                    }

                    if !inactive_sessions.is_empty() {
                        <section class="session-section inactive-section">
                            <h2 class="section-title muted">{ "Recent Sessions" }</h2>
                            <div class="sessions-grid">
                                {
                                    inactive_sessions.iter().map(|session| {
                                        html! {
                                            <SessionPortal
                                                key={session.id.to_string()}
                                                session={session.clone()}
                                                on_delete={on_delete.clone()}
                                            />
                                        }
                                    }).collect::<Html>()
                                }
                            </div>
                        </section>
                    }
                </>
            }
        </div>
    }
}

#[derive(Properties, PartialEq, Clone)]
struct SessionPortalProps {
    session: SessionInfo,
    on_delete: Callback<Uuid>,
}

#[function_component(SessionPortal)]
fn session_portal(props: &SessionPortalProps) -> Html {
    let navigator = use_navigator().unwrap();
    let session = &props.session;

    let handle_click = {
        let navigator = navigator.clone();
        let session_id = session.id;

        Callback::from(move |_| {
            navigator.push(&Route::Terminal {
                id: session_id.to_string(),
            });
        })
    };

    let handle_delete = {
        let on_delete = props.on_delete.clone();
        let session_id = session.id;

        Callback::from(move |e: MouseEvent| {
            e.stop_propagation();
            on_delete.emit(session_id);
        })
    };

    let is_active = session.status.as_str() == "active";
    let status_class = format!(
        "status-indicator {}",
        if is_active { "active" } else { "disconnected" }
    );
    let card_class = format!("session-portal {}", if is_active { "" } else { "inactive" });

    // Format last activity time
    let last_activity = format_time_ago(&session.last_activity);

    html! {
        <div class={card_class} onclick={handle_click}>
            <button class="delete-button" onclick={handle_delete} title="Remove session">
                { "x" }
            </button>
            <div class="portal-terminal">
                <div class="terminal-header">
                    <span class={status_class}></span>
                    <span class="session-name">{ &session.session_name }</span>
                </div>
                <div class="terminal-preview-content">
                    <div class="terminal-line">
                        <span class="muted">{ "Working directory:" }</span>
                    </div>
                    <div class="terminal-line">
                        <span class="path">
                            { session.working_directory.as_ref().unwrap_or(&"Unknown".to_string()) }
                        </span>
                    </div>
                    <div class="terminal-line">
                        <span class="muted">{ "Last activity: " }</span>
                        <span>{ last_activity }</span>
                    </div>
                    if is_active {
                        <div class="terminal-line">
                            <span class="prompt">{ "$ " }</span>
                            <span class="cursor blink">{ "▊" }</span>
                        </div>
                    }
                </div>
            </div>
            <div class="portal-overlay">
                <span class="portal-hint">
                    { if is_active { "Click to connect" } else { "Click to view history" } }
                </span>
            </div>
        </div>
    }
}

fn format_time_ago(timestamp: &str) -> String {
    // Use js_sys to get current time and parse timestamp
    use js_sys::Date;

    let now_ms = Date::now();
    let parsed = Date::parse(timestamp);

    if parsed.is_nan() {
        return timestamp.to_string();
    }

    let diff_secs = ((now_ms - parsed) / 1000.0) as i64;

    if diff_secs < 60 {
        "just now".to_string()
    } else if diff_secs < 3600 {
        format!("{} min ago", diff_secs / 60)
    } else if diff_secs < 86400 {
        format!("{} hours ago", diff_secs / 3600)
    } else {
        format!("{} days ago", diff_secs / 86400)
    }
}
