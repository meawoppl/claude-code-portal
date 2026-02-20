//! SessionView component - Main terminal view for a single session

use crate::components::{group_messages, MessageGroupRenderer, VoiceInput};
use crate::utils;
use gloo::timers::callback::Timeout;
use gloo_net::http::Request;
use shared::api::{ErrorMessage, PermissionAnswers};
use shared::{ClientToServer, SendMode, SessionInfo};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use uuid::Uuid;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;
use web_sys::{Element, HtmlTextAreaElement, KeyboardEvent};
use yew::prelude::*;

use super::history::CommandHistory;
use super::types::{PendingPermission, QuestionAnswers, WsSender, MAX_MESSAGES_PER_SESSION};
use super::websocket::{connect_websocket, send_message, WsEvent};
use crate::pages::dashboard::permission_dialog::PermissionDialog;
use crate::pages::dashboard::types::{
    calculate_backoff, parse_ask_user_question, MessagesResponse,
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
    pub on_branch_change: Callback<(Uuid, Option<String>, Option<String>)>,
    #[prop_or(false)]
    pub voice_enabled: bool,
}

/// Messages for the SessionView component
pub enum SessionViewMsg {
    SendInput,
    UpdateInput(String),
    LoadHistory(Vec<String>, Option<String>),
    ReceivedOutput(String),
    WebSocketConnected(WsSender),
    WebSocketError(String),
    AttemptReconnect,
    CheckAwaiting,
    ClearCostFlash,
    PermissionRequest(PendingPermission),
    ApprovePermission,
    ApprovePermissionAndRemember,
    DenyPermission,
    PermissionSelectUp,
    PermissionSelectDown,
    BranchChanged(Option<String>, Option<String>),
    PermissionConfirm,
    PermissionSelectAndConfirm(usize),
    HistoryUp,
    HistoryDown,
    VoiceRecordingChanged(bool),
    VoiceTranscription(String),
    VoiceInterimTranscription(String),
    VoiceError(String),
    ToggleVoice,
    SetQuestionAnswer(usize, String),
    ToggleQuestionOption(usize, usize),
    SubmitAllAnswers(QuestionAnswers),
    /// Handle WebSocket event from connection
    WsEvent(WsEvent),
    /// Toggle send mode dropdown visibility
    ToggleSendModeDropdown,
    /// Close send mode dropdown (click outside)
    CloseSendModeDropdown,
    /// Send with wiggum mode
    SendWiggum,
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
    reconnect_attempt: u32,
    #[allow(dead_code)]
    reconnect_timer: Option<Timeout>,
    command_history: CommandHistory,
    is_recording: bool,
    interim_transcription: Option<String>,
    last_message_timestamp: Option<String>,
    voice_button_ref: NodeRef,
    multi_select_options: HashMap<usize, HashSet<usize>>,
    question_answers: QuestionAnswers,
    send_mode_dropdown_open: bool,
}

impl Component for SessionView {
    type Message = SessionViewMsg;
    type Properties = SessionViewProps;

    fn create(ctx: &Context<Self>) -> Self {
        let link = ctx.link().clone();
        let session_id = ctx.props().session.id;
        let on_awaiting_change = ctx.props().on_awaiting_change.clone();

        // Fetch existing messages via REST, then connect WebSocket
        spawn_local(async move {
            let mut last_message_time: Option<String> = None;
            let api_endpoint = utils::api_url(&format!("/api/sessions/{}/messages", session_id));

            if let Ok(response) = Request::get(&api_endpoint).send().await {
                if let Ok(data) = response.json::<MessagesResponse>().await {
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

                    last_message_time = data.messages.last().map(|m| m.created_at.clone());

                    let messages: Vec<String> =
                        data.messages.into_iter().map(|m| m.content).collect();
                    link.send_message(SessionViewMsg::LoadHistory(
                        messages,
                        last_message_time.clone(),
                    ));
                }
            }

            // Connect WebSocket with event callback
            let ws_link = link.clone();
            let on_event = Callback::from(move |event: WsEvent| {
                ws_link.send_message(SessionViewMsg::WsEvent(event));
            });
            connect_websocket(session_id, last_message_time, false, on_event);
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
            command_history: CommandHistory::for_session(ctx.props().session.id),
            is_recording: false,
            interim_transcription: None,
            last_message_timestamp: None,
            voice_button_ref: NodeRef::default(),
            multi_select_options: HashMap::new(),
            question_answers: HashMap::new(),
            send_mode_dropdown_open: false,
        }
    }

    fn changed(&mut self, ctx: &Context<Self>, _old_props: &Self::Properties) -> bool {
        let now_focused = ctx.props().focused;
        let became_focused = now_focused && !self.was_focused;
        self.was_focused = now_focused;

        if became_focused {
            if let Some(input) = self.input_ref.cast::<HtmlTextAreaElement>() {
                let _ = input.focus();
            }
        }

        true
    }

    fn rendered(&mut self, ctx: &Context<Self>, first_render: bool) {
        if first_render && ctx.props().focused {
            if let Some(input) = self.input_ref.cast::<HtmlTextAreaElement>() {
                let _ = input.focus();
            }
        }

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
            SessionViewMsg::WsEvent(event) => self.handle_ws_event(ctx, event),
            SessionViewMsg::UpdateInput(value) => {
                self.input_value = value;
                true
            }
            SessionViewMsg::SendInput => self.handle_send_input_with_mode(ctx, SendMode::Normal),
            SessionViewMsg::LoadHistory(mut messages, last_timestamp) => {
                if messages.len() > MAX_MESSAGES_PER_SESSION {
                    let excess = messages.len() - MAX_MESSAGES_PER_SESSION;
                    messages.drain(0..excess);
                }
                self.messages = messages;
                self.last_message_timestamp = last_timestamp;
                ctx.link().send_message(SessionViewMsg::CheckAwaiting);
                true
            }
            SessionViewMsg::ReceivedOutput(output) => self.handle_received_output(ctx, output),
            SessionViewMsg::ClearCostFlash => {
                self.cost_flash = false;
                true
            }
            SessionViewMsg::PermissionRequest(perm) => {
                self.pending_permission = Some(perm);
                self.permission_selected = 0;
                self.question_answers.clear();
                self.multi_select_options.clear();
                let session_id = ctx.props().session.id;
                ctx.props().on_awaiting_change.emit((session_id, true));
                if let Some(el) = self.permission_ref.cast::<web_sys::HtmlElement>() {
                    let _ = el.focus();
                }
                true
            }
            SessionViewMsg::PermissionSelectUp => self.handle_permission_select(-1),
            SessionViewMsg::PermissionSelectDown => self.handle_permission_select(1),
            SessionViewMsg::PermissionConfirm => self.handle_permission_confirm(ctx),
            SessionViewMsg::PermissionSelectAndConfirm(index) => {
                self.permission_selected = index;
                ctx.link().send_message(SessionViewMsg::PermissionConfirm);
                false
            }
            SessionViewMsg::ApprovePermission => self.handle_approve_permission(ctx, false),
            SessionViewMsg::ApprovePermissionAndRemember => {
                self.handle_approve_permission(ctx, true)
            }
            SessionViewMsg::DenyPermission => self.handle_deny_permission(ctx),
            SessionViewMsg::WebSocketConnected(sender) => {
                self.ws_connected = true;
                self.ws_sender = Some(sender);
                self.reconnect_attempt = 0;
                self.reconnect_timer = None;
                let session_id = ctx.props().session.id;
                ctx.props().on_connected_change.emit((session_id, true));
                true
            }
            SessionViewMsg::WebSocketError(err) => self.handle_ws_error(ctx, err),
            SessionViewMsg::AttemptReconnect => {
                self.attempt_reconnect(ctx);
                false
            }
            SessionViewMsg::CheckAwaiting => {
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
                let is_awaiting = is_result_awaiting || self.pending_permission.is_some();
                let session_id = ctx.props().session.id;
                ctx.props()
                    .on_awaiting_change
                    .emit((session_id, is_awaiting));
                false
            }
            SessionViewMsg::BranchChanged(branch, pr_url) => {
                let session_id = ctx.props().session.id;
                ctx.props()
                    .on_branch_change
                    .emit((session_id, branch, pr_url));
                false
            }
            SessionViewMsg::HistoryUp => {
                if let Some(cmd) = self.command_history.navigate_up(&self.input_value) {
                    self.input_value = cmd;
                    true
                } else {
                    false
                }
            }
            SessionViewMsg::HistoryDown => {
                if let Some(cmd) = self.command_history.navigate_down() {
                    self.input_value = cmd;
                    true
                } else {
                    false
                }
            }
            SessionViewMsg::VoiceRecordingChanged(recording) => {
                self.is_recording = recording;
                if !recording {
                    self.interim_transcription = None;
                }
                true
            }
            SessionViewMsg::VoiceTranscription(text) => {
                self.interim_transcription = None;
                if !text.is_empty() {
                    if self.input_value.is_empty() {
                        self.input_value = text;
                    } else {
                        self.input_value.push(' ');
                        self.input_value.push_str(&text);
                    }
                    ctx.link().send_message(SessionViewMsg::SendInput);
                }
                true
            }
            SessionViewMsg::VoiceInterimTranscription(text) => {
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
                if let Some(button) = self.voice_button_ref.cast::<web_sys::HtmlElement>() {
                    button.click();
                }
                false
            }
            SessionViewMsg::SetQuestionAnswer(question_idx, answer) => {
                self.question_answers.insert(question_idx, answer);
                self.multi_select_options.remove(&question_idx);
                true
            }
            SessionViewMsg::ToggleQuestionOption(question_idx, option_idx) => {
                let options = self.multi_select_options.entry(question_idx).or_default();
                if options.contains(&option_idx) {
                    options.remove(&option_idx);
                } else {
                    options.insert(option_idx);
                }
                true
            }
            SessionViewMsg::SubmitAllAnswers(answers) => self.handle_submit_answers(ctx, answers),
            SessionViewMsg::ToggleSendModeDropdown => {
                self.send_mode_dropdown_open = !self.send_mode_dropdown_open;
                true
            }
            SessionViewMsg::CloseSendModeDropdown => {
                if self.send_mode_dropdown_open {
                    self.send_mode_dropdown_open = false;
                    true
                } else {
                    false
                }
            }
            SessionViewMsg::SendWiggum => {
                self.send_mode_dropdown_open = false;
                self.handle_send_input_with_mode(ctx, SendMode::Wiggum)
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
            let input: HtmlTextAreaElement = e.target_unchecked_into();
            SessionViewMsg::UpdateInput(input.value())
        });

        let handle_keydown = link.callback(|e: KeyboardEvent| {
            if e.ctrl_key() && e.key().to_lowercase() == "m" {
                e.prevent_default();
                return SessionViewMsg::ToggleVoice;
            }

            match e.key().as_str() {
                "Enter" if !e.shift_key() => {
                    // Enter without Shift submits
                    e.prevent_default();
                    SessionViewMsg::SendInput
                }
                "Enter" => {
                    // Shift+Enter inserts newline (default behavior)
                    SessionViewMsg::CheckAwaiting
                }
                "ArrowUp" => {
                    e.prevent_default();
                    SessionViewMsg::HistoryUp
                }
                "ArrowDown" => {
                    e.prevent_default();
                    SessionViewMsg::HistoryDown
                }
                _ => SessionViewMsg::CheckAwaiting,
            }
        });

        let close_dropdown = link.callback(|_| SessionViewMsg::CloseSendModeDropdown);

        html! {
            <div class="session-view" onclick={close_dropdown}>
                <div class="session-view-messages" ref={self.messages_ref.clone()}>
                    {
                        group_messages(&self.messages).into_iter().map(|group| {
                            html! { <MessageGroupRenderer group={group} session_id={Some(ctx.props().session.id)} /> }
                        }).collect::<Html>()
                    }
                </div>

                { self.render_permission_dialog(ctx) }

                <form class="session-view-input" onsubmit={handle_submit}>
                    <span class="input-prompt">{ ">" }</span>
                    { self.render_interim_transcription() }
                    <textarea
                        ref={self.input_ref.clone()}
                        class={classes!(
                            "message-input",
                            self.interim_transcription.is_some().then_some("has-interim")
                        )}
                        placeholder="Type your message... (Shift+Enter for new line)"
                        value={self.input_value.clone()}
                        oninput={handle_input}
                        onkeydown={handle_keydown}
                        disabled={!self.ws_connected}
                        rows="1"
                    />
                    { self.render_voice_input(ctx) }
                    { self.render_send_button(ctx) }
                </form>
            </div>
        }
    }
}

// Helper methods extracted from the main impl
impl SessionView {
    fn handle_ws_event(&mut self, ctx: &Context<Self>, event: WsEvent) -> bool {
        match event {
            WsEvent::Connected(sender) => {
                ctx.link()
                    .send_message(SessionViewMsg::WebSocketConnected(sender));
                false
            }
            WsEvent::Error(err) => {
                ctx.link().send_message(SessionViewMsg::WebSocketError(err));
                false
            }
            WsEvent::Output(content) => {
                ctx.link()
                    .send_message(SessionViewMsg::ReceivedOutput(content));
                ctx.link().send_message(SessionViewMsg::CheckAwaiting);
                false
            }
            WsEvent::HistoryBatch(messages) => {
                self.messages.extend(messages);
                if self.messages.len() > MAX_MESSAGES_PER_SESSION {
                    let excess = self.messages.len() - MAX_MESSAGES_PER_SESSION;
                    self.messages.drain(0..excess);
                }
                self.last_message_timestamp = Some(
                    js_sys::Date::new_0()
                        .to_iso_string()
                        .as_string()
                        .unwrap_or_default(),
                );
                ctx.link().send_message(SessionViewMsg::CheckAwaiting);
                true
            }
            WsEvent::Permission(perm) => {
                ctx.link()
                    .send_message(SessionViewMsg::PermissionRequest(perm));
                false
            }
            WsEvent::BranchChanged(branch, pr_url) => {
                ctx.link()
                    .send_message(SessionViewMsg::BranchChanged(branch, pr_url));
                false
            }
        }
    }

    fn handle_send_input_with_mode(&mut self, ctx: &Context<Self>, send_mode: SendMode) -> bool {
        crate::audio::ensure_audio_context();
        let input = self.input_value.trim().to_string();
        if input.is_empty() {
            return false;
        }

        self.command_history.push(input.clone());
        self.input_value.clear();

        let session_id = ctx.props().session.id;
        ctx.props().on_message_sent.emit(session_id);

        if let Some(ref sender) = self.ws_sender {
            let msg = ClientToServer::ClaudeInput {
                content: serde_json::Value::String(input),
                send_mode: if send_mode == SendMode::Normal {
                    None
                } else {
                    Some(send_mode)
                },
            };
            send_message(sender, msg);
        }
        true
    }

    fn handle_received_output(&mut self, ctx: &Context<Self>, output: String) -> bool {
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&output) {
            if parsed.get("type").and_then(|t| t.as_str()) == Some("result") {
                if let Some(cost) = parsed.get("total_cost_usd").and_then(|c| c.as_f64()) {
                    if cost != self.total_cost {
                        self.total_cost = cost;
                        self.cost_flash = true;

                        let session_id = ctx.props().session.id;
                        ctx.props().on_cost_change.emit((session_id, cost));

                        let link = ctx.link().clone();
                        spawn_local(async move {
                            gloo::timers::future::TimeoutFuture::new(600).await;
                            link.send_message(SessionViewMsg::ClearCostFlash);
                        });
                    }
                }
            }
        }
        crate::audio::play_sound(crate::audio::SoundEvent::Activity);
        self.messages.push(output);
        if self.messages.len() > MAX_MESSAGES_PER_SESSION {
            let excess = self.messages.len() - MAX_MESSAGES_PER_SESSION;
            self.messages.drain(0..excess);
        }
        self.last_message_timestamp = Some(
            js_sys::Date::new_0()
                .to_iso_string()
                .as_string()
                .unwrap_or_default(),
        );
        true
    }

    fn handle_permission_select(&mut self, delta: i32) -> bool {
        if let Some(ref perm) = self.pending_permission {
            let max = if perm.tool_name == "AskUserQuestion" {
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
                2
            } else {
                1
            };

            if delta < 0 {
                if self.permission_selected > 0 {
                    self.permission_selected -= 1;
                } else {
                    self.permission_selected = max;
                }
            } else if self.permission_selected < max {
                self.permission_selected += 1;
            } else {
                self.permission_selected = 0;
            }
        }
        true
    }

    fn handle_permission_confirm(&mut self, ctx: &Context<Self>) -> bool {
        if let Some(ref perm) = self.pending_permission {
            if perm.tool_name == "AskUserQuestion" {
                if !self.question_answers.is_empty() {
                    ctx.link().send_message(SessionViewMsg::SubmitAllAnswers(
                        self.question_answers.clone(),
                    ));
                }
            } else {
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
        false
    }

    fn handle_approve_permission(&mut self, ctx: &Context<Self>, remember: bool) -> bool {
        if let Some(perm) = self.pending_permission.take() {
            if let Some(ref sender) = self.ws_sender {
                let msg = ClientToServer::PermissionResponse {
                    request_id: perm.request_id,
                    allow: true,
                    input: Some(perm.input),
                    permissions: if remember {
                        perm.permission_suggestions
                    } else {
                        vec![]
                    },
                    reason: None,
                };
                send_message(sender, msg);
            }
            ctx.link().send_message(SessionViewMsg::CheckAwaiting);
            if let Some(input) = self.input_ref.cast::<HtmlTextAreaElement>() {
                let _ = input.focus();
            }
        }
        true
    }

    fn handle_deny_permission(&mut self, ctx: &Context<Self>) -> bool {
        if let Some(perm) = self.pending_permission.take() {
            if let Some(ref sender) = self.ws_sender {
                let msg = ClientToServer::PermissionResponse {
                    request_id: perm.request_id,
                    allow: false,
                    input: None,
                    permissions: vec![],
                    reason: Some("User denied".to_string()),
                };
                send_message(sender, msg);
            }
            ctx.link().send_message(SessionViewMsg::CheckAwaiting);
            if let Some(input) = self.input_ref.cast::<HtmlTextAreaElement>() {
                let _ = input.focus();
            }
        }
        true
    }

    fn handle_ws_error(&mut self, ctx: &Context<Self>, err: String) -> bool {
        crate::audio::play_sound(crate::audio::SoundEvent::Error);
        self.ws_connected = false;
        self.ws_sender = None;
        let session_id = ctx.props().session.id;
        ctx.props().on_connected_change.emit((session_id, false));

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
            let error_msg = ErrorMessage::new(format!("Connection lost: {}", err));
            self.messages
                .push(serde_json::to_string(&error_msg).unwrap_or_default());
        }
        true
    }

    fn attempt_reconnect(&self, ctx: &Context<Self>) {
        let link = ctx.link().clone();
        let session_id = ctx.props().session.id;
        let replay_after = self.last_message_timestamp.clone();

        let on_event = Callback::from(move |event: WsEvent| {
            link.send_message(SessionViewMsg::WsEvent(event));
        });
        connect_websocket(session_id, replay_after, true, on_event);
    }

    fn handle_submit_answers(&mut self, ctx: &Context<Self>, answers: QuestionAnswers) -> bool {
        if let Some(perm) = self.pending_permission.take() {
            if let Some(ref sender) = self.ws_sender {
                let answers_json = if let Some(parsed) = parse_ask_user_question(&perm.input) {
                    let mut pa = PermissionAnswers::empty();
                    for (idx, answer) in answers.iter() {
                        if let Some(q) = parsed.questions.get(*idx) {
                            pa.answers.insert(
                                q.question.clone(),
                                serde_json::Value::String(answer.clone()),
                            );
                        }
                    }
                    serde_json::to_value(&pa).unwrap_or_default()
                } else {
                    serde_json::to_value(PermissionAnswers::empty()).unwrap_or_default()
                };

                let msg = ClientToServer::PermissionResponse {
                    request_id: perm.request_id,
                    allow: true,
                    input: Some(answers_json),
                    permissions: vec![],
                    reason: None,
                };
                send_message(sender, msg);
            }
            self.multi_select_options.clear();
            self.question_answers.clear();
            ctx.link().send_message(SessionViewMsg::CheckAwaiting);
            if let Some(input) = self.input_ref.cast::<HtmlTextAreaElement>() {
                let _ = input.focus();
            }
        }
        true
    }

    fn render_permission_dialog(&self, ctx: &Context<Self>) -> Html {
        if let Some(ref perm) = self.pending_permission {
            let link = ctx.link();
            let on_select_up = link.callback(|_| SessionViewMsg::PermissionSelectUp);
            let on_select_down = link.callback(|_| SessionViewMsg::PermissionSelectDown);
            let on_confirm = link.callback(|_| SessionViewMsg::PermissionConfirm);
            let on_select_and_confirm = link.callback(SessionViewMsg::PermissionSelectAndConfirm);
            let on_submit_answers = link.callback(SessionViewMsg::SubmitAllAnswers);
            let on_set_answer =
                link.callback(|(q_idx, answer)| SessionViewMsg::SetQuestionAnswer(q_idx, answer));
            let on_toggle_option = link
                .callback(|(q_idx, opt_idx)| SessionViewMsg::ToggleQuestionOption(q_idx, opt_idx));

            html! {
                <PermissionDialog
                    permission={perm.clone()}
                    selected={self.permission_selected}
                    multi_select_options={self.multi_select_options.clone()}
                    question_answers={self.question_answers.clone()}
                    dialog_ref={self.permission_ref.clone()}
                    {on_select_up}
                    {on_select_down}
                    {on_confirm}
                    {on_select_and_confirm}
                    {on_submit_answers}
                    {on_set_answer}
                    {on_toggle_option}
                />
            }
        } else {
            html! {}
        }
    }

    fn render_interim_transcription(&self) -> Html {
        if let Some(ref interim) = self.interim_transcription {
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

    fn render_voice_input(&self, ctx: &Context<Self>) -> Html {
        if ctx.props().voice_enabled {
            let link = ctx.link();
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

    fn render_send_button(&self, ctx: &Context<Self>) -> Html {
        let link = ctx.link();
        let on_send = link.callback(|_| SessionViewMsg::SendInput);
        let on_toggle_dropdown = link.callback(|e: MouseEvent| {
            e.stop_propagation();
            SessionViewMsg::ToggleSendModeDropdown
        });
        let on_wiggum = link.callback(|_| SessionViewMsg::SendWiggum);

        let dropdown_class = if self.send_mode_dropdown_open {
            "send-mode-dropdown open"
        } else {
            "send-mode-dropdown"
        };

        html! {
            <div class="send-button-container">
                <button
                    type="submit"
                    class="send-button"
                    disabled={!self.ws_connected}
                    onclick={on_send}
                >
                    { "Send" }
                </button>
                <button
                    type="button"
                    class="send-mode-toggle"
                    disabled={!self.ws_connected}
                    onclick={on_toggle_dropdown}
                >
                    { "▼" }
                </button>
                <div class={dropdown_class}>
                    <button
                        type="button"
                        class="dropdown-option selected"
                        onclick={link.callback(|_| SessionViewMsg::CloseSendModeDropdown)}
                    >
                        { "Send" }
                        <span class="option-hint">{ "Normal message" }</span>
                    </button>
                    <button
                        type="button"
                        class="dropdown-option wiggum"
                        onclick={on_wiggum}
                    >
                        <span class="wiggum-label">
                            <img src="wiggum.png" alt="" class="wiggum-icon" />
                            { "Wiggum" }
                        </span>
                        <span class="option-hint">{ "Loop until DONE" }</span>
                    </button>
                </div>
            </div>
        }
    }
}
