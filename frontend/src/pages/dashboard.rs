use crate::components::{
    group_messages, MessageGroupRenderer, ProxyTokenSetup, ShareDialog, VoiceInput,
};
use crate::utils;
use crate::Route;
use futures_util::{SinkExt, StreamExt};
use gloo::timers::callback::Timeout;
use gloo_net::http::Request;
use gloo_net::websocket::{futures::WebSocket, Message};
use shared::{ProxyMessage, SessionCost, SessionInfo};
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use uuid::Uuid;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;
use web_sys::{Element, HtmlInputElement, KeyboardEvent};
use yew::prelude::*;
use yew_router::prelude::*;

/// Calculate exponential backoff delay for reconnection attempts
fn calculate_backoff(attempt: u32) -> u32 {
    const INITIAL_MS: u32 = 1000;
    const MAX_MS: u32 = 30000;
    INITIAL_MS
        .saturating_mul(2u32.saturating_pow(attempt.min(5)))
        .min(MAX_MS)
}

/// Type alias for WebSocket sender to reduce type complexity
type WsSender = Rc<RefCell<Option<futures_util::stream::SplitSink<WebSocket, Message>>>>;

/// Extract session display parts from session_name and working_directory
/// Input session_name format: "hostname-YYYYMMDD-HHMMSS"
/// Returns: (project_name, hostname) - project may be None if no working_directory
fn get_session_display_parts(session: &SessionInfo) -> (Option<String>, String) {
    // Extract hostname from session_name (everything before the date suffix)
    // Format: hostname-YYYYMMDD-HHMMSS
    let hostname = session
        .session_name
        .rsplit('-')
        .skip(2) // Skip HHMMSS and YYYYMMDD
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("-");

    let hostname = if hostname.is_empty() {
        session.session_name.clone()
    } else {
        hostname
    };

    // Extract project folder from working_directory
    let project = session
        .working_directory
        .as_ref()
        .and_then(|dir| dir.split('/').next_back())
        .map(|s| s.to_string());

    (project, hostname)
}

/// Message data from the API
#[derive(Clone, PartialEq, serde::Deserialize)]
struct MessageData {
    #[allow(dead_code)]
    role: String,
    content: String,
    /// ISO 8601 timestamp when message was created
    created_at: String,
}

#[derive(Clone, PartialEq, serde::Deserialize)]
struct MessagesResponse {
    messages: Vec<MessageData>,
}

// =============================================================================
// Dashboard Page - Focus Flow Design
// =============================================================================

use std::collections::HashMap;

#[function_component(DashboardPage)]
pub fn dashboard_page() -> Html {
    let sessions = use_state(Vec::<SessionInfo>::new);
    let loading = use_state(|| true);
    let refresh_trigger = use_state(|| 0u32);
    let show_new_session = use_state(|| false);
    let focused_index = use_state(|| 0usize);
    let awaiting_sessions = use_state(HashSet::<Uuid>::new);
    let paused_sessions = use_state(HashSet::<Uuid>::new);
    let session_costs = use_state(HashMap::<Uuid, f64>::new);
    let connected_sessions = use_state(HashSet::<Uuid>::new);
    let pending_delete = use_state(|| None::<Uuid>);
    let pending_leave = use_state(|| None::<Uuid>);
    let nav_mode = use_state(|| false);
    let total_user_spend = use_state(|| 0.0f64);
    let is_admin = use_state(|| false);
    let voice_enabled = use_state(|| false);
    let share_session_id = use_state(|| None::<Uuid>);

    // Fetch current user info (to check admin status and voice_enabled)
    {
        let is_admin = is_admin.clone();
        let voice_enabled = voice_enabled.clone();
        use_effect_with((), move |_| {
            spawn_local(async move {
                let api_endpoint = utils::api_url("/api/auth/me");
                if let Ok(response) = Request::get(&api_endpoint).send().await {
                    if let Ok(data) = response.json::<serde_json::Value>().await {
                        if let Some(admin) = data.get("is_admin").and_then(|v| v.as_bool()) {
                            is_admin.set(admin);
                        }
                        if let Some(voice) = data.get("voice_enabled").and_then(|v| v.as_bool()) {
                            voice_enabled.set(voice);
                        }
                    }
                }
            });
            || ()
        });
    }

    // Fetch sessions
    let fetch_sessions = {
        let sessions = sessions.clone();
        let loading = loading.clone();
        let focused_index = focused_index.clone();

        Callback::from(move |set_loading: bool| {
            let sessions = sessions.clone();
            let loading = loading.clone();
            let focused_index = focused_index.clone();

            spawn_local(async move {
                let api_endpoint = utils::api_url("/api/sessions");
                match Request::get(&api_endpoint).send().await {
                    Ok(response) => {
                        if response.status() == 401 {
                            // Session invalid - redirect to logout
                            if let Some(window) = web_sys::window() {
                                let _ = window.location().set_href("/api/auth/logout");
                            }
                            return;
                        }
                        if let Ok(data) = response.json::<serde_json::Value>().await {
                            if let Some(session_list) = data.get("sessions") {
                                if let Ok(parsed) =
                                    serde_json::from_value::<Vec<SessionInfo>>(session_list.clone())
                                {
                                    // Ensure focused_index is within bounds
                                    if *focused_index >= parsed.len() && !parsed.is_empty() {
                                        focused_index.set(parsed.len() - 1);
                                    }
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

    // Initial fetch
    {
        let fetch_sessions = fetch_sessions.clone();
        use_effect_with((), move |_| {
            fetch_sessions.emit(true);
            || ()
        });
    }

    // Polling every 5 seconds
    {
        let fetch_sessions = fetch_sessions.clone();
        use_effect_with((), move |_| {
            let interval = gloo::timers::callback::Interval::new(5_000, move || {
                fetch_sessions.emit(false);
            });
            move || drop(interval)
        });
    }

    // Refresh trigger
    {
        let fetch_sessions = fetch_sessions.clone();
        let refresh = *refresh_trigger;
        use_effect_with(refresh, move |_| {
            if refresh > 0 {
                fetch_sessions.emit(false);
            }
            || ()
        });
    }

    // WebSocket connection for user-level spend updates (with reconnection)
    {
        let total_user_spend = total_user_spend.clone();
        let session_costs = session_costs.clone();
        use_effect_with((), move |_| {
            let total_user_spend = total_user_spend.clone();
            let session_costs = session_costs.clone();
            spawn_local(async move {
                let mut attempt: u32 = 0;
                const MAX_ATTEMPTS: u32 = 10;

                loop {
                    let ws_endpoint = utils::ws_url("/ws/client");
                    match WebSocket::open(&ws_endpoint) {
                        Ok(ws) => {
                            attempt = 0; // Reset on successful connection
                            let (_sender, mut receiver) = ws.split();

                            while let Some(msg) = receiver.next().await {
                                match msg {
                                    Ok(Message::Text(text)) => {
                                        if let Ok(ProxyMessage::UserSpendUpdate {
                                            total_spend_usd,
                                            session_costs: costs,
                                        }) = serde_json::from_str::<ProxyMessage>(&text)
                                        {
                                            total_user_spend.set(total_spend_usd);
                                            let mut map = (*session_costs).clone();
                                            for SessionCost {
                                                session_id,
                                                total_cost_usd,
                                            } in costs
                                            {
                                                map.insert(session_id, total_cost_usd);
                                            }
                                            session_costs.set(map);
                                        }
                                    }
                                    Err(e) => {
                                        log::error!("Spend WebSocket error: {:?}", e);
                                        break;
                                    }
                                    _ => {}
                                }
                            }
                        }
                        Err(e) => {
                            log::error!("Failed to connect spend WebSocket: {:?}", e);
                        }
                    }

                    // Reconnection with exponential backoff
                    if attempt >= MAX_ATTEMPTS {
                        log::error!("Spend WebSocket: max reconnection attempts reached");
                        break;
                    }
                    let delay_ms = calculate_backoff(attempt);
                    attempt += 1;
                    log::info!(
                        "Spend WebSocket reconnecting in {}ms (attempt {})",
                        delay_ms,
                        attempt
                    );
                    gloo::timers::future::TimeoutFuture::new(delay_ms).await;
                }
            });
            || ()
        });
    }

    // Show delete confirmation modal
    let on_delete = {
        let pending_delete = pending_delete.clone();
        Callback::from(move |session_id: Uuid| {
            pending_delete.set(Some(session_id));
        })
    };

    // Cancel delete
    let on_cancel_delete = {
        let pending_delete = pending_delete.clone();
        Callback::from(move |_| {
            pending_delete.set(None);
        })
    };

    // Confirm delete
    let on_confirm_delete = {
        let pending_delete = pending_delete.clone();
        let refresh_trigger = refresh_trigger.clone();
        Callback::from(move |_| {
            if let Some(session_id) = *pending_delete {
                let refresh_trigger = refresh_trigger.clone();
                let pending_delete = pending_delete.clone();
                spawn_local(async move {
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
                    pending_delete.set(None);
                });
            }
        })
    };

    // Show leave confirmation modal (for non-owners)
    let on_leave = {
        let pending_leave = pending_leave.clone();
        Callback::from(move |session_id: Uuid| {
            pending_leave.set(Some(session_id));
        })
    };

    // Cancel leave
    let on_cancel_leave = {
        let pending_leave = pending_leave.clone();
        Callback::from(move |_| {
            pending_leave.set(None);
        })
    };

    // Confirm leave - calls remove member endpoint with own user_id
    let on_confirm_leave = {
        let pending_leave = pending_leave.clone();
        let refresh_trigger = refresh_trigger.clone();
        Callback::from(move |_| {
            if let Some(session_id) = *pending_leave {
                let refresh_trigger = refresh_trigger.clone();
                let pending_leave = pending_leave.clone();
                spawn_local(async move {
                    // Get current user ID from /api/auth/me
                    let me_endpoint = utils::api_url("/api/auth/me");
                    let user_id = match Request::get(&me_endpoint).send().await {
                        Ok(response) => {
                            if let Ok(data) = response.json::<serde_json::Value>().await {
                                data.get("id")
                                    .and_then(|v| v.as_str())
                                    .map(|s| s.to_string())
                            } else {
                                None
                            }
                        }
                        Err(_) => None,
                    };

                    if let Some(user_id) = user_id {
                        let api_endpoint = utils::api_url(&format!(
                            "/api/sessions/{}/members/{}",
                            session_id, user_id
                        ));
                        match Request::delete(&api_endpoint).send().await {
                            Ok(response) if response.status() == 204 => {
                                refresh_trigger.set(*refresh_trigger + 1);
                            }
                            Ok(response) => {
                                log::error!(
                                    "Failed to leave session: status {}",
                                    response.status()
                                );
                            }
                            Err(e) => {
                                log::error!("Failed to leave session: {:?}", e);
                            }
                        }
                    } else {
                        log::error!("Failed to get current user ID for leave");
                    }
                    pending_leave.set(None);
                });
            }
        })
    };

    let toggle_new_session = {
        let show_new_session = show_new_session.clone();
        Callback::from(move |_| {
            show_new_session.set(!*show_new_session);
        })
    };

    // Get all sessions for the rail, sorted by status (active first), then repo name, then hostname
    // NOTE: This must be computed BEFORE navigation callbacks so they use the same sorted order
    let active_sessions: Vec<_> = {
        let mut sorted: Vec<_> = sessions.iter().cloned().collect();
        sorted.sort_by(|a, b| {
            // Active sessions come before disconnected/inactive
            let a_is_active = a.status.as_str() == "active";
            let b_is_active = b.status.as_str() == "active";
            match (a_is_active, b_is_active) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => {
                    // Same status - sort by repo name then hostname
                    let (project_a, hostname_a) = get_session_display_parts(a);
                    let (project_b, hostname_b) = get_session_display_parts(b);
                    let repo_a = project_a.as_deref().unwrap_or("");
                    let repo_b = project_b.as_deref().unwrap_or("");
                    match repo_a.to_lowercase().cmp(&repo_b.to_lowercase()) {
                        std::cmp::Ordering::Equal => {
                            hostname_a.to_lowercase().cmp(&hostname_b.to_lowercase())
                        }
                        other => other,
                    }
                }
            }
        });
        sorted
    };

    // Navigation callbacks
    let on_select_session = {
        let focused_index = focused_index.clone();
        Callback::from(move |index: usize| {
            focused_index.set(index);
        })
    };

    let on_navigate = {
        let focused_index = focused_index.clone();
        let active_sessions = active_sessions.clone();
        let paused_sessions = paused_sessions.clone();
        Callback::from(move |delta: i32| {
            let len = active_sessions.len();
            if len == 0 {
                return;
            }

            // Count non-paused sessions
            let non_paused_count = active_sessions
                .iter()
                .filter(|s| !paused_sessions.contains(&s.id))
                .count();

            // If all sessions are paused, allow normal navigation
            if non_paused_count == 0 {
                let current = *focused_index as i32;
                let new_index = (current + delta).rem_euclid(len as i32) as usize;
                focused_index.set(new_index);
                return;
            }

            // Skip paused sessions when navigating
            let current = *focused_index;
            let mut new_index = current;
            let step = if delta > 0 { 1 } else { len - 1 };

            for _ in 0..len {
                new_index = (new_index + step) % len;
                if let Some(session) = active_sessions.get(new_index) {
                    if !paused_sessions.contains(&session.id) {
                        focused_index.set(new_index);
                        return;
                    }
                }
            }
        })
    };

    let on_next_active = {
        let focused_index = focused_index.clone();
        let active_sessions = active_sessions.clone();
        let paused_sessions = paused_sessions.clone();
        Callback::from(move |_| {
            let len = active_sessions.len();
            if len == 0 {
                return;
            }
            let current = *focused_index;
            // Find next non-paused session after current (wraps around)
            for i in 1..=len {
                let idx = (current + i) % len;
                if let Some(session) = active_sessions.get(idx) {
                    if !paused_sessions.contains(&session.id) {
                        focused_index.set(idx);
                        return;
                    }
                }
            }
            // If all sessions are paused, stay on current
        })
    };

    let on_awaiting_change = {
        let awaiting_sessions = awaiting_sessions.clone();
        Callback::from(move |(session_id, is_awaiting): (Uuid, bool)| {
            let mut set = (*awaiting_sessions).clone();
            if is_awaiting {
                set.insert(session_id);
            } else {
                set.remove(&session_id);
            }
            awaiting_sessions.set(set);
        })
    };

    let on_cost_change = {
        let session_costs = session_costs.clone();
        Callback::from(move |(session_id, cost): (Uuid, f64)| {
            let mut map = (*session_costs).clone();
            map.insert(session_id, cost);
            session_costs.set(map);
        })
    };

    let on_connected_change = {
        let connected_sessions = connected_sessions.clone();
        Callback::from(move |(session_id, connected): (Uuid, bool)| {
            let mut set = (*connected_sessions).clone();
            if connected {
                set.insert(session_id);
            } else {
                set.remove(&session_id);
            }
            connected_sessions.set(set);
        })
    };

    let on_toggle_pause = {
        let paused_sessions = paused_sessions.clone();
        Callback::from(move |session_id: Uuid| {
            let mut set = (*paused_sessions).clone();
            if set.contains(&session_id) {
                set.remove(&session_id);
            } else {
                set.insert(session_id);
            }
            paused_sessions.set(set);
        })
    };

    // Open share dialog for a session
    let on_share = {
        let share_session_id = share_session_id.clone();
        Callback::from(move |session_id: Uuid| {
            share_session_id.set(Some(session_id));
        })
    };

    // Close share dialog
    let on_close_share = {
        let share_session_id = share_session_id.clone();
        Callback::from(move |_| {
            share_session_id.set(None);
        })
    };

    // Update awaiting state after sending message (no auto-advance)
    let on_message_sent = {
        let awaiting_sessions = awaiting_sessions.clone();
        Callback::from(move |current_session_id: Uuid| {
            // Remove current from awaiting since we just sent a message
            let mut set = (*awaiting_sessions).clone();
            set.remove(&current_session_id);
            awaiting_sessions.set(set);
        })
    };

    // Update git branch when it changes
    let on_branch_change = {
        let sessions = sessions.clone();
        Callback::from(move |(session_id, branch): (Uuid, Option<String>)| {
            let mut updated = (*sessions).clone();
            if let Some(session) = updated.iter_mut().find(|s| s.id == session_id) {
                session.git_branch = branch;
            }
            sessions.set(updated);
        })
    };

    let waiting_count = awaiting_sessions.len();

    // Count disconnected sessions for the reconnection banner
    let disconnected_count = active_sessions
        .iter()
        .filter(|s| !connected_sessions.contains(&s.id))
        .count();

    // Two-mode keyboard handling:
    // - Edit Mode (default): typing works, Escape -> Nav Mode, Shift+Tab -> next active (skips paused)
    // - Nav Mode: arrow keys navigate, Enter/Escape -> Edit Mode, numbers select directly
    let on_keydown = {
        let on_navigate = on_navigate.clone();
        let on_next_active = on_next_active.clone();
        let on_toggle_pause = on_toggle_pause.clone();
        let on_select_session = on_select_session.clone();
        let focused_index = focused_index.clone();
        let nav_mode = nav_mode.clone();
        let active_sessions = active_sessions.clone();
        Callback::from(move |e: KeyboardEvent| {
            let in_nav_mode = *nav_mode;

            // Shift+Tab always jumps to next active session, skipping paused (works in both modes)
            if e.shift_key() && e.key() == "Tab" {
                e.prevent_default();
                on_next_active.emit(());
                return;
            }

            // Ctrl+Shift+P toggles pause (works in both modes)
            if e.ctrl_key() && e.shift_key() && (e.key() == "P" || e.key() == "p") {
                e.prevent_default();
                if let Some(session) = active_sessions.get(*focused_index) {
                    on_toggle_pause.emit(session.id);
                }
                return;
            }

            if in_nav_mode {
                // Navigation Mode
                match e.key().as_str() {
                    "Escape" | "i" => {
                        e.prevent_default();
                        nav_mode.set(false);
                    }
                    "ArrowUp" | "ArrowLeft" | "k" | "h" => {
                        e.prevent_default();
                        on_navigate.emit(-1);
                    }
                    "ArrowDown" | "ArrowRight" | "j" | "l" => {
                        e.prevent_default();
                        on_navigate.emit(1);
                    }
                    "Enter" => {
                        e.prevent_default();
                        nav_mode.set(false);
                    }
                    "w" => {
                        e.prevent_default();
                        on_next_active.emit(());
                    }
                    "x" => {
                        // Close session - could trigger delete confirmation
                        // For now, just a placeholder
                    }
                    key => {
                        // Number keys 1-9 for direct selection
                        if let Ok(num) = key.parse::<usize>() {
                            if (1..=9).contains(&num) && num <= active_sessions.len() {
                                e.prevent_default();
                                on_select_session.emit(num - 1);
                                nav_mode.set(false);
                            }
                        }
                    }
                }
            } else {
                // Edit Mode
                match e.key().as_str() {
                    "Escape" => {
                        e.prevent_default();
                        nav_mode.set(true);
                    }
                    _ => {
                        // Let all other keys through to the input
                    }
                }
            }
        })
    };

    html! {
        <div class="focus-flow-container" onkeydown={on_keydown} tabindex="0">
            // Header with new session button
            <header class="focus-flow-header">
                <h1>{ "Claude Code Sessions" }</h1>
                <div class="header-actions">
                    {
                        if *total_user_spend > 0.0 {
                            html! {
                                <span class="total-spend-badge" title="Total spend across all sessions">
                                    { format!("${:.2}", *total_user_spend) }
                                </span>
                            }
                        } else {
                            html! {}
                        }
                    }
                    {
                        if waiting_count > 0 {
                            html! {
                                <span class="waiting-badge">
                                    { format!("{} waiting", waiting_count) }
                                </span>
                            }
                        } else {
                            html! {}
                        }
                    }
                    <button
                        class={classes!("new-session-button", if *show_new_session { "active" } else { "" })}
                        onclick={toggle_new_session.clone()}
                        title={if *show_new_session { "Close" } else { "Connect a new Claude proxy session" }}
                    >
                        { if *show_new_session { "Close" } else { "+ New Session" } }
                    </button>
                    {
                        if *is_admin {
                            html! {
                                <Link<Route> to={Route::Admin} classes="admin-link">
                                    { "Admin" }
                                </Link<Route>>
                            }
                        } else {
                            html! {}
                        }
                    }
                    <Link<Route> to={Route::Settings} classes="settings-button">
                        { "Settings" }
                    </Link<Route>>
                    <a href="/api/auth/logout" class="logout-button">
                        { "Logout" }
                    </a>
                </div>
            </header>

            // New session modal
            if *show_new_session {
                <div class="modal-overlay" onclick={toggle_new_session.clone()}>
                    <div class="modal-content" onclick={Callback::from(|e: MouseEvent| e.stop_propagation())}>
                        <ProxyTokenSetup />
                    </div>
                </div>
            }

            // Reconnection banner - show when any session is disconnected
            if disconnected_count > 0 && !*loading {
                <div class="reconnection-banner">
                    <span class="reconnection-spinner">{ "↻" }</span>
                    <span class="reconnection-text">
                        { if disconnected_count == 1 {
                            "Reconnecting...".to_string()
                        } else {
                            format!("{} sessions reconnecting...", disconnected_count)
                        }}
                    </span>
                </div>
            }

            if *loading {
                <div class="loading">
                    <div class="spinner"></div>
                    <p>{ "Loading sessions..." }</p>
                </div>
            } else if active_sessions.is_empty() {
                <div class="empty-state">
                    <h2>{ "No Active Sessions" }</h2>
                    <p>{ "Click \"+ New Session\" to connect a Claude proxy from your machine." }</p>
                </div>
            } else {
                <>
                    // Session Rail (horizontal carousel)
                    <SessionRail
                        sessions={active_sessions.clone()}
                        focused_index={*focused_index}
                        awaiting_sessions={(*awaiting_sessions).clone()}
                        paused_sessions={(*paused_sessions).clone()}
                        session_costs={(*session_costs).clone()}
                        connected_sessions={(*connected_sessions).clone()}
                        nav_mode={*nav_mode}
                        on_select={on_select_session.clone()}
                        on_delete={on_delete.clone()}
                        on_leave={on_leave.clone()}
                        on_toggle_pause={on_toggle_pause.clone()}
                        on_share={on_share.clone()}
                    />

                    // Render ALL session views - keep them alive for instant switching
                    // Only the focused one is visible, others are hidden via CSS
                    <div class={classes!("session-views-container", if *nav_mode { Some("nav-mode") } else { None })}>
                        {
                            active_sessions.iter().enumerate().map(|(index, session)| {
                                let is_focused = index == *focused_index;
                                html! {
                                    <div
                                        key={session.id.to_string()}
                                        class={classes!("session-view-wrapper", if is_focused { "focused" } else { "hidden" })}
                                    >
                                        <SessionView
                                            session={session.clone()}
                                            focused={is_focused}
                                            on_awaiting_change={on_awaiting_change.clone()}
                                            on_cost_change={on_cost_change.clone()}
                                            on_connected_change={on_connected_change.clone()}
                                            on_message_sent={on_message_sent.clone()}
                                            on_branch_change={on_branch_change.clone()}
                                            voice_enabled={*voice_enabled}
                                        />
                                    </div>
                                }
                            }).collect::<Html>()
                        }
                    </div>

                    // Keyboard hints - context-sensitive based on mode
                    <div class={classes!("keyboard-hints", if *nav_mode { Some("nav-mode") } else { None })}>
                        {
                            if *nav_mode {
                                html! {
                                    <>
                                        <span class="mode-indicator">{ "NAV" }</span>
                                        <span>{ "↑↓ or jk = navigate" }</span>
                                        <span>{ "1-9 = select" }</span>
                                        <span>{ "w = next waiting" }</span>
                                        <span>{ "Enter/Esc = edit mode" }</span>
                                    </>
                                }
                            } else {
                                html! {
                                    <>
                                        <span>{ "Esc = nav mode" }</span>
                                        <span>{ "Shift+Tab = next (skip paused)" }</span>
                                        <span>{ "Ctrl+Shift+P = pause" }</span>
                                        if *voice_enabled {
                                            <span>{ "Ctrl+M = voice" }</span>
                                        }
                                        <span>{ "Enter = send" }</span>
                                    </>
                                }
                            }
                        }
                    </div>
                </>
            }

            // Delete confirmation modal
            {
                if let Some(session_id) = *pending_delete {
                    let session_name = sessions.iter()
                        .find(|s| s.id == session_id)
                        .map(|s| {
                            let (project, hostname) = get_session_display_parts(s);
                            project.unwrap_or(hostname)
                        })
                        .unwrap_or_else(|| "this session".to_string());

                    html! {
                        <div class="modal-overlay" onclick={on_cancel_delete.clone()}>
                            <div class="modal-content delete-confirm" onclick={Callback::from(|e: MouseEvent| e.stop_propagation())}>
                                <h2>{ "Delete Session?" }</h2>
                                <p>{ format!("Are you sure you want to delete \"{}\"?", session_name) }</p>
                                <p class="modal-warning">{ "This action cannot be undone." }</p>
                                <div class="modal-actions">
                                    <button class="modal-cancel" onclick={on_cancel_delete.clone()}>{ "Cancel" }</button>
                                    <button class="modal-confirm" onclick={on_confirm_delete.clone()}>{ "Delete" }</button>
                                </div>
                            </div>
                        </div>
                    }
                } else {
                    html! {}
                }
            }

            // Leave confirmation modal (for non-owners)
            {
                if let Some(session_id) = *pending_leave {
                    let session_name = sessions.iter()
                        .find(|s| s.id == session_id)
                        .map(|s| {
                            let (project, hostname) = get_session_display_parts(s);
                            project.unwrap_or(hostname)
                        })
                        .unwrap_or_else(|| "this session".to_string());

                    html! {
                        <div class="modal-overlay" onclick={on_cancel_leave.clone()}>
                            <div class="modal-content delete-confirm" onclick={Callback::from(|e: MouseEvent| e.stop_propagation())}>
                                <h2>{ "Leave Session?" }</h2>
                                <p>{ format!("Are you sure you want to leave \"{}\"?", session_name) }</p>
                                <p class="modal-warning">{ "You will need to be re-invited to access this session again." }</p>
                                <div class="modal-actions">
                                    <button class="modal-cancel" onclick={on_cancel_leave.clone()}>{ "Cancel" }</button>
                                    <button class="modal-confirm" onclick={on_confirm_leave.clone()}>{ "Leave" }</button>
                                </div>
                            </div>
                        </div>
                    }
                } else {
                    html! {}
                }
            }

            // Share dialog
            {
                if let Some(session_id) = *share_session_id {
                    html! {
                        <ShareDialog
                            session_id={session_id}
                            on_close={on_close_share.clone()}
                        />
                    }
                } else {
                    html! {}
                }
            }
        </div>
    }
}

// =============================================================================
// Session Rail - Horizontal Carousel of Session Pills
// =============================================================================

#[derive(Properties, PartialEq)]
struct SessionRailProps {
    sessions: Vec<SessionInfo>,
    focused_index: usize,
    awaiting_sessions: HashSet<Uuid>,
    paused_sessions: HashSet<Uuid>,
    session_costs: HashMap<Uuid, f64>,
    connected_sessions: HashSet<Uuid>,
    nav_mode: bool,
    on_select: Callback<usize>,
    on_delete: Callback<Uuid>,
    on_leave: Callback<Uuid>,
    on_toggle_pause: Callback<Uuid>,
    on_share: Callback<Uuid>,
}

#[function_component(SessionRail)]
fn session_rail(props: &SessionRailProps) -> Html {
    let rail_ref = use_node_ref();

    // Scroll focused session into view
    {
        let rail_ref = rail_ref.clone();
        let focused_index = props.focused_index;
        use_effect_with(focused_index, move |_| {
            if let Some(rail) = rail_ref.cast::<Element>() {
                if let Some(child) = rail.children().item(focused_index as u32) {
                    // Use simple scroll into view - smooth scrolling via CSS
                    child.scroll_into_view();
                }
            }
            || ()
        });
    }

    html! {
        <div class="session-rail" ref={rail_ref}>
            {
                props.sessions.iter().enumerate().map(|(index, session)| {
                    let is_focused = index == props.focused_index;
                    let is_awaiting = props.awaiting_sessions.contains(&session.id);
                    let is_paused = props.paused_sessions.contains(&session.id);
                    let is_connected = props.connected_sessions.contains(&session.id);
                    let cost = props.session_costs.get(&session.id).copied().unwrap_or(0.0);

                    let on_click = {
                        let on_select = props.on_select.clone();
                        Callback::from(move |_| on_select.emit(index))
                    };

                    let on_delete = {
                        let on_delete = props.on_delete.clone();
                        let session_id = session.id;
                        Callback::from(move |e: MouseEvent| {
                            e.stop_propagation();
                            on_delete.emit(session_id);
                        })
                    };

                    let on_pause = {
                        let on_toggle_pause = props.on_toggle_pause.clone();
                        let session_id = session.id;
                        Callback::from(move |e: MouseEvent| {
                            e.stop_propagation();
                            on_toggle_pause.emit(session_id);
                        })
                    };

                    let on_share = props.on_share.clone();

                    let on_leave = {
                        let on_leave = props.on_leave.clone();
                        let session_id = session.id;
                        Callback::from(move |e: MouseEvent| {
                            e.stop_propagation();
                            on_leave.emit(session_id);
                        })
                    };

                    let in_nav_mode = props.nav_mode;
                    let is_status_disconnected = session.status.as_str() != "active";
                    let pill_class = classes!(
                        "session-pill",
                        if is_focused { Some("focused") } else { None },
                        if is_awaiting { Some("awaiting") } else { None },
                        if is_paused { Some("paused") } else { None },
                        if in_nav_mode { Some("nav-mode") } else { None },
                        if is_status_disconnected { Some("status-disconnected") } else { None },
                    );

                    let (project, hostname) = get_session_display_parts(session);
                    let project_display = project.unwrap_or_else(|| hostname.clone());
                    let show_hostname = session.working_directory.is_some();

                    let connection_class = if is_connected { "pill-status connected" } else { "pill-status disconnected" };

                    // Show number annotation only in nav mode (1-9)
                    let number_annotation = if in_nav_mode && index < 9 {
                        Some(format!("{}", index + 1))
                    } else {
                        None
                    };

                    html! {
                        <div class={pill_class} onclick={on_click}>
                            {
                                if let Some(num) = &number_annotation {
                                    html! { <span class="pill-number">{ num }</span> }
                                } else {
                                    html! {}
                                }
                            }
                            <span class={connection_class}>
                                { if is_connected { "●" } else { "○" } }
                            </span>
                            <span class="pill-name" title={session.session_name.clone()}>
                                <span class="pill-project">{ project_display }</span>
                                {
                                    if show_hostname {
                                        html! { <span class="pill-hostname">{ hostname }</span> }
                                    } else {
                                        html! {}
                                    }
                                }
                                {
                                    if let Some(ref branch) = session.git_branch {
                                        html! { <span class="pill-branch">{ branch }</span> }
                                    } else {
                                        html! { <span class="pill-no-branch">{ "⚠️ No Git Branch ⚠️" }</span> }
                                    }
                                }
                            </span>
                            {
                                if cost > 0.0 {
                                    html! { <span class="pill-cost">{ format!("${:.2}", cost) }</span> }
                                } else {
                                    html! {}
                                }
                            }
                            {
                                if is_paused {
                                    html! { <span class="pill-paused-badge">{ "ᴾ" }</span> }
                                } else {
                                    html! {}
                                }
                            }
                            // Show role badge for non-owners
                            {
                                if session.my_role != "owner" {
                                    let role_class = format!("pill-role-badge role-{}", session.my_role);
                                    html! { <span class={role_class}>{ &session.my_role }</span> }
                                } else {
                                    html! {}
                                }
                            }
                            <button
                                class={classes!("pill-pause", if is_paused { Some("active") } else { None })}
                                onclick={on_pause}
                                title={if is_paused { "Unpause session" } else { "Pause session (skip in rotation)" }}
                            >
                                { if is_paused { "▶" } else { "⏸" } }
                            </button>
                            // Share button for owners
                            {
                                if session.my_role == "owner" {
                                    let session_id = session.id;
                                    let on_share_click = on_share.reform(move |e: MouseEvent| {
                                        e.stop_propagation();
                                        session_id
                                    });
                                    html! {
                                        <button class="pill-share" onclick={on_share_click} title="Share session">{ "👤+" }</button>
                                    }
                                } else {
                                    html! {}
                                }
                            }
                            // Delete for owners, Leave for non-owners
                            {
                                if session.my_role == "owner" {
                                    html! {
                                        <button class="pill-delete" onclick={on_delete} title="Delete session">{ "×" }</button>
                                    }
                                } else {
                                    html! {
                                        <button class="pill-leave" onclick={on_leave} title="Leave session">{ "↩" }</button>
                                    }
                                }
                            }
                        </div>
                    }
                }).collect::<Html>()
            }
        </div>
    }
}

// =============================================================================
// Session View - Main Terminal View (inline, not modal)
// =============================================================================

#[derive(Properties, PartialEq)]
pub struct SessionViewProps {
    pub session: SessionInfo,
    pub focused: bool,
    pub on_awaiting_change: Callback<(Uuid, bool)>,
    pub on_cost_change: Callback<(Uuid, f64)>,
    pub on_connected_change: Callback<(Uuid, bool)>,
    pub on_message_sent: Callback<Uuid>,
    pub on_branch_change: Callback<(Uuid, Option<String>)>,
    /// Whether voice input is enabled for this user
    #[prop_or(false)]
    pub voice_enabled: bool,
}

/// Pending permission request
#[derive(Clone, Debug)]
pub struct PendingPermission {
    pub request_id: String,
    pub tool_name: String,
    pub input: serde_json::Value,
    pub permission_suggestions: Vec<serde_json::Value>,
}

/// Parsed AskUserQuestion option
#[derive(Clone, Debug, serde::Deserialize)]
pub struct AskUserOption {
    pub label: String,
    #[serde(default)]
    pub description: String,
}

/// Parsed AskUserQuestion question
#[derive(Clone, Debug, serde::Deserialize)]
pub struct AskUserQuestion {
    pub question: String,
    #[serde(default)]
    pub header: String,
    #[serde(default)]
    pub options: Vec<AskUserOption>,
    #[serde(default, rename = "multiSelect")]
    pub multi_select: bool,
}

/// Parsed AskUserQuestion input
#[derive(Clone, Debug, serde::Deserialize)]
pub struct AskUserQuestionInput {
    pub questions: Vec<AskUserQuestion>,
}

/// Try to parse AskUserQuestion input from permission input
fn parse_ask_user_question(input: &serde_json::Value) -> Option<AskUserQuestionInput> {
    serde_json::from_value(input.clone()).ok()
}

pub enum SessionViewMsg {
    SendInput,
    UpdateInput(String),
    /// Bulk load historical messages (no per-message scroll)
    /// Contains (messages, last_message_timestamp) for replay_after tracking
    LoadHistory(Vec<String>, Option<String>),
    /// Single new message from WebSocket (triggers scroll)
    ReceivedOutput(String),
    WebSocketConnected(WsSender),
    WebSocketError(String),
    /// Attempt to reconnect WebSocket after disconnect
    AttemptReconnect,
    CheckAwaiting,
    ClearCostFlash,
    /// Permission request received
    PermissionRequest(PendingPermission),
    /// User approved permission (one-time)
    ApprovePermission,
    /// User approved and wants to remember for similar future requests
    ApprovePermissionAndRemember,
    /// User denied permission
    DenyPermission,
    /// Navigate permission options
    PermissionSelectUp,
    PermissionSelectDown,
    /// Git branch changed
    BranchChanged(Option<String>),
    /// Confirm current permission selection
    PermissionConfirm,
    /// Select and confirm permission option by index (for click/touch)
    PermissionSelectAndConfirm(usize),
    /// Navigate command history up (older)
    HistoryUp,
    /// Navigate command history down (newer)
    HistoryDown,
    /// Voice recording state changed
    VoiceRecordingChanged(bool),
    /// Voice transcription received (final)
    VoiceTranscription(String),
    /// Interim (partial) voice transcription received
    VoiceInterimTranscription(String),
    /// Voice error occurred
    VoiceError(String),
    /// Toggle voice recording (for keyboard shortcut)
    ToggleVoice,
    /// Answer an AskUserQuestion with selected option(s)
    AnswerQuestion(String),
    /// Toggle multi-select option for AskUserQuestion
    ToggleQuestionOption(usize),
}

pub struct SessionView {
    messages: Vec<String>,
    input_value: String,
    ws_connected: bool,
    ws_sender: Option<WsSender>,
    messages_ref: NodeRef,
    input_ref: NodeRef,
    permission_ref: NodeRef,
    should_autoscroll: Rc<RefCell<bool>>,
    #[allow(dead_code)]
    scroll_listener: Option<Closure<dyn Fn()>>,
    was_focused: bool,
    total_cost: f64,
    cost_flash: bool,
    pending_permission: Option<PendingPermission>,
    permission_selected: usize,
    /// Current reconnection attempt number (0 = not reconnecting)
    reconnect_attempt: u32,
    /// Handle to cancel pending reconnect timer
    #[allow(dead_code)]
    reconnect_timer: Option<Timeout>,
    /// Command history (most recent last)
    command_history: Vec<String>,
    /// Current position in history (None = new input, Some(i) = viewing history[i])
    history_position: Option<usize>,
    /// Draft input preserved when navigating history
    draft_input: String,
    /// Whether voice recording is active
    is_recording: bool,
    /// Interim (partial) voice transcription being displayed
    interim_transcription: Option<String>,
    /// Timestamp of the last received message (ISO 8601 format)
    /// Used for replay_after on reconnection to avoid duplicate messages
    last_message_timestamp: Option<String>,
    /// NodeRef to voice button for keyboard shortcut
    voice_button_ref: NodeRef,
    /// Selected options for multi-select AskUserQuestion (indices)
    multi_select_options: HashSet<usize>,
}

impl Component for SessionView {
    type Message = SessionViewMsg;
    type Properties = SessionViewProps;

    fn create(ctx: &Context<Self>) -> Self {
        let link = ctx.link().clone();
        let session_id = ctx.props().session.id;
        let on_awaiting_change = ctx.props().on_awaiting_change.clone();

        // Fetch existing messages via REST, then connect WebSocket with replay_after
        // This ensures we don't get duplicate messages
        spawn_local(async move {
            // Step 1: Fetch existing messages via REST API
            let mut last_message_time: Option<String> = None;
            let api_endpoint = utils::api_url(&format!("/api/sessions/{}/messages", session_id));
            if let Ok(response) = Request::get(&api_endpoint).send().await {
                if let Ok(data) = response.json::<MessagesResponse>().await {
                    // Check if awaiting input
                    let is_awaiting = data.messages.last().is_some_and(|msg| {
                        serde_json::from_str::<serde_json::Value>(&msg.content)
                            .ok()
                            .and_then(|p| {
                                p.get("type")
                                    .and_then(|t| t.as_str())
                                    .map(|t| t == "result")
                            })
                            .unwrap_or(false)
                    });
                    on_awaiting_change.emit((session_id, is_awaiting));

                    // Get last message timestamp for WebSocket replay_after
                    last_message_time = data.messages.last().map(|m| m.created_at.clone());

                    // Bulk load all historical messages at once (with timestamp for reconnection)
                    let messages: Vec<String> =
                        data.messages.into_iter().map(|m| m.content).collect();
                    link.send_message(SessionViewMsg::LoadHistory(
                        messages,
                        last_message_time.clone(),
                    ));
                }
            }

            // Step 2: Connect WebSocket with replay_after set to last message time
            // This prevents duplicate messages from being sent
            let ws_endpoint = utils::ws_url("/ws/client");
            match WebSocket::open(&ws_endpoint) {
                Ok(ws) => {
                    let (mut sender, mut receiver) = ws.split();

                    let register_msg = ProxyMessage::Register {
                        session_id,
                        session_name: session_id.to_string(),
                        auth_token: None,
                        working_directory: String::new(),
                        resuming: false,
                        git_branch: None,
                        replay_after: last_message_time,
                    };

                    if let Ok(json) = serde_json::to_string(&register_msg) {
                        if sender.send(Message::Text(json)).await.is_err() {
                            link.send_message(SessionViewMsg::WebSocketError(
                                "Failed to send registration".to_string(),
                            ));
                            return;
                        }
                    }

                    let sender = Rc::new(RefCell::new(Some(sender)));
                    link.send_message(SessionViewMsg::WebSocketConnected(sender));

                    while let Some(msg) = receiver.next().await {
                        match msg {
                            Ok(Message::Text(text)) => {
                                if let Ok(proxy_msg) = serde_json::from_str::<ProxyMessage>(&text) {
                                    match proxy_msg {
                                        ProxyMessage::ClaudeOutput { content } => {
                                            link.send_message(SessionViewMsg::ReceivedOutput(
                                                content.to_string(),
                                            ));
                                            link.send_message(SessionViewMsg::CheckAwaiting);
                                        }
                                        ProxyMessage::PermissionRequest {
                                            request_id,
                                            tool_name,
                                            input,
                                            permission_suggestions,
                                        } => {
                                            link.send_message(SessionViewMsg::PermissionRequest(
                                                PendingPermission {
                                                    request_id,
                                                    tool_name,
                                                    input,
                                                    permission_suggestions,
                                                },
                                            ));
                                        }
                                        ProxyMessage::Error { message } => {
                                            let error_json = serde_json::json!({
                                                "type": "error",
                                                "message": message
                                            });
                                            link.send_message(SessionViewMsg::ReceivedOutput(
                                                error_json.to_string(),
                                            ));
                                        }
                                        ProxyMessage::SessionUpdate {
                                            session_id: _,
                                            git_branch,
                                        } => {
                                            link.send_message(SessionViewMsg::BranchChanged(
                                                git_branch,
                                            ));
                                        }
                                        _ => {}
                                    }
                                }
                            }
                            Err(e) => {
                                log::error!("WebSocket error: {:?}", e);
                                link.send_message(SessionViewMsg::WebSocketError(format!(
                                    "{:?}",
                                    e
                                )));
                                break;
                            }
                            _ => {}
                        }
                    }
                }
                Err(e) => {
                    log::error!("Failed to connect WebSocket: {:?}", e);
                    link.send_message(SessionViewMsg::WebSocketError(format!("{:?}", e)));
                }
            }
        });

        Self {
            messages: vec![],
            input_value: String::new(),
            ws_connected: false,
            ws_sender: None,
            messages_ref: NodeRef::default(),
            input_ref: NodeRef::default(),
            permission_ref: NodeRef::default(),
            should_autoscroll: Rc::new(RefCell::new(true)),
            scroll_listener: None,
            was_focused: ctx.props().focused,
            total_cost: 0.0,
            cost_flash: false,
            pending_permission: None,
            permission_selected: 0,
            reconnect_attempt: 0,
            reconnect_timer: None,
            command_history: Vec::new(),
            history_position: None,
            draft_input: String::new(),
            is_recording: false,
            interim_transcription: None,
            last_message_timestamp: None,
            voice_button_ref: NodeRef::default(),
            multi_select_options: HashSet::new(),
        }
    }

    fn changed(&mut self, ctx: &Context<Self>, _old_props: &Self::Properties) -> bool {
        let now_focused = ctx.props().focused;
        let became_focused = now_focused && !self.was_focused;
        self.was_focused = now_focused;

        // Focus input when this session becomes visible
        if became_focused {
            if let Some(input) = self.input_ref.cast::<HtmlInputElement>() {
                let _ = input.focus();
            }
        }

        true
    }

    fn rendered(&mut self, ctx: &Context<Self>, first_render: bool) {
        // Focus input on first render only if this session is focused
        if first_render && ctx.props().focused {
            if let Some(input) = self.input_ref.cast::<HtmlInputElement>() {
                let _ = input.focus();
            }
        }

        // Auto-focus permission prompt when it appears
        if self.pending_permission.is_some() && ctx.props().focused {
            if let Some(el) = self.permission_ref.cast::<web_sys::HtmlElement>() {
                let _ = el.focus();
            }
        }

        if let Some(element) = self.messages_ref.cast::<Element>() {
            if first_render {
                let should_autoscroll = self.should_autoscroll.clone();
                let element_clone = element.clone();

                let closure = Closure::new(move || {
                    let scroll_top = element_clone.scroll_top();
                    let scroll_height = element_clone.scroll_height();
                    let client_height = element_clone.client_height();
                    let at_bottom = scroll_height - scroll_top - client_height < 50;
                    *should_autoscroll.borrow_mut() = at_bottom;
                });

                let _ = element
                    .add_event_listener_with_callback("scroll", closure.as_ref().unchecked_ref());

                self.scroll_listener = Some(closure);
            }

            if *self.should_autoscroll.borrow() {
                element.set_scroll_top(element.scroll_height());
            }
        }
    }

    fn update(&mut self, ctx: &Context<Self>, msg: Self::Message) -> bool {
        match msg {
            SessionViewMsg::UpdateInput(value) => {
                self.input_value = value;
                true
            }
            SessionViewMsg::SendInput => {
                let input = self.input_value.trim().to_string();
                if input.is_empty() {
                    return false;
                }

                // Add to command history (avoid consecutive duplicates)
                const MAX_HISTORY: usize = 100;
                if self.command_history.last() != Some(&input) {
                    self.command_history.push(input.clone());
                    // Trim to max size
                    if self.command_history.len() > MAX_HISTORY {
                        self.command_history.remove(0);
                    }
                }
                // Reset history navigation
                self.history_position = None;
                self.draft_input.clear();

                // Don't add to messages here - wait for it to come back via WebSocket
                // (with --replay-user-messages flag, Claude echoes user input back)
                self.input_value.clear();

                // Notify parent that message was sent (for auto-advance)
                let session_id = ctx.props().session.id;
                ctx.props().on_message_sent.emit(session_id);

                if let Some(ref sender_rc) = self.ws_sender {
                    let sender_rc = sender_rc.clone();
                    let msg = ProxyMessage::ClaudeInput {
                        content: serde_json::Value::String(input),
                    };

                    spawn_local(async move {
                        if let Ok(json) = serde_json::to_string(&msg) {
                            let maybe_sender = sender_rc.borrow_mut().take();
                            if let Some(mut sender) = maybe_sender {
                                let _ = sender.send(Message::Text(json)).await;
                                *sender_rc.borrow_mut() = Some(sender);
                            }
                        }
                    });
                }
                true
            }
            SessionViewMsg::LoadHistory(messages, last_timestamp) => {
                // Bulk load - set all at once, no per-message renders
                self.messages = messages;
                // Store timestamp for reconnection replay_after
                self.last_message_timestamp = last_timestamp;
                // Trigger CheckAwaiting to update parent state based on loaded messages
                ctx.link().send_message(SessionViewMsg::CheckAwaiting);
                true
            }
            SessionViewMsg::ReceivedOutput(output) => {
                // Extract cost from result messages (total_cost_usd is cumulative, not incremental)
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&output) {
                    if parsed.get("type").and_then(|t| t.as_str()) == Some("result") {
                        if let Some(cost) = parsed.get("total_cost_usd").and_then(|c| c.as_f64()) {
                            if cost != self.total_cost {
                                self.total_cost = cost;
                                self.cost_flash = true;

                                // Emit cost change to parent
                                let session_id = ctx.props().session.id;
                                ctx.props().on_cost_change.emit((session_id, cost));

                                // Clear flash after animation
                                let link = ctx.link().clone();
                                spawn_local(async move {
                                    gloo::timers::future::TimeoutFuture::new(600).await;
                                    link.send_message(SessionViewMsg::ClearCostFlash);
                                });
                            }
                        }
                    }
                }
                self.messages.push(output);
                // Update timestamp for reconnection - use current time for real-time messages
                self.last_message_timestamp = Some(
                    js_sys::Date::new_0()
                        .to_iso_string()
                        .as_string()
                        .unwrap_or_default(),
                );
                true
            }
            SessionViewMsg::ClearCostFlash => {
                self.cost_flash = false;
                true
            }
            SessionViewMsg::PermissionRequest(perm) => {
                self.pending_permission = Some(perm);
                self.permission_selected = 0; // Default to "Allow"
                                              // Permission requests count as "awaiting" - notify parent
                let session_id = ctx.props().session.id;
                ctx.props().on_awaiting_change.emit((session_id, true));
                // Focus the permission prompt after render
                if let Some(el) = self.permission_ref.cast::<web_sys::HtmlElement>() {
                    let _ = el.focus();
                }
                true
            }
            SessionViewMsg::PermissionSelectUp => {
                if self.pending_permission.is_some() {
                    let max = if self
                        .pending_permission
                        .as_ref()
                        .map(|p| !p.permission_suggestions.is_empty())
                        .unwrap_or(false)
                    {
                        2 // Allow, Allow & Remember, Deny
                    } else {
                        1 // Allow, Deny
                    };
                    if self.permission_selected > 0 {
                        self.permission_selected -= 1;
                    } else {
                        self.permission_selected = max;
                    }
                }
                true
            }
            SessionViewMsg::PermissionSelectDown => {
                if self.pending_permission.is_some() {
                    let max = if self
                        .pending_permission
                        .as_ref()
                        .map(|p| !p.permission_suggestions.is_empty())
                        .unwrap_or(false)
                    {
                        2 // Allow, Allow & Remember, Deny
                    } else {
                        1 // Allow, Deny
                    };
                    if self.permission_selected < max {
                        self.permission_selected += 1;
                    } else {
                        self.permission_selected = 0;
                    }
                }
                true
            }
            SessionViewMsg::PermissionConfirm => {
                if let Some(ref perm) = self.pending_permission {
                    // Check if this is an AskUserQuestion
                    if perm.tool_name == "AskUserQuestion" {
                        if let Some(parsed) = parse_ask_user_question(&perm.input) {
                            if let Some(q) = parsed.questions.first() {
                                if q.multi_select {
                                    // For multi-select, build answer from selected indices
                                    let answer: String = self
                                        .multi_select_options
                                        .iter()
                                        .filter_map(|&idx| {
                                            q.options.get(idx).map(|o| o.label.clone())
                                        })
                                        .collect::<Vec<_>>()
                                        .join(", ");
                                    ctx.link()
                                        .send_message(SessionViewMsg::AnswerQuestion(answer));
                                } else {
                                    // For single-select, get the selected option
                                    if let Some(opt) = q.options.get(self.permission_selected) {
                                        ctx.link().send_message(SessionViewMsg::AnswerQuestion(
                                            opt.label.clone(),
                                        ));
                                    }
                                }
                            }
                        }
                    } else {
                        // Regular permission handling
                        let has_suggestions = !perm.permission_suggestions.is_empty();
                        let msg = match (self.permission_selected, has_suggestions) {
                            (0, _) => SessionViewMsg::ApprovePermission,
                            (1, true) => SessionViewMsg::ApprovePermissionAndRemember,
                            (1, false) => SessionViewMsg::DenyPermission,
                            (2, true) => SessionViewMsg::DenyPermission,
                            _ => SessionViewMsg::ApprovePermission,
                        };
                        ctx.link().send_message(msg);
                    }
                }
                false // Don't re-render, the delegated message will handle it
            }
            SessionViewMsg::PermissionSelectAndConfirm(index) => {
                // Select the option and immediately confirm (for click/touch)
                self.permission_selected = index;
                ctx.link().send_message(SessionViewMsg::PermissionConfirm);
                false // Don't re-render, delegated message will handle it
            }
            SessionViewMsg::ApprovePermission => {
                if let Some(perm) = self.pending_permission.take() {
                    if let Some(ref sender_rc) = self.ws_sender {
                        let sender_rc = sender_rc.clone();
                        let msg = ProxyMessage::PermissionResponse {
                            request_id: perm.request_id,
                            allow: true,
                            input: Some(perm.input),
                            permissions: vec![],
                            reason: None,
                        };
                        spawn_local(async move {
                            if let Ok(json) = serde_json::to_string(&msg) {
                                let maybe_sender = sender_rc.borrow_mut().take();
                                if let Some(mut sender) = maybe_sender {
                                    let _ = sender.send(Message::Text(json)).await;
                                    *sender_rc.borrow_mut() = Some(sender);
                                }
                            }
                        });
                    }
                    // Recheck awaiting state (permission is cleared)
                    ctx.link().send_message(SessionViewMsg::CheckAwaiting);
                    // Focus back to input
                    if let Some(input) = self.input_ref.cast::<HtmlInputElement>() {
                        let _ = input.focus();
                    }
                }
                true
            }
            SessionViewMsg::ApprovePermissionAndRemember => {
                if let Some(perm) = self.pending_permission.take() {
                    if let Some(ref sender_rc) = self.ws_sender {
                        let sender_rc = sender_rc.clone();
                        let msg = ProxyMessage::PermissionResponse {
                            request_id: perm.request_id,
                            allow: true,
                            input: Some(perm.input),
                            permissions: perm.permission_suggestions,
                            reason: None,
                        };
                        spawn_local(async move {
                            if let Ok(json) = serde_json::to_string(&msg) {
                                let maybe_sender = sender_rc.borrow_mut().take();
                                if let Some(mut sender) = maybe_sender {
                                    let _ = sender.send(Message::Text(json)).await;
                                    *sender_rc.borrow_mut() = Some(sender);
                                }
                            }
                        });
                    }
                    // Recheck awaiting state (permission is cleared)
                    ctx.link().send_message(SessionViewMsg::CheckAwaiting);
                    // Focus back to input
                    if let Some(input) = self.input_ref.cast::<HtmlInputElement>() {
                        let _ = input.focus();
                    }
                }
                true
            }
            SessionViewMsg::DenyPermission => {
                if let Some(perm) = self.pending_permission.take() {
                    if let Some(ref sender_rc) = self.ws_sender {
                        let sender_rc = sender_rc.clone();
                        let msg = ProxyMessage::PermissionResponse {
                            request_id: perm.request_id,
                            allow: false,
                            input: None,
                            permissions: vec![],
                            reason: Some("User denied".to_string()),
                        };
                        spawn_local(async move {
                            if let Ok(json) = serde_json::to_string(&msg) {
                                let maybe_sender = sender_rc.borrow_mut().take();
                                if let Some(mut sender) = maybe_sender {
                                    let _ = sender.send(Message::Text(json)).await;
                                    *sender_rc.borrow_mut() = Some(sender);
                                }
                            }
                        });
                    }
                    // Recheck awaiting state (permission is cleared)
                    ctx.link().send_message(SessionViewMsg::CheckAwaiting);
                    // Focus back to input
                    if let Some(input) = self.input_ref.cast::<HtmlInputElement>() {
                        let _ = input.focus();
                    }
                }
                true
            }
            SessionViewMsg::WebSocketConnected(sender) => {
                self.ws_connected = true;
                self.ws_sender = Some(sender);
                self.reconnect_attempt = 0;
                self.reconnect_timer = None;
                let session_id = ctx.props().session.id;
                ctx.props().on_connected_change.emit((session_id, true));
                true
            }
            SessionViewMsg::WebSocketError(err) => {
                self.ws_connected = false;
                self.ws_sender = None;
                let session_id = ctx.props().session.id;
                ctx.props().on_connected_change.emit((session_id, false));

                // Schedule reconnection with exponential backoff (max 10 attempts)
                const MAX_ATTEMPTS: u32 = 10;
                if self.reconnect_attempt < MAX_ATTEMPTS {
                    self.reconnect_attempt += 1;
                    let delay_ms = calculate_backoff(self.reconnect_attempt - 1);
                    log::info!(
                        "WebSocket disconnected, reconnecting in {}ms (attempt {})",
                        delay_ms,
                        self.reconnect_attempt
                    );

                    let link = ctx.link().clone();
                    self.reconnect_timer = Some(Timeout::new(delay_ms, move || {
                        link.send_message(SessionViewMsg::AttemptReconnect);
                    }));
                } else {
                    // Max attempts reached, show error
                    let error_msg = serde_json::json!({
                        "type": "error",
                        "message": format!("Connection lost: {}", err)
                    });
                    self.messages.push(error_msg.to_string());
                }
                true
            }
            SessionViewMsg::AttemptReconnect => {
                let link = ctx.link().clone();
                let session_id = ctx.props().session.id;
                let replay_after = self.last_message_timestamp.clone();

                spawn_local(async move {
                    let ws_endpoint = utils::ws_url("/ws/client");
                    match WebSocket::open(&ws_endpoint) {
                        Ok(ws) => {
                            let (mut sender, mut receiver) = ws.split();

                            let register_msg = ProxyMessage::Register {
                                session_id,
                                session_name: session_id.to_string(),
                                auth_token: None,
                                working_directory: String::new(),
                                resuming: true, // Mark as resuming connection
                                git_branch: None,
                                replay_after, // Only get messages after last seen
                            };

                            if let Ok(json) = serde_json::to_string(&register_msg) {
                                if sender.send(Message::Text(json)).await.is_err() {
                                    link.send_message(SessionViewMsg::WebSocketError(
                                        "Failed to send registration".to_string(),
                                    ));
                                    return;
                                }
                            }

                            let sender = Rc::new(RefCell::new(Some(sender)));
                            link.send_message(SessionViewMsg::WebSocketConnected(sender));

                            while let Some(msg) = receiver.next().await {
                                match msg {
                                    Ok(Message::Text(text)) => {
                                        if let Ok(proxy_msg) =
                                            serde_json::from_str::<ProxyMessage>(&text)
                                        {
                                            match proxy_msg {
                                                ProxyMessage::ClaudeOutput { content } => {
                                                    link.send_message(
                                                        SessionViewMsg::ReceivedOutput(
                                                            content.to_string(),
                                                        ),
                                                    );
                                                    link.send_message(
                                                        SessionViewMsg::CheckAwaiting,
                                                    );
                                                }
                                                ProxyMessage::PermissionRequest {
                                                    request_id,
                                                    tool_name,
                                                    input,
                                                    permission_suggestions,
                                                } => {
                                                    link.send_message(
                                                        SessionViewMsg::PermissionRequest(
                                                            PendingPermission {
                                                                request_id,
                                                                tool_name,
                                                                input,
                                                                permission_suggestions,
                                                            },
                                                        ),
                                                    );
                                                }
                                                ProxyMessage::Error { message } => {
                                                    let error_json = serde_json::json!({
                                                        "type": "error",
                                                        "message": message
                                                    });
                                                    link.send_message(
                                                        SessionViewMsg::ReceivedOutput(
                                                            error_json.to_string(),
                                                        ),
                                                    );
                                                }
                                                ProxyMessage::SessionUpdate {
                                                    session_id: _,
                                                    git_branch,
                                                } => {
                                                    link.send_message(
                                                        SessionViewMsg::BranchChanged(git_branch),
                                                    );
                                                }
                                                _ => {}
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        log::error!("WebSocket error: {:?}", e);
                                        link.send_message(SessionViewMsg::WebSocketError(format!(
                                            "{:?}",
                                            e
                                        )));
                                        break;
                                    }
                                    _ => {}
                                }
                            }
                        }
                        Err(e) => {
                            log::error!("Failed to reconnect WebSocket: {:?}", e);
                            link.send_message(SessionViewMsg::WebSocketError(format!("{:?}", e)));
                        }
                    }
                });
                false
            }
            SessionViewMsg::CheckAwaiting => {
                // Check if last message is a result (awaiting input) OR if there's a pending permission request
                let is_result_awaiting = self.messages.last().is_some_and(|msg| {
                    serde_json::from_str::<serde_json::Value>(msg)
                        .ok()
                        .and_then(|p| {
                            p.get("type")
                                .and_then(|t| t.as_str())
                                .map(|t| t == "result")
                        })
                        .unwrap_or(false)
                });
                // Permission requests also count as "awaiting" - they block Claude
                let is_awaiting = is_result_awaiting || self.pending_permission.is_some();
                let session_id = ctx.props().session.id;
                ctx.props()
                    .on_awaiting_change
                    .emit((session_id, is_awaiting));
                false
            }
            SessionViewMsg::BranchChanged(branch) => {
                let session_id = ctx.props().session.id;
                ctx.props().on_branch_change.emit((session_id, branch));
                false
            }
            SessionViewMsg::HistoryUp => {
                if self.command_history.is_empty() {
                    return false;
                }
                match self.history_position {
                    None => {
                        // First time pressing up - save current input as draft
                        self.draft_input = self.input_value.clone();
                        // Go to most recent command
                        let pos = self.command_history.len() - 1;
                        self.history_position = Some(pos);
                        self.input_value = self.command_history[pos].clone();
                    }
                    Some(pos) if pos > 0 => {
                        // Go to older command
                        let new_pos = pos - 1;
                        self.history_position = Some(new_pos);
                        self.input_value = self.command_history[new_pos].clone();
                    }
                    _ => {
                        // Already at oldest, do nothing
                        return false;
                    }
                }
                true
            }
            SessionViewMsg::HistoryDown => {
                match self.history_position {
                    Some(pos) if pos < self.command_history.len() - 1 => {
                        // Go to newer command
                        let new_pos = pos + 1;
                        self.history_position = Some(new_pos);
                        self.input_value = self.command_history[new_pos].clone();
                    }
                    Some(_) => {
                        // At newest history entry, go back to draft
                        self.history_position = None;
                        self.input_value = self.draft_input.clone();
                    }
                    None => {
                        // Not in history mode, do nothing
                        return false;
                    }
                }
                true
            }
            SessionViewMsg::VoiceRecordingChanged(recording) => {
                self.is_recording = recording;
                // Clear interim transcription when recording stops
                if !recording {
                    self.interim_transcription = None;
                }
                true
            }
            SessionViewMsg::VoiceTranscription(text) => {
                // Final transcription - commit to input field, clear interim, and auto-send
                // With single_utterance mode, this is the complete spoken message
                self.interim_transcription = None;
                if !text.is_empty() {
                    // Append final transcription to input_value
                    if self.input_value.is_empty() {
                        self.input_value = text;
                    } else {
                        self.input_value.push(' ');
                        self.input_value.push_str(&text);
                    }
                    // Auto-send the message now that we have a complete utterance
                    ctx.link().send_message(SessionViewMsg::SendInput);
                }
                true
            }
            SessionViewMsg::VoiceInterimTranscription(text) => {
                // Interim transcription - this is Google's current best guess for the utterance
                // It replaces previous interim (not accumulates) because Google sends the full
                // current guess each time, not incremental words
                self.interim_transcription = if text.is_empty() { None } else { Some(text) };
                true
            }
            SessionViewMsg::VoiceError(err) => {
                log::error!("Voice error: {}", err);
                self.is_recording = false;
                self.interim_transcription = None;
                true
            }
            SessionViewMsg::ToggleVoice => {
                // Programmatically click the voice button if it exists
                if let Some(button) = self.voice_button_ref.cast::<web_sys::HtmlElement>() {
                    button.click();
                }
                false
            }
            SessionViewMsg::AnswerQuestion(answer) => {
                if let Some(perm) = self.pending_permission.take() {
                    if let Some(ref sender_rc) = self.ws_sender {
                        let sender_rc = sender_rc.clone();
                        // Parse the question to get the question text as key
                        let answers = if let Some(parsed) = parse_ask_user_question(&perm.input) {
                            if let Some(q) = parsed.questions.first() {
                                serde_json::json!({
                                    "answers": {
                                        q.question.clone(): answer
                                    }
                                })
                            } else {
                                serde_json::json!({ "answers": { "": answer } })
                            }
                        } else {
                            serde_json::json!({ "answers": { "": answer } })
                        };

                        let msg = ProxyMessage::PermissionResponse {
                            request_id: perm.request_id,
                            allow: true,
                            input: Some(answers),
                            permissions: vec![],
                            reason: None,
                        };
                        spawn_local(async move {
                            if let Ok(json) = serde_json::to_string(&msg) {
                                let maybe_sender = sender_rc.borrow_mut().take();
                                if let Some(mut sender) = maybe_sender {
                                    let _ = sender.send(Message::Text(json)).await;
                                    *sender_rc.borrow_mut() = Some(sender);
                                }
                            }
                        });
                    }
                    // Clear multi-select state
                    self.multi_select_options.clear();
                    // Recheck awaiting state
                    ctx.link().send_message(SessionViewMsg::CheckAwaiting);
                    // Focus back to input
                    if let Some(input) = self.input_ref.cast::<HtmlInputElement>() {
                        let _ = input.focus();
                    }
                }
                true
            }
            SessionViewMsg::ToggleQuestionOption(index) => {
                if self.multi_select_options.contains(&index) {
                    self.multi_select_options.remove(&index);
                } else {
                    self.multi_select_options.insert(index);
                }
                true
            }
        }
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        let link = ctx.link();

        let handle_submit = link.callback(|e: SubmitEvent| {
            e.prevent_default();
            SessionViewMsg::SendInput
        });

        let handle_input = link.callback(|e: InputEvent| {
            let input: HtmlInputElement = e.target_unchecked_into();
            SessionViewMsg::UpdateInput(input.value())
        });

        let handle_keydown = link.callback(|e: KeyboardEvent| {
            // Ctrl+Shift+M or Ctrl+M to toggle voice recording
            if e.ctrl_key() && e.key().to_lowercase() == "m" {
                e.prevent_default();
                return SessionViewMsg::ToggleVoice;
            }

            match e.key().as_str() {
                "ArrowUp" => {
                    e.prevent_default();
                    SessionViewMsg::HistoryUp
                }
                "ArrowDown" => {
                    e.prevent_default();
                    SessionViewMsg::HistoryDown
                }
                _ => SessionViewMsg::CheckAwaiting, // No-op
            }
        });

        html! {
            <div class="session-view">
                <div class="session-view-messages" ref={self.messages_ref.clone()}>
                    {
                        group_messages(&self.messages).into_iter().map(|group| {
                            html! { <MessageGroupRenderer group={group} session_id={Some(ctx.props().session.id)} /> }
                        }).collect::<Html>()
                    }
                </div>

                {
                    if let Some(ref perm) = self.pending_permission {
                        // Check if this is an AskUserQuestion
                        if perm.tool_name == "AskUserQuestion" {
                            if let Some(parsed) = parse_ask_user_question(&perm.input) {
                                // Render specialized question UI
                                let multi_select_options = self.multi_select_options.clone();
                                let selected = self.permission_selected;

                                // For single-select questions, use keyboard navigation
                                let onkeydown = link.callback(|e: KeyboardEvent| {
                                    match e.key().as_str() {
                                        "ArrowUp" | "k" => {
                                            e.prevent_default();
                                            SessionViewMsg::PermissionSelectUp
                                        }
                                        "ArrowDown" | "j" => {
                                            e.prevent_default();
                                            SessionViewMsg::PermissionSelectDown
                                        }
                                        "Enter" | " " => {
                                            e.prevent_default();
                                            SessionViewMsg::PermissionConfirm
                                        }
                                        _ => SessionViewMsg::CheckAwaiting, // No-op
                                    }
                                });

                                html! {
                                    <div
                                        class="permission-prompt ask-user-question"
                                        ref={self.permission_ref.clone()}
                                        tabindex="0"
                                        onkeydown={onkeydown}
                                    >
                                        {
                                            parsed.questions.iter().map(|q| {
                                                let is_multi = q.multi_select;
                                                html! {
                                                    <div class="question-container">
                                                        {
                                                            if !q.header.is_empty() {
                                                                html! {
                                                                    <div class="question-header-badge">
                                                                        <span class="badge">{ &q.header }</span>
                                                                        {
                                                                            if is_multi {
                                                                                html! { <span class="multi-badge">{ "multi-select" }</span> }
                                                                            } else {
                                                                                html! {}
                                                                            }
                                                                        }
                                                                    </div>
                                                                }
                                                            } else if is_multi {
                                                                html! {
                                                                    <div class="question-header-badge">
                                                                        <span class="multi-badge">{ "multi-select" }</span>
                                                                    </div>
                                                                }
                                                            } else {
                                                                html! {}
                                                            }
                                                        }
                                                        <div class="question-text">{ &q.question }</div>
                                                        <div class="question-options">
                                                            {
                                                                q.options.iter().enumerate().map(|(i, opt)| {
                                                                    let is_selected = if is_multi {
                                                                        multi_select_options.contains(&i)
                                                                    } else {
                                                                        i == selected
                                                                    };
                                                                    let item_class = if is_selected {
                                                                        "question-option selected"
                                                                    } else {
                                                                        "question-option"
                                                                    };
                                                                    let label_clone = opt.label.clone();
                                                                    let onclick = if is_multi {
                                                                        link.callback(move |_| SessionViewMsg::ToggleQuestionOption(i))
                                                                    } else {
                                                                        link.callback(move |_| SessionViewMsg::AnswerQuestion(label_clone.clone()))
                                                                    };
                                                                    let icon = if is_selected {
                                                                        if is_multi { "☑" } else { "●" }
                                                                    } else if is_multi {
                                                                        "☐"
                                                                    } else {
                                                                        "○"
                                                                    };

                                                                    html! {
                                                                        <div class={item_class} onclick={onclick}>
                                                                            <span class="option-icon">{ icon }</span>
                                                                            <div class="option-content">
                                                                                <span class="option-label">{ &opt.label }</span>
                                                                                {
                                                                                    if !opt.description.is_empty() {
                                                                                        html! { <span class="option-description">{ &opt.description }</span> }
                                                                                    } else {
                                                                                        html! {}
                                                                                    }
                                                                                }
                                                                            </div>
                                                                        </div>
                                                                    }
                                                                }).collect::<Html>()
                                                            }
                                                        </div>
                                                        {
                                                            // Show submit button for multi-select
                                                            if is_multi {
                                                                let options_clone = q.options.clone();
                                                                let multi_select_clone = multi_select_options.clone();
                                                                let onclick = link.callback(move |_| {
                                                                    // Build comma-separated answer from selected indices
                                                                    let answer: String = multi_select_clone
                                                                        .iter()
                                                                        .filter_map(|&idx| options_clone.get(idx).map(|o| o.label.clone()))
                                                                        .collect::<Vec<_>>()
                                                                        .join(", ");
                                                                    SessionViewMsg::AnswerQuestion(answer)
                                                                });
                                                                html! {
                                                                    <button class="submit-answer" onclick={onclick} disabled={multi_select_options.is_empty()}>
                                                                        { "Submit" }
                                                                    </button>
                                                                }
                                                            } else {
                                                                html! {}
                                                            }
                                                        }
                                                    </div>
                                                }
                                            }).collect::<Html>()
                                        }
                                        <div class="question-hint">
                                            { "Click an option or use ↑↓ and Enter" }
                                        </div>
                                    </div>
                                }
                            } else {
                                // Fallback to regular permission UI if parsing fails
                                render_permission_dialog(link, perm, self.permission_selected, self.permission_ref.clone())
                            }
                        } else {
                            // Regular permission dialog
                            render_permission_dialog(link, perm, self.permission_selected, self.permission_ref.clone())
                        }
                    } else {
                        html! {}
                    }
                }

                <form class="session-view-input" onsubmit={handle_submit}>
                    <span class="input-prompt">{ ">" }</span>
                    {
                        // Show combined text (committed + interim) as overlay when recording
                        if let Some(ref interim) = self.interim_transcription {
                            // Build the full preview: committed text + interim
                            let preview = if self.input_value.is_empty() {
                                interim.clone()
                            } else {
                                format!("{} {}", self.input_value, interim)
                            };
                            html! {
                                <div class="interim-transcription">{ preview }</div>
                            }
                        } else {
                            html! {}
                        }
                    }
                    <input
                        ref={self.input_ref.clone()}
                        type="text"
                        class={classes!(
                            "message-input",
                            self.interim_transcription.is_some().then_some("has-interim")
                        )}
                        placeholder="Type your message..."
                        value={self.input_value.clone()}
                        oninput={handle_input}
                        onkeydown={handle_keydown}
                        disabled={!self.ws_connected}
                    />
                    {
                        if ctx.props().voice_enabled {
                            let session_id = ctx.props().session.id;
                            let on_recording_change = link.callback(SessionViewMsg::VoiceRecordingChanged);
                            let on_transcription = link.callback(SessionViewMsg::VoiceTranscription);
                            let on_interim_transcription = link.callback(SessionViewMsg::VoiceInterimTranscription);
                            let on_error = link.callback(SessionViewMsg::VoiceError);
                            let button_ref = self.voice_button_ref.clone();
                            html! {
                                <VoiceInput
                                    {session_id}
                                    {on_recording_change}
                                    {on_transcription}
                                    on_interim_transcription={Some(on_interim_transcription)}
                                    {on_error}
                                    disabled={!self.ws_connected}
                                    button_ref={Some(button_ref)}
                                />
                            }
                        } else {
                            html! {}
                        }
                    }
                    <button type="submit" class="send-button" disabled={!self.ws_connected}>
                        { "Send" }
                    </button>
                </form>
            </div>
        }
    }
}

fn format_permission_input(tool_name: &str, input: &serde_json::Value) -> String {
    match tool_name {
        "Bash" => input
            .get("command")
            .and_then(|v| v.as_str())
            .map(|s| format!("$ {}", s))
            .unwrap_or_else(|| serde_json::to_string_pretty(input).unwrap_or_default()),
        "Read" | "Edit" | "Write" => input
            .get("file_path")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| serde_json::to_string_pretty(input).unwrap_or_default()),
        _ => serde_json::to_string_pretty(input).unwrap_or_else(|_| format!("{:?}", input)),
    }
}

/// Render the standard permission dialog (Allow/Deny)
fn render_permission_dialog(
    link: &yew::html::Scope<SessionView>,
    perm: &PendingPermission,
    selected: usize,
    permission_ref: NodeRef,
) -> Html {
    let input_preview = format_permission_input(&perm.tool_name, &perm.input);
    let has_suggestions = !perm.permission_suggestions.is_empty();

    let onkeydown = link.callback(|e: KeyboardEvent| {
        match e.key().as_str() {
            "ArrowUp" | "k" => {
                e.prevent_default();
                SessionViewMsg::PermissionSelectUp
            }
            "ArrowDown" | "j" => {
                e.prevent_default();
                SessionViewMsg::PermissionSelectDown
            }
            "Enter" | " " => {
                e.prevent_default();
                SessionViewMsg::PermissionConfirm
            }
            _ => SessionViewMsg::CheckAwaiting, // No-op
        }
    });

    // Build options list
    let options: Vec<(&str, &str)> = if has_suggestions {
        vec![
            ("allow", "Allow"),
            ("remember", "Allow & Remember"),
            ("deny", "Deny"),
        ]
    } else {
        vec![("allow", "Allow"), ("deny", "Deny")]
    };

    html! {
        <div
            class="permission-prompt"
            ref={permission_ref}
            tabindex="0"
            onkeydown={onkeydown}
        >
            <div class="permission-header">
                <span class="permission-icon">{ "⚠️" }</span>
                <span class="permission-title">{ "Permission Required" }</span>
            </div>
            <div class="permission-body">
                <div class="permission-tool">
                    <span class="tool-label">{ "Tool:" }</span>
                    <span class="tool-name">{ &perm.tool_name }</span>
                </div>
                <div class="permission-input">
                    <pre>{ input_preview }</pre>
                </div>
            </div>
            <div class="permission-options">
                {
                    options.iter().enumerate().map(|(i, (class, label))| {
                        let is_selected = i == selected;
                        let cursor = if is_selected { ">" } else { " " };
                        let item_class = if is_selected {
                            format!("permission-option selected {}", class)
                        } else {
                            format!("permission-option {}", class)
                        };
                        let onclick = link.callback(move |_| {
                            SessionViewMsg::PermissionSelectAndConfirm(i)
                        });
                        html! {
                            <div class={item_class} {onclick}>
                                <span class="option-cursor">{ cursor }</span>
                                <span class="option-label">{ *label }</span>
                            </div>
                        }
                    }).collect::<Html>()
                }
            </div>
            <div class="permission-hint">
                { "↑↓ or tap to select" }
            </div>
        </div>
    }
}
