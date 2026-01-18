//! SessionRail component - Horizontal carousel of session pills

use crate::utils;
use shared::SessionInfo;
use std::collections::{HashMap, HashSet};
use uuid::Uuid;
use web_sys::Element;
use yew::prelude::*;

/// Props for the SessionRail component
#[derive(Properties, PartialEq)]
pub struct SessionRailProps {
    pub sessions: Vec<SessionInfo>,
    pub focused_index: usize,
    pub awaiting_sessions: HashSet<Uuid>,
    pub paused_sessions: HashSet<Uuid>,
    pub inactive_hidden: bool,
    pub session_costs: HashMap<Uuid, f64>,
    pub connected_sessions: HashSet<Uuid>,
    pub nav_mode: bool,
    pub on_select: Callback<usize>,
    pub on_leave: Callback<Uuid>,
    pub on_toggle_pause: Callback<Uuid>,
    pub on_toggle_inactive_hidden: Callback<MouseEvent>,
}

/// SessionRail - Horizontal carousel of session pills
#[function_component(SessionRail)]
pub fn session_rail(props: &SessionRailProps) -> Html {
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

    // Helper to render a single session pill
    // index: position in full sessions array (for selection)
    // display_number: visible position for nav mode numbering (None = no number shown)
    let render_pill = |index: usize,
                       session: &SessionInfo,
                       display_number: Option<usize>|
     -> Html {
        let is_focused = index == props.focused_index;
        let is_awaiting = props.awaiting_sessions.contains(&session.id);
        let is_paused = props.paused_sessions.contains(&session.id);
        let is_connected = props.connected_sessions.contains(&session.id);
        let cost = props.session_costs.get(&session.id).copied().unwrap_or(0.0);

        let on_click = {
            let on_select = props.on_select.clone();
            Callback::from(move |_| on_select.emit(index))
        };

        let on_pause = {
            let on_toggle_pause = props.on_toggle_pause.clone();
            let session_id = session.id;
            Callback::from(move |e: MouseEvent| {
                e.stop_propagation();
                on_toggle_pause.emit(session_id);
            })
        };

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
            if is_status_disconnected {
                Some("status-disconnected")
            } else {
                None
            },
        );

        let hostname = utils::extract_hostname(&session.session_name);
        let folder = utils::extract_folder(&session.working_directory);

        let connection_class = if is_connected {
            "pill-status connected"
        } else {
            "pill-status disconnected"
        };

        // Show number annotation only in nav mode (1-9) for visible sessions
        let number_annotation = if in_nav_mode {
            display_number
                .filter(|&n| n < 9)
                .map(|n| format!("{}", n + 1))
        } else {
            None
        };

        html! {
            <div class={pill_class} onclick={on_click} key={session.id.to_string()}>
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
                    <span class="pill-folder">{ folder }</span>
                    <span class="pill-hostname">{ hostname }</span>
                    {
                        if let Some(ref branch) = session.git_branch {
                            html! { <span class="pill-branch">{ branch }</span> }
                        } else {
                            html! {}
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
                // Leave button for non-owners (delete is in Settings)
                {
                    if session.my_role != "owner" {
                        html! {
                            <button class="pill-leave" onclick={on_leave} title="Leave session">{ "↩" }</button>
                        }
                    } else {
                        html! {}
                    }
                }
            </div>
        }
    };

    // Split sessions into active (connected and not paused) vs inactive (disconnected or paused)
    let (active_indices, inactive_indices): (Vec<_>, Vec<_>) =
        props.sessions.iter().enumerate().partition(|(_, session)| {
            let is_connected = props.connected_sessions.contains(&session.id);
            let is_paused = props.paused_sessions.contains(&session.id);
            is_connected && !is_paused
        });

    let inactive_count = inactive_indices.len();

    // Calculate display numbers for visible sessions
    // When inactive is hidden, only active sessions get numbers
    // When inactive is shown, all sessions get numbers in display order
    let active_count = active_indices.len();

    html! {
        <div class="session-rail" ref={rail_ref}>
            // Active sessions - always get numbers starting from 0
            { active_indices.iter().enumerate().map(|(display_idx, (index, session))| {
                render_pill(*index, session, Some(display_idx))
            }).collect::<Html>() }

            // Divider (only show if there are inactive sessions)
            {
                if inactive_count > 0 {
                    let toggle_class = classes!(
                        "session-rail-divider",
                        if props.inactive_hidden { Some("collapsed") } else { None }
                    );
                    html! {
                        <div class={toggle_class} onclick={props.on_toggle_inactive_hidden.clone()}>
                            <span class="divider-line"></span>
                            <button class="divider-toggle" title={if props.inactive_hidden { "Show inactive sessions" } else { "Hide inactive sessions" }}>
                                { if props.inactive_hidden {
                                    format!("▶ {}", inactive_count)
                                } else {
                                    "◀".to_string()
                                }}
                            </button>
                        </div>
                    }
                } else {
                    html! {}
                }
            }

            // Inactive sessions (hidden when collapsed)
            // When shown, continue numbering from where active sessions left off
            {
                if !props.inactive_hidden {
                    inactive_indices.iter().enumerate().map(|(display_idx, (index, session))| {
                        render_pill(*index, session, Some(active_count + display_idx))
                    }).collect::<Html>()
                } else {
                    html! {}
                }
            }
        </div>
    }
}
