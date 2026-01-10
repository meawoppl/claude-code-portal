use crate::components::{MessageRenderer, ProxyTokenSetup};
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
    let show_new_session = use_state(|| false);

    // Fetch sessions function
    let fetch_sessions = {
        let sessions = sessions.clone();
        let loading = loading.clone();

        Callback::from(move |set_loading: bool| {
            let sessions = sessions.clone();
            let loading = loading.clone();

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
                if set_loading {
                    loading.set(false);
                }
            });
        })
    };

    // Initial fetch on mount
    {
        let fetch_sessions = fetch_sessions.clone();
        use_effect_with((), move |_| {
            fetch_sessions.emit(true);
            || ()
        });
    }

    // Polling interval for auto-refresh (every 5 seconds)
    {
        let fetch_sessions = fetch_sessions.clone();
        use_effect_with((), move |_| {
            let interval = gloo::timers::callback::Interval::new(5_000, move || {
                fetch_sessions.emit(false);
            });

            // Keep interval alive until component unmounts
            move || drop(interval)
        });
    }

    // Manual refresh when refresh_trigger changes (e.g., after delete)
    {
        let fetch_sessions = fetch_sessions.clone();
        let refresh = *refresh_trigger;

        use_effect_with(refresh, move |_| {
            // Skip initial render (refresh == 0)
            if refresh > 0 {
                fetch_sessions.emit(false);
            }
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

    let toggle_new_session = {
        let show_new_session = show_new_session.clone();
        Callback::from(move |_| {
            show_new_session.set(!*show_new_session);
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
                <div class="header-actions">
                    <button
                        class={classes!("new-session-button", if *show_new_session { "active" } else { "" })}
                        onclick={toggle_new_session.clone()}
                    >
                        { if *show_new_session { "− Close" } else { "+ New Session" } }
                    </button>
                    <button class="logout-button">{ "Logout" }</button>
                </div>
            </header>

            // Modal overlay for new session
            if *show_new_session {
                <div class="modal-overlay" onclick={toggle_new_session.clone()}>
                    <div class="modal-content" onclick={Callback::from(|e: MouseEvent| e.stop_propagation())}>
                        <ProxyTokenSetup />
                    </div>
                </div>
            }

            if *loading {
                <div class="loading">
                    <div class="spinner"></div>
                    <p>{ "Loading sessions..." }</p>
                </div>
            } else if sessions.is_empty() {
                <div class="empty-state">
                    <h2>{ "No Sessions Yet" }</h2>
                    <p>{ "Click \"+ New Session\" above to connect a Claude proxy from your remote machine." }</p>
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

/// Message data from the API
#[derive(Clone, PartialEq, serde::Deserialize)]
struct MessageData {
    role: String,
    content: String,
}

#[derive(Clone, PartialEq, serde::Deserialize)]
struct MessagesResponse {
    messages: Vec<MessageData>,
}

#[function_component(SessionPortal)]
fn session_portal(props: &SessionPortalProps) -> Html {
    let navigator = use_navigator().unwrap();
    let session = &props.session;
    let messages = use_state(|| Vec::<MessageData>::new());
    let awaiting_input = use_state(|| false);

    // Fetch messages for this session
    {
        let messages = messages.clone();
        let awaiting_input = awaiting_input.clone();
        let session_id = session.id;

        use_effect_with(session_id, move |_| {
            wasm_bindgen_futures::spawn_local(async move {
                let api_endpoint = utils::api_url(&format!("/api/sessions/{}/messages", session_id));
                match Request::get(&api_endpoint).send().await {
                    Ok(response) => {
                        if let Ok(data) = response.json::<MessagesResponse>().await {
                            // Check if last message is a "result" type (Claude finished, awaiting input)
                            let is_awaiting = data.messages.last().map_or(false, |msg| {
                                // Try to parse the content as JSON and check if it's a result
                                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&msg.content) {
                                    parsed.get("type").and_then(|t| t.as_str()) == Some("result")
                                } else {
                                    false
                                }
                            });
                            awaiting_input.set(is_awaiting);
                            messages.set(data.messages);
                        }
                    }
                    Err(e) => {
                        log::error!("Failed to fetch messages for session {}: {:?}", session_id, e);
                    }
                }
            });
            || ()
        });
    }

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
    let show_awaiting_glow = is_active && *awaiting_input;

    let card_class = classes!(
        "session-portal",
        if !is_active { Some("inactive") } else { None },
        if show_awaiting_glow { Some("awaiting-input") } else { None }
    );

    html! {
        <div class={card_class} onclick={handle_click}>
            <button class="delete-button" onclick={handle_delete} title="Remove session">
                { "x" }
            </button>
            <div class="portal-terminal">
                <div class="portal-terminal-header">
                    <div class="terminal-dots">
                        <span class="dot red"></span>
                        <span class="dot yellow"></span>
                        <span class="dot green"></span>
                    </div>
                    <span class="session-name">{ &session.session_name }</span>
                    {
                        if is_active {
                            html! { <span class="status-badge active">{ "LIVE" }</span> }
                        } else {
                            html! { <span class="status-badge">{ "offline" }</span> }
                        }
                    }
                </div>
                <div class="portal-terminal-viewport">
                    <div class="portal-terminal-content">
                        {
                            if messages.is_empty() {
                                html! {
                                    <div class="empty-terminal">
                                        <span class="muted">{ "No messages yet..." }</span>
                                    </div>
                                }
                            } else {
                                // Use the same MessageRenderer as the full terminal
                                html! {
                                    <div class="messages">
                                        {
                                            messages.iter().map(|msg| {
                                                html! {
                                                    <MessageRenderer json={msg.content.clone()} />
                                                }
                                            }).collect::<Html>()
                                        }
                                    </div>
                                }
                            }
                        }
                    </div>
                </div>
                {
                    if show_awaiting_glow {
                        html! {
                            <div class="portal-input-bar">
                                <span class="prompt-symbol">{ ">" }</span>
                                <span class="cursor blink">{ "▊" }</span>
                            </div>
                        }
                    } else {
                        html! {}
                    }
                }
            </div>
            <div class="portal-overlay">
                <span class="portal-hint">
                    { if is_active { "Click to connect" } else { "Click to view history" } }
                </span>
            </div>
        </div>
    }
}
