//! Admin dashboard page
//!
//! Restricted to users with is_admin=true. Provides system overview,
//! user management, and session management capabilities.

use crate::utils;
use crate::Route;
use gloo_net::http::Request;
use serde::Deserialize;
use uuid::Uuid;
use wasm_bindgen_futures::spawn_local;
use web_sys::MouseEvent;
use yew::prelude::*;
use yew_router::prelude::*;

/// Admin page tabs
#[derive(Clone, Copy, PartialEq)]
enum AdminTab {
    Overview,
    Users,
    Sessions,
}

// ============================================================================
// API Response Types
// ============================================================================

#[derive(Debug, Clone, Deserialize, PartialEq)]
struct AdminStats {
    total_users: i64,
    admin_users: i64,
    disabled_users: i64,
    total_sessions: i64,
    active_sessions: i64,
    connected_proxy_clients: usize,
    connected_web_clients: usize,
    total_spend_usd: f64,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
struct AdminUserInfo {
    id: Uuid,
    email: String,
    name: Option<String>,
    #[allow(dead_code)]
    avatar_url: Option<String>,
    is_admin: bool,
    disabled: bool,
    created_at: String,
    session_count: i64,
    total_spend_usd: f64,
}

#[derive(Debug, Clone, Deserialize)]
struct AdminUsersResponse {
    users: Vec<AdminUserInfo>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
struct AdminSessionInfo {
    id: Uuid,
    #[allow(dead_code)]
    user_id: Uuid,
    user_email: String,
    session_name: String,
    working_directory: Option<String>,
    git_branch: Option<String>,
    status: String,
    total_cost_usd: f64,
    #[allow(dead_code)]
    created_at: String,
    last_activity: String,
    is_connected: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct AdminSessionsResponse {
    sessions: Vec<AdminSessionInfo>,
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Format a timestamp for display
fn format_timestamp(ts: &str) -> String {
    let date = js_sys::Date::new(&ts.into());
    if date.get_time().is_nan() {
        return ts.to_string();
    }
    format!(
        "{}-{:02}-{:02} {:02}:{:02}",
        date.get_full_year(),
        date.get_month() + 1,
        date.get_date(),
        date.get_hours(),
        date.get_minutes()
    )
}

// ============================================================================
// Stats Card Component
// ============================================================================

#[derive(Properties, PartialEq)]
struct StatCardProps {
    label: String,
    value: String,
    #[prop_or_default]
    subvalue: Option<String>,
    #[prop_or_default]
    class: Option<String>,
}

#[function_component(StatCard)]
fn stat_card(props: &StatCardProps) -> Html {
    let class = classes!("admin-stat-card", props.class.clone());
    html! {
        <div class={class}>
            <div class="stat-value">{ &props.value }</div>
            <div class="stat-label">{ &props.label }</div>
            {
                if let Some(ref sub) = props.subvalue {
                    html! { <div class="stat-subvalue">{ sub }</div> }
                } else {
                    html! {}
                }
            }
        </div>
    }
}

// ============================================================================
// User Row Component
// ============================================================================

#[derive(Properties, PartialEq)]
struct UserRowProps {
    user: AdminUserInfo,
    on_toggle_admin: Callback<Uuid>,
    on_toggle_disabled: Callback<Uuid>,
    current_user_id: Uuid,
}

#[function_component(UserRow)]
fn user_row(props: &UserRowProps) -> Html {
    let user = &props.user;
    let is_self = user.id == props.current_user_id;

    let on_toggle_admin = {
        let callback = props.on_toggle_admin.clone();
        let user_id = user.id;
        Callback::from(move |_: MouseEvent| callback.emit(user_id))
    };

    let on_toggle_disabled = {
        let callback = props.on_toggle_disabled.clone();
        let user_id = user.id;
        Callback::from(move |_: MouseEvent| callback.emit(user_id))
    };

    let status_class = if user.disabled {
        "user-status disabled"
    } else if user.is_admin {
        "user-status admin"
    } else {
        "user-status active"
    };

    let status_text = if user.disabled {
        "Disabled"
    } else if user.is_admin {
        "Admin"
    } else {
        "User"
    };

    html! {
        <tr class={classes!(if is_self { Some("self-row") } else { None })}>
            <td class="user-email">
                { &user.email }
                { if is_self { html! { <span class="you-badge">{ "(you)" }</span> } } else { html! {} } }
            </td>
            <td>{ user.name.as_deref().unwrap_or("-") }</td>
            <td class={status_class}>{ status_text }</td>
            <td class="numeric">{ user.session_count }</td>
            <td class="numeric">{ format!("${:.2}", user.total_spend_usd) }</td>
            <td class="timestamp">{ format_timestamp(&user.created_at) }</td>
            <td class="actions">
                <button
                    class={classes!("admin-toggle", if user.is_admin { Some("active") } else { None })}
                    onclick={on_toggle_admin}
                    disabled={is_self}
                    title={if is_self { "Cannot change your own admin status" } else if user.is_admin { "Remove admin" } else { "Make admin" }}
                >
                    { if user.is_admin { "Remove Admin" } else { "Make Admin" } }
                </button>
                <button
                    class={classes!("disable-toggle", if user.disabled { Some("active") } else { None })}
                    onclick={on_toggle_disabled}
                    disabled={is_self}
                    title={if is_self { "Cannot disable your own account" } else if user.disabled { "Enable user" } else { "Disable user" }}
                >
                    { if user.disabled { "Enable" } else { "Disable" } }
                </button>
            </td>
        </tr>
    }
}

// ============================================================================
// Session Row Component
// ============================================================================

#[derive(Properties, PartialEq)]
struct SessionRowProps {
    session: AdminSessionInfo,
    on_delete: Callback<Uuid>,
}

#[function_component(SessionRow)]
fn session_row(props: &SessionRowProps) -> Html {
    let session = &props.session;

    let on_delete = {
        let callback = props.on_delete.clone();
        let session_id = session.id;
        Callback::from(move |_: MouseEvent| callback.emit(session_id))
    };

    let status_class = if session.is_connected {
        "session-status connected"
    } else if session.status == "active" {
        "session-status active"
    } else {
        "session-status disconnected"
    };

    let status_text = if session.is_connected {
        "Connected"
    } else {
        &session.status
    };

    // Extract project name from working directory
    let project_name = session
        .working_directory
        .as_ref()
        .and_then(|dir| dir.split('/').next_back())
        .unwrap_or("-");

    html! {
        <tr>
            <td class="session-user">{ &session.user_email }</td>
            <td class="session-project">{ project_name }</td>
            <td class="session-branch">{ session.git_branch.as_deref().unwrap_or("-") }</td>
            <td class={status_class}>{ status_text }</td>
            <td class="numeric">{ format!("${:.2}", session.total_cost_usd) }</td>
            <td class="timestamp">{ format_timestamp(&session.last_activity) }</td>
            <td class="actions">
                <button class="delete-btn" onclick={on_delete} title="Delete session">
                    { "Delete" }
                </button>
            </td>
        </tr>
    }
}

// ============================================================================
// Main Admin Page Component
// ============================================================================

#[function_component(AdminPage)]
pub fn admin_page() -> Html {
    let active_tab = use_state(|| AdminTab::Overview);
    let stats = use_state(|| None::<AdminStats>);
    let users = use_state(Vec::<AdminUserInfo>::new);
    let sessions = use_state(Vec::<AdminSessionInfo>::new);
    let loading = use_state(|| true);
    let error = use_state(|| None::<String>);
    let current_user_id = use_state(|| None::<Uuid>);
    let confirm_action = use_state(|| None::<(String, Callback<MouseEvent>)>);

    let navigator = use_navigator().unwrap();

    // Fetch current user to get their ID
    {
        let current_user_id = current_user_id.clone();
        let error = error.clone();
        let navigator = navigator.clone();
        use_effect_with((), move |_| {
            spawn_local(async move {
                let api_endpoint = utils::api_url("/api/auth/me");
                match Request::get(&api_endpoint).send().await {
                    Ok(response) => {
                        if response.status() == 401 {
                            navigator.push(&Route::Home);
                            return;
                        }
                        if response.status() == 403 {
                            error.set(Some(
                                "Access denied. Admin privileges required.".to_string(),
                            ));
                            return;
                        }
                        if let Ok(data) = response.json::<serde_json::Value>().await {
                            if let Some(id) = data.get("id").and_then(|v| v.as_str()) {
                                if let Ok(uuid) = id.parse::<Uuid>() {
                                    current_user_id.set(Some(uuid));
                                }
                            }
                        }
                    }
                    Err(e) => {
                        error.set(Some(format!("Failed to fetch user: {:?}", e)));
                    }
                }
            });
            || ()
        });
    }

    // Fetch stats
    let fetch_stats = {
        let stats = stats.clone();
        let error = error.clone();
        let loading = loading.clone();
        let navigator = navigator.clone();
        Callback::from(move |_| {
            let stats = stats.clone();
            let error = error.clone();
            let loading = loading.clone();
            let navigator = navigator.clone();
            spawn_local(async move {
                let api_endpoint = utils::api_url("/api/admin/stats");
                match Request::get(&api_endpoint).send().await {
                    Ok(response) => {
                        if response.status() == 401 {
                            navigator.push(&Route::Home);
                            return;
                        }
                        if response.status() == 403 {
                            error.set(Some(
                                "Access denied. Admin privileges required.".to_string(),
                            ));
                            loading.set(false);
                            return;
                        }
                        match response.json::<AdminStats>().await {
                            Ok(data) => {
                                stats.set(Some(data));
                                error.set(None);
                            }
                            Err(e) => {
                                error.set(Some(format!("Failed to parse stats: {:?}", e)));
                            }
                        }
                    }
                    Err(e) => {
                        error.set(Some(format!("Failed to fetch stats: {:?}", e)));
                    }
                }
                loading.set(false);
            });
        })
    };

    // Fetch users
    let fetch_users = {
        let users = users.clone();
        let error = error.clone();
        Callback::from(move |_| {
            let users = users.clone();
            let error = error.clone();
            spawn_local(async move {
                let api_endpoint = utils::api_url("/api/admin/users");
                match Request::get(&api_endpoint).send().await {
                    Ok(response) => {
                        if response.status() == 403 {
                            return;
                        }
                        match response.json::<AdminUsersResponse>().await {
                            Ok(data) => {
                                users.set(data.users);
                            }
                            Err(e) => {
                                error.set(Some(format!("Failed to parse users: {:?}", e)));
                            }
                        }
                    }
                    Err(e) => {
                        error.set(Some(format!("Failed to fetch users: {:?}", e)));
                    }
                }
            });
        })
    };

    // Fetch sessions
    let fetch_sessions = {
        let sessions = sessions.clone();
        let error = error.clone();
        Callback::from(move |_| {
            let sessions = sessions.clone();
            let error = error.clone();
            spawn_local(async move {
                let api_endpoint = utils::api_url("/api/admin/sessions");
                match Request::get(&api_endpoint).send().await {
                    Ok(response) => {
                        if response.status() == 403 {
                            return;
                        }
                        match response.json::<AdminSessionsResponse>().await {
                            Ok(data) => {
                                sessions.set(data.sessions);
                            }
                            Err(e) => {
                                error.set(Some(format!("Failed to parse sessions: {:?}", e)));
                            }
                        }
                    }
                    Err(e) => {
                        error.set(Some(format!("Failed to fetch sessions: {:?}", e)));
                    }
                }
            });
        })
    };

    // Initial data fetch
    {
        let fetch_stats = fetch_stats.clone();
        let fetch_users = fetch_users.clone();
        let fetch_sessions = fetch_sessions.clone();
        use_effect_with((), move |_| {
            fetch_stats.emit(());
            fetch_users.emit(());
            fetch_sessions.emit(());
            || ()
        });
    }

    // Auto-refresh stats every 10 seconds
    {
        let fetch_stats = fetch_stats.clone();
        use_effect_with((), move |_| {
            let interval = gloo::timers::callback::Interval::new(10_000, move || {
                fetch_stats.emit(());
            });
            move || drop(interval)
        });
    }

    // Toggle admin handler
    let on_toggle_admin = {
        let users = users.clone();
        let confirm_action = confirm_action.clone();
        Callback::from(move |user_id: Uuid| {
            let users_inner = users.clone();
            let confirm_inner = confirm_action.clone();

            let target_user = users_inner.iter().find(|u| u.id == user_id).cloned();
            let is_currently_admin = target_user.as_ref().map(|u| u.is_admin).unwrap_or(false);
            let action_text = if is_currently_admin {
                "Remove admin privileges from this user?"
            } else {
                "Grant admin privileges to this user?"
            };

            let action = Callback::from(move |_: MouseEvent| {
                let users = users_inner.clone();
                let confirm = confirm_inner.clone();
                let new_admin_status = !is_currently_admin;
                spawn_local(async move {
                    let api_endpoint = utils::api_url(&format!("/api/admin/users/{}", user_id));
                    let body = serde_json::json!({ "is_admin": new_admin_status });
                    match Request::patch(&api_endpoint)
                        .header("Content-Type", "application/json")
                        .body(body.to_string())
                        .unwrap()
                        .send()
                        .await
                    {
                        Ok(response) => {
                            if response.status() == 204 {
                                let mut updated = (*users).clone();
                                if let Some(user) = updated.iter_mut().find(|u| u.id == user_id) {
                                    user.is_admin = new_admin_status;
                                }
                                users.set(updated);
                            }
                        }
                        Err(e) => {
                            log::error!("Failed to update user: {:?}", e);
                        }
                    }
                    confirm.set(None);
                });
            });

            confirm_action.set(Some((action_text.to_string(), action)));
        })
    };

    // Toggle disabled handler
    let on_toggle_disabled = {
        let users = users.clone();
        let confirm_action = confirm_action.clone();
        Callback::from(move |user_id: Uuid| {
            let users_inner = users.clone();
            let confirm_inner = confirm_action.clone();

            let target_user = users_inner.iter().find(|u| u.id == user_id).cloned();
            let is_currently_disabled = target_user.as_ref().map(|u| u.disabled).unwrap_or(false);
            let action_text = if is_currently_disabled {
                "Enable this user account?"
            } else {
                "Disable this user account? They will be unable to log in."
            };

            let action = Callback::from(move |_: MouseEvent| {
                let users = users_inner.clone();
                let confirm = confirm_inner.clone();
                let new_disabled_status = !is_currently_disabled;
                spawn_local(async move {
                    let api_endpoint = utils::api_url(&format!("/api/admin/users/{}", user_id));
                    let body = serde_json::json!({ "disabled": new_disabled_status });
                    match Request::patch(&api_endpoint)
                        .header("Content-Type", "application/json")
                        .body(body.to_string())
                        .unwrap()
                        .send()
                        .await
                    {
                        Ok(response) => {
                            if response.status() == 204 {
                                let mut updated = (*users).clone();
                                if let Some(user) = updated.iter_mut().find(|u| u.id == user_id) {
                                    user.disabled = new_disabled_status;
                                }
                                users.set(updated);
                            }
                        }
                        Err(e) => {
                            log::error!("Failed to update user: {:?}", e);
                        }
                    }
                    confirm.set(None);
                });
            });

            confirm_action.set(Some((action_text.to_string(), action)));
        })
    };

    // Delete session handler
    let on_delete_session = {
        let sessions = sessions.clone();
        let confirm_action = confirm_action.clone();
        let fetch_stats = fetch_stats.clone();
        Callback::from(move |session_id: Uuid| {
            let sessions_inner = sessions.clone();
            let confirm_inner = confirm_action.clone();
            let fetch_stats = fetch_stats.clone();

            let action = Callback::from(move |_: MouseEvent| {
                let sessions = sessions_inner.clone();
                let confirm = confirm_inner.clone();
                let fetch_stats = fetch_stats.clone();
                spawn_local(async move {
                    let api_endpoint =
                        utils::api_url(&format!("/api/admin/sessions/{}", session_id));
                    match Request::delete(&api_endpoint).send().await {
                        Ok(response) => {
                            if response.status() == 204 {
                                let updated: Vec<_> = (*sessions)
                                    .iter()
                                    .filter(|s| s.id != session_id)
                                    .cloned()
                                    .collect();
                                sessions.set(updated);
                                fetch_stats.emit(());
                            }
                        }
                        Err(e) => {
                            log::error!("Failed to delete session: {:?}", e);
                        }
                    }
                    confirm.set(None);
                });
            });

            confirm_action.set(Some((
                "Delete this session? All message history will be lost.".to_string(),
                action,
            )));
        })
    };

    // Tab click handlers
    let on_overview_tab = {
        let active_tab = active_tab.clone();
        Callback::from(move |_| active_tab.set(AdminTab::Overview))
    };
    let on_users_tab = {
        let active_tab = active_tab.clone();
        Callback::from(move |_| active_tab.set(AdminTab::Users))
    };
    let on_sessions_tab = {
        let active_tab = active_tab.clone();
        Callback::from(move |_| active_tab.set(AdminTab::Sessions))
    };

    // Cancel confirmation
    let on_cancel_confirm = {
        let confirm_action = confirm_action.clone();
        Callback::from(move |_| confirm_action.set(None))
    };

    // Back to dashboard
    let go_back = {
        let navigator = navigator.clone();
        Callback::from(move |_| navigator.push(&Route::Dashboard))
    };

    html! {
        <div class="admin-container">
            <header class="admin-header">
                <button class="back-button" onclick={go_back}>
                    { "< Back to Dashboard" }
                </button>
                <h1>{ "Admin Dashboard" }</h1>
            </header>

            {
                if let Some(ref err) = *error {
                    html! {
                        <div class="admin-error">
                            { err }
                        </div>
                    }
                } else {
                    html! {}
                }
            }

            {
                if *loading {
                    html! {
                        <div class="admin-loading">
                            <div class="spinner"></div>
                            <p>{ "Loading admin data..." }</p>
                        </div>
                    }
                } else {
                    html! {
                        <>
                            <nav class="admin-tabs">
                                <button
                                    class={classes!("tab-btn", if *active_tab == AdminTab::Overview { Some("active") } else { None })}
                                    onclick={on_overview_tab}
                                >
                                    { "Overview" }
                                </button>
                                <button
                                    class={classes!("tab-btn", if *active_tab == AdminTab::Users { Some("active") } else { None })}
                                    onclick={on_users_tab}
                                >
                                    { format!("Users ({})", users.len()) }
                                </button>
                                <button
                                    class={classes!("tab-btn", if *active_tab == AdminTab::Sessions { Some("active") } else { None })}
                                    onclick={on_sessions_tab}
                                >
                                    { format!("Sessions ({})", sessions.len()) }
                                </button>
                            </nav>

                            <div class="admin-content">
                                {
                                    match *active_tab {
                                        AdminTab::Overview => {
                                            if let Some(ref s) = *stats {
                                                html! {
                                                    <div class="admin-overview">
                                                        <div class="stats-grid">
                                                            <StatCard
                                                                label="Total Users"
                                                                value={s.total_users.to_string()}
                                                                subvalue={Some(format!("{} admins, {} disabled", s.admin_users, s.disabled_users))}
                                                            />
                                                            <StatCard
                                                                label="Total Sessions"
                                                                value={s.total_sessions.to_string()}
                                                                subvalue={Some(format!("{} active", s.active_sessions))}
                                                            />
                                                            <StatCard
                                                                label="Connected Clients"
                                                                value={format!("{}", s.connected_proxy_clients + s.connected_web_clients)}
                                                                subvalue={Some(format!("{} proxy, {} web", s.connected_proxy_clients, s.connected_web_clients))}
                                                            />
                                                            <StatCard
                                                                label="Total API Spend"
                                                                value={format!("${:.2}", s.total_spend_usd)}
                                                                class="spend-card"
                                                            />
                                                        </div>
                                                    </div>
                                                }
                                            } else {
                                                html! { <p>{ "No stats available" }</p> }
                                            }
                                        }
                                        AdminTab::Users => {
                                            html! {
                                                <div class="admin-users">
                                                    <table class="admin-table">
                                                        <thead>
                                                            <tr>
                                                                <th>{ "Email" }</th>
                                                                <th>{ "Name" }</th>
                                                                <th>{ "Status" }</th>
                                                                <th>{ "Sessions" }</th>
                                                                <th>{ "Spend" }</th>
                                                                <th>{ "Created" }</th>
                                                                <th>{ "Actions" }</th>
                                                            </tr>
                                                        </thead>
                                                        <tbody>
                                                            {
                                                                users.iter().map(|user| {
                                                                    html! {
                                                                        <UserRow
                                                                            key={user.id.to_string()}
                                                                            user={user.clone()}
                                                                            on_toggle_admin={on_toggle_admin.clone()}
                                                                            on_toggle_disabled={on_toggle_disabled.clone()}
                                                                            current_user_id={current_user_id.unwrap_or_default()}
                                                                        />
                                                                    }
                                                                }).collect::<Html>()
                                                            }
                                                        </tbody>
                                                    </table>
                                                </div>
                                            }
                                        }
                                        AdminTab::Sessions => {
                                            html! {
                                                <div class="admin-sessions">
                                                    <table class="admin-table">
                                                        <thead>
                                                            <tr>
                                                                <th>{ "User" }</th>
                                                                <th>{ "Project" }</th>
                                                                <th>{ "Branch" }</th>
                                                                <th>{ "Status" }</th>
                                                                <th>{ "Cost" }</th>
                                                                <th>{ "Last Activity" }</th>
                                                                <th>{ "Actions" }</th>
                                                            </tr>
                                                        </thead>
                                                        <tbody>
                                                            {
                                                                sessions.iter().map(|session| {
                                                                    html! {
                                                                        <SessionRow
                                                                            key={session.id.to_string()}
                                                                            session={session.clone()}
                                                                            on_delete={on_delete_session.clone()}
                                                                        />
                                                                    }
                                                                }).collect::<Html>()
                                                            }
                                                        </tbody>
                                                    </table>
                                                </div>
                                            }
                                        }
                                    }
                                }
                            </div>
                        </>
                    }
                }
            }

            // Confirmation modal
            {
                if let Some((ref message, ref action)) = *confirm_action {
                    html! {
                        <div class="modal-overlay" onclick={on_cancel_confirm.clone()}>
                            <div class="modal-content confirm-modal" onclick={Callback::from(|e: MouseEvent| e.stop_propagation())}>
                                <p>{ message }</p>
                                <div class="modal-actions">
                                    <button class="modal-cancel" onclick={on_cancel_confirm.clone()}>{ "Cancel" }</button>
                                    <button class="modal-confirm" onclick={action.clone()}>{ "Confirm" }</button>
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
