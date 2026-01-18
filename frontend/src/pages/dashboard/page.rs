//! Dashboard page - Main session management interface

use super::session_rail::SessionRail;
use super::session_view::SessionView;
use super::types::{
    calculate_backoff, load_inactive_hidden, load_paused_sessions, save_inactive_hidden,
    save_paused_sessions,
};
use crate::components::ProxyTokenSetup;
use crate::utils;
use crate::Route;
use futures_util::StreamExt;
use gloo_net::http::Request;
use gloo_net::websocket::{futures::WebSocket, Message};
use shared::{AppConfig, ProxyMessage, SessionCost, SessionInfo};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;
use wasm_bindgen_futures::spawn_local;
use web_sys::KeyboardEvent;
use yew::prelude::*;
use yew_router::prelude::*;

// =============================================================================
// Dashboard Page - Main Orchestrating Component
// =============================================================================

#[function_component(DashboardPage)]
pub fn dashboard_page() -> Html {
    let navigator = use_navigator().unwrap();
    let sessions = use_state(Vec::<SessionInfo>::new);
    let loading = use_state(|| true);
    let refresh_trigger = use_state(|| 0u32);
    let show_new_session = use_state(|| false);
    let focused_index = use_state(|| 0usize);
    let awaiting_sessions = use_state(HashSet::<Uuid>::new);
    let paused_sessions = use_state(load_paused_sessions);
    let inactive_hidden = use_state(load_inactive_hidden);
    let session_costs = use_state(HashMap::<Uuid, f64>::new);
    let connected_sessions = use_state(HashSet::<Uuid>::new);
    let pending_leave = use_state(|| None::<Uuid>);
    let nav_mode = use_state(|| false);
    let total_user_spend = use_state(|| 0.0f64);
    let is_admin = use_state(|| false);
    let voice_enabled = use_state(|| false);
    // App title from backend config (customizable via APP_TITLE env var)
    let app_title = use_state(|| "Claude Code Sessions".to_string());
    // Track which sessions have been activated (focused at least once)
    // This prevents loading history for paused sessions until they're selected
    let activated_sessions = use_state(HashSet::<Uuid>::new);
    // Track if initial focus has been set (to pick first non-paused session)
    let initial_focus_set = use_state(|| false);
    // Server shutdown notification (shown as toast)
    let server_shutdown_reason = use_state(|| None::<String>);

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

    // Fetch app configuration (title, etc.)
    {
        let app_title = app_title.clone();
        use_effect_with((), move |_| {
            spawn_local(async move {
                let api_endpoint = utils::api_url("/api/config");
                if let Ok(response) = Request::get(&api_endpoint).send().await {
                    if let Ok(config) = response.json::<AppConfig>().await {
                        app_title.set(config.app_title);
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

    // Set initial focus to first non-paused session (once sessions are loaded)
    {
        let sessions = sessions.clone();
        let paused_sessions = paused_sessions.clone();
        let focused_index = focused_index.clone();
        let initial_focus_set = initial_focus_set.clone();
        let activated_sessions = activated_sessions.clone();
        let loading = loading.clone();

        use_effect_with(
            (sessions.len(), *loading),
            move |(session_count, is_loading)| {
                // Only set initial focus once, after sessions are loaded
                if !*initial_focus_set && !*is_loading && *session_count > 0 {
                    // Sort sessions the same way active_sessions does (active first, then by name)
                    let mut sorted: Vec<_> = sessions.iter().cloned().collect();
                    sorted.sort_by(|a, b| {
                        let a_is_active = a.status.as_str() == "active";
                        let b_is_active = b.status.as_str() == "active";
                        match (a_is_active, b_is_active) {
                            (true, false) => std::cmp::Ordering::Less,
                            (false, true) => std::cmp::Ordering::Greater,
                            _ => {
                                let folder_a = utils::extract_folder(&a.working_directory);
                                let folder_b = utils::extract_folder(&b.working_directory);
                                match folder_a.to_lowercase().cmp(&folder_b.to_lowercase()) {
                                    std::cmp::Ordering::Equal => {
                                        let hostname_a = utils::extract_hostname(&a.session_name);
                                        let hostname_b = utils::extract_hostname(&b.session_name);
                                        hostname_a.to_lowercase().cmp(&hostname_b.to_lowercase())
                                    }
                                    other => other,
                                }
                            }
                        }
                    });

                    // Find first non-paused session
                    let first_non_paused_idx = sorted
                        .iter()
                        .position(|s| !paused_sessions.contains(&s.id))
                        .unwrap_or(0);

                    focused_index.set(first_non_paused_idx);

                    // Mark the initially focused session as activated
                    if let Some(session) = sorted.get(first_non_paused_idx) {
                        let mut activated = (*activated_sessions).clone();
                        activated.insert(session.id);
                        activated_sessions.set(activated);
                    }

                    initial_focus_set.set(true);
                }
                || ()
            },
        );
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
        let server_shutdown_reason = server_shutdown_reason.clone();
        use_effect_with((), move |_| {
            let total_user_spend = total_user_spend.clone();
            let session_costs = session_costs.clone();
            let server_shutdown_reason = server_shutdown_reason.clone();
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
                                        if let Ok(proxy_msg) =
                                            serde_json::from_str::<ProxyMessage>(&text)
                                        {
                                            match proxy_msg {
                                                ProxyMessage::UserSpendUpdate {
                                                    total_spend_usd,
                                                    session_costs: costs,
                                                } => {
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
                                                ProxyMessage::ServerShutdown {
                                                    reason,
                                                    reconnect_delay_ms,
                                                } => {
                                                    log::info!(
                                                        "Server shutdown: {} (reconnect in {}ms)",
                                                        reason,
                                                        reconnect_delay_ms
                                                    );
                                                    server_shutdown_reason.set(Some(reason));
                                                }
                                                _ => {}
                                            }
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

    // Navigation callbacks
    let go_to_admin = {
        let navigator = navigator.clone();
        Callback::from(move |_| navigator.push(&Route::Admin))
    };

    let go_to_settings = {
        let navigator = navigator.clone();
        Callback::from(move |_| navigator.push(&Route::Settings))
    };

    let do_logout = Callback::from(move |_| {
        if let Some(window) = web_sys::window() {
            let _ = window.location().set_href("/api/auth/logout");
        }
    });

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
                    // Same status - sort by folder name then hostname
                    let folder_a = utils::extract_folder(&a.working_directory);
                    let folder_b = utils::extract_folder(&b.working_directory);
                    match folder_a.to_lowercase().cmp(&folder_b.to_lowercase()) {
                        std::cmp::Ordering::Equal => {
                            let hostname_a = utils::extract_hostname(&a.session_name);
                            let hostname_b = utils::extract_hostname(&b.session_name);
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
        let activated_sessions = activated_sessions.clone();
        let active_sessions = active_sessions.clone();
        Callback::from(move |index: usize| {
            focused_index.set(index);
            // Mark this session as activated so it loads its history
            if let Some(session) = active_sessions.get(index) {
                let mut activated = (*activated_sessions).clone();
                activated.insert(session.id);
                activated_sessions.set(activated);
            }
        })
    };

    let on_navigate = {
        let focused_index = focused_index.clone();
        let active_sessions = active_sessions.clone();
        let paused_sessions = paused_sessions.clone();
        let activated_sessions = activated_sessions.clone();
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

            // Helper to activate a session at index
            let activate_session = |idx: usize| {
                if let Some(session) = active_sessions.get(idx) {
                    let mut activated = (*activated_sessions).clone();
                    activated.insert(session.id);
                    activated_sessions.set(activated);
                }
            };

            // If all sessions are paused, allow normal navigation
            if non_paused_count == 0 {
                let current = *focused_index as i32;
                let new_index = (current + delta).rem_euclid(len as i32) as usize;
                focused_index.set(new_index);
                activate_session(new_index);
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
                        activate_session(new_index);
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
        let activated_sessions = activated_sessions.clone();
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
                        // Mark as activated
                        let mut activated = (*activated_sessions).clone();
                        activated.insert(session.id);
                        activated_sessions.set(activated);
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
            save_paused_sessions(&set);
            paused_sessions.set(set);
        })
    };

    let on_toggle_inactive_hidden = {
        let inactive_hidden = inactive_hidden.clone();
        Callback::from(move |_: MouseEvent| {
            let new_val = !*inactive_hidden;
            save_inactive_hidden(new_val);
            inactive_hidden.set(new_val);
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

    // Count only non-paused sessions that are awaiting input
    let waiting_count = awaiting_sessions
        .iter()
        .filter(|id| !paused_sessions.contains(id))
        .count();

    // Update browser tab title based on waiting sessions count
    {
        let app_title = app_title.clone();
        use_effect_with(
            (waiting_count, (*app_title).clone()),
            move |(count, title)| {
                if let Some(window) = web_sys::window() {
                    if let Some(document) = window.document() {
                        let new_title = if *count > 0 {
                            format!("({}) {}", count, title)
                        } else {
                            title.clone()
                        };
                        document.set_title(&new_title);
                    }
                }
                || ()
            },
        );
    }

    // Count disconnected sessions for the reconnection banner
    // Only count sessions that are both activated (have started loading) and not paused
    let disconnected_count = active_sessions
        .iter()
        .filter(|s| {
            activated_sessions.contains(&s.id)
                && !paused_sessions.contains(&s.id)
                && !connected_sessions.contains(&s.id)
        })
        .count();

    // Two-mode keyboard handling:
    // - Edit Mode (default): typing works, Escape -> Nav Mode, Shift+Tab -> next active (skips paused)
    // - Nav Mode: arrow keys navigate, Enter/Escape -> Edit Mode, numbers select directly
    let on_keydown = {
        let on_navigate = on_navigate.clone();
        let on_next_active = on_next_active.clone();
        let on_select_session = on_select_session.clone();
        let nav_mode = nav_mode.clone();
        let active_sessions = active_sessions.clone();
        let inactive_hidden = inactive_hidden.clone();
        let connected_sessions = connected_sessions.clone();
        let paused_sessions = paused_sessions.clone();
        Callback::from(move |e: KeyboardEvent| {
            let in_nav_mode = *nav_mode;

            // Shift+Tab always jumps to next active session, skipping paused (works in both modes)
            if e.shift_key() && e.key() == "Tab" {
                e.prevent_default();
                on_next_active.emit(());
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
                        // Number keys 1-9 for direct selection based on visible order
                        if let Ok(num) = key.parse::<usize>() {
                            if (1..=9).contains(&num) {
                                // Build visible session indices in display order
                                // Active (connected and not paused) come first, then inactive
                                let mut visible_indices: Vec<usize> = Vec::new();

                                // Add active sessions first
                                for (idx, session) in active_sessions.iter().enumerate() {
                                    let is_connected = connected_sessions.contains(&session.id);
                                    let is_paused = paused_sessions.contains(&session.id);
                                    if is_connected && !is_paused {
                                        visible_indices.push(idx);
                                    }
                                }

                                // Add inactive sessions if not hidden
                                if !*inactive_hidden {
                                    for (idx, session) in active_sessions.iter().enumerate() {
                                        let is_connected = connected_sessions.contains(&session.id);
                                        let is_paused = paused_sessions.contains(&session.id);
                                        if !is_connected || is_paused {
                                            visible_indices.push(idx);
                                        }
                                    }
                                }

                                // Map display number (1-based) to actual index
                                let display_idx = num - 1;
                                if display_idx < visible_indices.len() {
                                    e.prevent_default();
                                    on_select_session.emit(visible_indices[display_idx]);
                                    nav_mode.set(false);
                                }
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
            // Server shutdown warning banner
            {
                if let Some(reason) = (*server_shutdown_reason).as_ref() {
                    html! {
                        <div class="server-shutdown-banner">
                            <span class="shutdown-icon">{ "⚠" }</span>
                            <span class="shutdown-text">{ format!("Server shutting down: {} — reconnecting...", reason) }</span>
                        </div>
                    }
                } else {
                    html! {}
                }
            }
            // Header with new session button
            <header class="focus-flow-header">
                <h1>{ (*app_title).clone() }</h1>
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
                                <button class="header-button" onclick={go_to_admin.clone()}>
                                    { "Admin" }
                                </button>
                            }
                        } else {
                            html! {}
                        }
                    }
                    <button class="header-button" onclick={go_to_settings.clone()}>
                        { "Settings" }
                    </button>
                    <button class="header-button logout" onclick={do_logout.clone()}>
                        { "Logout" }
                    </button>
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
                <div class="onboarding-container">
                    <div class="onboarding-content">
                        <h2>{ "No Sessions Connected" }</h2>

                        <div class="onboarding-steps">
                            <div class="onboarding-step">
                                <span class="step-number">{ "1" }</span>
                                <div class="step-content">
                                    <p>{ "Click " }<strong>{ "+ New Session" }</strong>{ " above to get a setup command" }</p>
                                </div>
                            </div>

                            <div class="onboarding-step">
                                <span class="step-number">{ "2" }</span>
                                <div class="step-content">
                                    <p>{ "Run that command on your dev machine to connect Claude Code" }</p>
                                </div>
                            </div>
                        </div>
                    </div>
                </div>
            } else {
                <>
                    // Session Rail (horizontal carousel)
                    <SessionRail
                        sessions={active_sessions.clone()}
                        focused_index={*focused_index}
                        awaiting_sessions={(*awaiting_sessions).clone()}
                        paused_sessions={(*paused_sessions).clone()}
                        inactive_hidden={*inactive_hidden}
                        session_costs={(*session_costs).clone()}
                        connected_sessions={(*connected_sessions).clone()}
                        nav_mode={*nav_mode}
                        on_select={on_select_session.clone()}
                        on_leave={on_leave.clone()}
                        on_toggle_pause={on_toggle_pause.clone()}
                        on_toggle_inactive_hidden={on_toggle_inactive_hidden.clone()}
                    />

                    // Render session views only for activated sessions (focused at least once)
                    // This prevents loading history for paused sessions until they're selected
                    <div class={classes!("session-views-container", if *nav_mode { Some("nav-mode") } else { None })}>
                        {
                            active_sessions.iter().enumerate().map(|(index, session)| {
                                let is_focused = index == *focused_index;
                                let is_activated = activated_sessions.contains(&session.id);
                                // Only render SessionView if session has been activated
                                // This prevents fetching history for paused sessions until selected
                                if is_activated {
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
                                } else {
                                    // Placeholder for non-activated sessions
                                    html! {
                                        <div
                                            key={session.id.to_string()}
                                            class="session-view-wrapper hidden"
                                        />
                                    }
                                }
                            }).collect::<Html>()
                        }
                    </div>

                    // Keyboard hints - context-sensitive based on mode
                    <div class={classes!("keyboard-hints", if *nav_mode { Some("nav-mode") } else { None })}>
                        <div class="hints-content">
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
                                            if *voice_enabled {
                                                <span>{ "Ctrl+M = voice" }</span>
                                            }
                                            <span>{ "Enter = send" }</span>
                                        </>
                                    }
                                }
                            }
                        </div>
                        <a
                            href="https://github.com/meawoppl/claude-code-portal/issues/new"
                            target="_blank"
                            rel="noopener noreferrer"
                            class="bug-report-link"
                        >
                            { "🐛 Report a Bug" }
                        </a>
                    </div>
                </>
            }

            // Leave confirmation modal (for non-owners)
            {
                if let Some(session_id) = *pending_leave {
                    let session_name = sessions.iter()
                        .find(|s| s.id == session_id)
                        .map(|s| utils::extract_folder(&s.working_directory))
                        .unwrap_or("this session");

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
        </div>
    }
}
