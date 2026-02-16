//! SessionRail component - Horizontal carousel of session pills

use crate::utils;
use shared::SessionInfo;
use std::collections::HashSet;
use uuid::Uuid;
use web_sys::{Element, HtmlElement, WheelEvent};
use yew::prelude::*;

/// Props for the SessionRail component
#[derive(Properties, PartialEq)]
pub struct SessionRailProps {
    pub sessions: Vec<SessionInfo>,
    pub focused_index: usize,
    pub awaiting_sessions: HashSet<Uuid>,
    pub paused_sessions: HashSet<Uuid>,
    pub inactive_hidden: bool,
    pub connected_sessions: HashSet<Uuid>,
    pub nav_mode: bool,
    pub on_select: Callback<usize>,
    pub on_leave: Callback<Uuid>,
    pub on_toggle_pause: Callback<Uuid>,
    pub on_toggle_inactive_hidden: Callback<MouseEvent>,
    pub on_stop: Callback<Uuid>,
}

/// SessionRail - Horizontal carousel of session pills
#[function_component(SessionRail)]
pub fn session_rail(props: &SessionRailProps) -> Html {
    let rail_ref = use_node_ref();
    let context_menu_session = use_state(|| None::<Uuid>);

    // Scroll focused session into view
    {
        let rail_ref = rail_ref.clone();
        let focused_index = props.focused_index;
        use_effect_with(focused_index, move |_| {
            if let Some(rail) = rail_ref.cast::<Element>() {
                if let Some(child) = rail.children().item(focused_index as u32) {
                    child.scroll_into_view();
                }
            }
            || ()
        });
    }

    // Close context menu on any click outside
    {
        let context_menu_session = context_menu_session.clone();
        let is_open = (*context_menu_session).is_some();
        use_effect_with(is_open, move |is_open| {
            let listener = if *is_open {
                Some(gloo::events::EventListener::new(
                    &gloo::utils::document(),
                    "click",
                    move |_| {
                        context_menu_session.set(None);
                    },
                ))
            } else {
                None
            };
            move || drop(listener)
        });
    }

    // Handle wheel event to translate vertical scroll to horizontal
    let on_wheel = {
        let rail_ref = rail_ref.clone();
        Callback::from(move |e: WheelEvent| {
            if let Some(rail) = rail_ref.cast::<HtmlElement>() {
                e.prevent_default();
                let delta = e.delta_y();
                rail.set_scroll_left(rail.scroll_left() + (delta * 3.0) as i32);
            }
        })
    };

    // Helper to render a single session pill
    let render_pill = |index: usize,
                       session: &SessionInfo,
                       display_number: Option<usize>|
     -> Html {
        let is_focused = index == props.focused_index;
        let is_awaiting = props.awaiting_sessions.contains(&session.id);
        let is_paused = props.paused_sessions.contains(&session.id);
        let is_connected = props.connected_sessions.contains(&session.id);
        let is_menu_open = *context_menu_session == Some(session.id);

        let on_click = {
            let on_select = props.on_select.clone();
            Callback::from(move |_| on_select.emit(index))
        };

        let on_toggle_menu = {
            let context_menu_session = context_menu_session.clone();
            let session_id = session.id;
            Callback::from(move |e: MouseEvent| {
                e.stop_propagation();
                context_menu_session.set(Some(session_id));
            })
        };

        let on_pause = {
            let on_toggle_pause = props.on_toggle_pause.clone();
            let session_id = session.id;
            let context_menu_session = context_menu_session.clone();
            Callback::from(move |e: MouseEvent| {
                e.stop_propagation();
                on_toggle_pause.emit(session_id);
                context_menu_session.set(None);
            })
        };

        let on_leave = {
            let on_leave = props.on_leave.clone();
            let session_id = session.id;
            let context_menu_session = context_menu_session.clone();
            Callback::from(move |e: MouseEvent| {
                e.stop_propagation();
                on_leave.emit(session_id);
                context_menu_session.set(None);
            })
        };

        let on_stop = {
            let on_stop = props.on_stop.clone();
            let session_id = session.id;
            let context_menu_session = context_menu_session.clone();
            Callback::from(move |e: MouseEvent| {
                e.stop_propagation();
                on_stop.emit(session_id);
                context_menu_session.set(None);
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

        let number_annotation = if in_nav_mode {
            display_number
                .filter(|&n| n < 9)
                .map(|n| format!("{}", n + 1))
        } else {
            None
        };

        // Pre-compute context menu HTML
        let context_menu_html = if is_menu_open {
            let stop_option = if is_connected && session.status == shared::SessionStatus::Active {
                html! {
                    <button type="button" class="context-menu-option stop" onclick={on_stop}>
                        { "Stop Session" }
                        <span class="option-hint">{ "Terminate process" }</span>
                    </button>
                }
            } else {
                html! {}
            };

            let pause_label = if is_paused {
                "Unpause Session"
            } else {
                "Pause Session"
            };
            let pause_hint = if is_paused {
                "Resume rotation"
            } else {
                "Skip in rotation"
            };

            let leave_option = if session.my_role != "owner" {
                html! {
                    <button type="button" class="context-menu-option leave" onclick={on_leave}>
                        { "Leave Session" }
                        <span class="option-hint">{ "Remove from your list" }</span>
                    </button>
                }
            } else {
                html! {}
            };

            html! {
                <div class="pill-context-menu" onclick={Callback::from(|e: MouseEvent| e.stop_propagation())}>
                    { stop_option }
                    <button
                        type="button"
                        class={classes!("context-menu-option", "pause", is_paused.then_some("active"))}
                        onclick={on_pause}
                    >
                        { pause_label }
                        <span class="option-hint">{ pause_hint }</span>
                    </button>
                    { leave_option }
                </div>
            }
        } else {
            html! {}
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
                            html! { <span class="pill-branch" title={branch.clone()}>{ branch }</span> }
                        } else {
                            html! {}
                        }
                    }
                </span>
                {
                    if is_paused {
                        html! { <span class="pill-paused-badge">{ "ᴾ" }</span> }
                    } else {
                        html! {}
                    }
                }
                {
                    if session.my_role != "owner" {
                        let role_class = format!("pill-role-badge role-{}", session.my_role);
                        html! { <span class={role_class}>{ &session.my_role }</span> }
                    } else {
                        html! {}
                    }
                }
                <button type="button" class="pill-menu-toggle" onclick={on_toggle_menu}>
                    { "▼" }
                </button>
                { context_menu_html }
            </div>
        }
    };

    // Split sessions into visible (not paused) vs hidden (paused only)
    let (visible_indices, paused_indices): (Vec<_>, Vec<_>) =
        props.sessions.iter().enumerate().partition(|(_, session)| {
            let is_paused = props.paused_sessions.contains(&session.id);
            !is_paused
        });

    let paused_count = paused_indices.len();
    let visible_count = visible_indices.len();

    html! {
        <div class="session-rail" ref={rail_ref} onwheel={on_wheel}>
            { visible_indices.iter().enumerate().map(|(display_idx, (index, session))| {
                render_pill(*index, session, Some(display_idx))
            }).collect::<Html>() }

            {
                if paused_count > 0 {
                    let toggle_class = classes!(
                        "session-rail-divider",
                        if props.inactive_hidden { Some("collapsed") } else { None }
                    );
                    html! {
                        <div class={toggle_class} onclick={props.on_toggle_inactive_hidden.clone()}>
                            <span class="divider-line"></span>
                            <button class="divider-toggle" title={if props.inactive_hidden { "Show paused sessions" } else { "Hide paused sessions" }}>
                                { if props.inactive_hidden {
                                    format!("▶ {}", paused_count)
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

            {
                if !props.inactive_hidden {
                    paused_indices.iter().enumerate().map(|(display_idx, (index, session))| {
                        render_pill(*index, session, Some(visible_count + display_idx))
                    }).collect::<Html>()
                } else {
                    html! {}
                }
            }
        </div>
    }
}
