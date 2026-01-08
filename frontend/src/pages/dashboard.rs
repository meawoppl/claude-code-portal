use yew::prelude::*;
use yew_router::prelude::*;
use gloo_net::http::Request;
use shared::SessionInfo;
use crate::Route;

#[function_component(DashboardPage)]
pub fn dashboard_page() -> Html {
    let sessions = use_state(|| Vec::<SessionInfo>::new());
    let loading = use_state(|| true);

    // Fetch sessions on mount
    {
        let sessions = sessions.clone();
        let loading = loading.clone();

        use_effect_with((), move |_| {
            wasm_bindgen_futures::spawn_local(async move {
                match Request::get("http://localhost:3000/api/sessions")
                    .send()
                    .await
                {
                    Ok(response) => {
                        if let Ok(data) = response.json::<serde_json::Value>().await {
                            if let Some(session_list) = data.get("sessions") {
                                if let Ok(parsed) = serde_json::from_value::<Vec<SessionInfo>>(session_list.clone()) {
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

    html! {
        <div class="dashboard-container">
            <header class="dashboard-header">
                <h1>{ "Your Claude Code Sessions" }</h1>
                <button class="logout-button">{ "Logout" }</button>
            </header>

            <div class="sessions-grid">
                if *loading {
                    <div class="loading">
                        <div class="spinner"></div>
                        <p>{ "Loading sessions..." }</p>
                    </div>
                } else if sessions.is_empty() {
                    <div class="empty-state">
                        <h2>{ "No Active Sessions" }</h2>
                        <p>{ "Start a Claude Code session on your remote machine:" }</p>
                        <pre class="code-block">
                            { "claude-proxy --backend-url ws://localhost:3000 \\\n" }
                            { "              --session-name my-machine" }
                        </pre>
                    </div>
                } else {
                    {
                        sessions.iter().map(|session| {
                            html! {
                                <SessionPortal key={session.id.to_string()} session={session.clone()} />
                            }
                        }).collect::<Html>()
                    }
                }
            </div>
        </div>
    }
}

#[derive(Properties, PartialEq, Clone)]
struct SessionPortalProps {
    session: SessionInfo,
}

#[function_component(SessionPortal)]
fn session_portal(props: &SessionPortalProps) -> Html {
    let navigator = use_navigator().unwrap();
    let session = &props.session;

    let handle_click = {
        let navigator = navigator.clone();
        let session_id = session.id;

        Callback::from(move |_| {
            navigator.push(&Route::Terminal { id: session_id.to_string() });
        })
    };

    let status_class = format!("status-indicator {}", session.status.as_str());

    html! {
        <div class="session-portal" onclick={handle_click}>
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
                        <span>{ &session.last_activity }</span>
                    </div>
                    <div class="terminal-line">
                        <span class="prompt">{ "$ " }</span>
                        <span class="cursor blink">{ "▊" }</span>
                    </div>
                </div>
            </div>
            <div class="portal-overlay">
                <span class="portal-hint">{ "Click to connect" }</span>
            </div>
        </div>
    }
}
