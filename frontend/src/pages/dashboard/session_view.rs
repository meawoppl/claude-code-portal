//! SessionView component - Main terminal view for a single session

use crate::components::{group_messages, MessageGroupRenderer, VoiceInput};
use crate::utils;
use futures_util::{SinkExt, StreamExt};
use gloo::timers::callback::Timeout;
use gloo_net::http::Request;
use gloo_net::websocket::{futures::WebSocket, Message};
use shared::{ProxyMessage, SessionInfo};
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use uuid::Uuid;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;
use web_sys::{Element, HtmlInputElement, KeyboardEvent};
use yew::prelude::*;

use super::types::{
    calculate_backoff, format_permission_input, parse_ask_user_question, MessagesResponse,
    PendingPermission, WsSender, MAX_MESSAGES_PER_SESSION,
};

/// Props for the SessionView component
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

/// Messages for the SessionView component
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

/// SessionView - Main terminal view for a single session
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
                        client_version: None, // Web client, not proxy
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
            SessionViewMsg::LoadHistory(mut messages, last_timestamp) => {
                // Truncate to keep only the last MAX_MESSAGES_PER_SESSION messages
                if messages.len() > MAX_MESSAGES_PER_SESSION {
                    let excess = messages.len() - MAX_MESSAGES_PER_SESSION;
                    messages.drain(0..excess);
                }
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
                // Truncate to keep only the last MAX_MESSAGES_PER_SESSION messages
                if self.messages.len() > MAX_MESSAGES_PER_SESSION {
                    let excess = self.messages.len() - MAX_MESSAGES_PER_SESSION;
                    self.messages.drain(0..excess);
                }
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
                if let Some(ref perm) = self.pending_permission {
                    // Calculate max based on whether this is an AskUserQuestion or regular permission
                    let max = if perm.tool_name == "AskUserQuestion" {
                        // For questions, max is based on number of options
                        if let Some(parsed) = parse_ask_user_question(&perm.input) {
                            parsed
                                .questions
                                .first()
                                .map(|q| q.options.len().saturating_sub(1))
                                .unwrap_or(0)
                        } else {
                            0
                        }
                    } else if !perm.permission_suggestions.is_empty() {
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
                if let Some(ref perm) = self.pending_permission {
                    // Calculate max based on whether this is an AskUserQuestion or regular permission
                    let max = if perm.tool_name == "AskUserQuestion" {
                        // For questions, max is based on number of options
                        if let Some(parsed) = parse_ask_user_question(&perm.input) {
                            parsed
                                .questions
                                .first()
                                .map(|q| q.options.len().saturating_sub(1))
                                .unwrap_or(0)
                        } else {
                            0
                        }
                    } else if !perm.permission_suggestions.is_empty() {
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
                                client_version: None, // Web client, not proxy
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
