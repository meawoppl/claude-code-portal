//! SessionRail component - Horizontal carousel of session pills

use crate::utils;
use shared::SessionInfo;
use std::collections::HashSet;
use uuid::Uuid;
use wasm_bindgen::JsCast;
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
    let menu_session = use_state(|| None::<Uuid>);
    let menu_pos = use_state(|| (0i32, 0i32));

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

    // Find the session whose menu is open (for rendering the floating dropdown)
    let open_session: Option<&SessionInfo> =
        (*menu_session).and_then(|id| props.sessions.iter().find(|s| s.id == id));

    // Pre-compute the floating dropdown menu (rendered outside the rail)
    let floating_menu = if let Some(session) = open_session {
        let is_paused = props.paused_sessions.contains(&session.id);
        let is_connected = props.connected_sessions.contains(&session.id);

        let on_stop = {
            let on_stop = props.on_stop.clone();
            let session_id = session.id;
            let menu_session = menu_session.clone();
            Callback::from(move |e: MouseEvent| {
                e.stop_propagation();
                on_stop.emit(session_id);
                menu_session.set(None);
            })
        };

        let on_pause = {
            let on_toggle_pause = props.on_toggle_pause.clone();
            let session_id = session.id;
            let menu_session = menu_session.clone();
            Callback::from(move |e: MouseEvent| {
                e.stop_propagation();
                on_toggle_pause.emit(session_id);
                menu_session.set(None);
            })
        };

        let on_leave = {
            let on_leave = props.on_leave.clone();
            let session_id = session.id;
            let menu_session = menu_session.clone();
            Callback::from(move |e: MouseEvent| {
                e.stop_propagation();
                on_leave.emit(session_id);
                menu_session.set(None);
            })
        };

        let stop_option = if is_connected && session.status == shared::SessionStatus::Active {
            html! {
                <button type="button" class="pill-menu-option stop" onclick={on_stop}>
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
                <button type="button" class="pill-menu-option leave" onclick={on_leave}>
                    { "Leave Session" }
                    <span class="option-hint">{ "Remove from your list" }</span>
                </button>
            }
        } else {
            html! {}
        };

        let (left, top) = *menu_pos;
        let style = format!("left: {}px; top: {}px;", left, top);

        let on_close_overlay = {
            let menu_session = menu_session.clone();
            Callback::from(move |_: MouseEvent| {
                menu_session.set(None);
            })
        };

        html! {
            <>
                <div class="pill-dropdown-overlay" onclick={on_close_overlay} />
                <div class="pill-dropdown" {style}>
                    { stop_option }
                    <button
                        type="button"
                        class={classes!("pill-menu-option", "pause", is_paused.then_some("active"))}
                        onclick={on_pause}
                    >
                        { pause_label }
                        <span class="option-hint">{ pause_hint }</span>
                    </button>
                    { leave_option }
                </div>
            </>
        }
    } else {
        html! {}
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

        let on_click = {
            let on_select = props.on_select.clone();
            Callback::from(move |_| on_select.emit(index))
        };

        let on_toggle_menu = {
            let menu_session = menu_session.clone();
            let menu_pos = menu_pos.clone();
            let session_id = session.id;
            Callback::from(move |e: MouseEvent| {
                e.stop_propagation();
                // Toggle: if already open for this session, close it
                if *menu_session == Some(session_id) {
                    menu_session.set(None);
                    return;
                }
                // Compute position from the clicked button
                if let Some(el) = e.target_dyn_into::<HtmlElement>() {
                    let rect = el.get_bounding_client_rect();
                    menu_pos.set((rect.left() as i32, rect.bottom() as i32 + 4));
                }
                menu_session.set(Some(session_id));
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
        <>
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
            { floating_menu }
        </>
    }
}
