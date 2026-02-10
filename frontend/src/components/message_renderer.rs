use super::markdown::render_markdown;
use super::tool_renderers::render_tool_use;
use gloo_net::http::Request;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use shared::ToolResultContent;
use uuid::Uuid;
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;

/// A group of messages to render together
#[derive(Debug, Clone, PartialEq)]
pub enum MessageGroup {
    /// A single non-assistant message
    Single(String),
    /// Multiple consecutive assistant messages grouped together
    AssistantGroup(Vec<String>),
}

/// Check if a message should be grouped with assistant messages
/// This includes assistant messages AND tool result messages (user messages containing only tool results)
fn should_group_with_assistant(json: &str) -> bool {
    match serde_json::from_str::<ClaudeMessage>(json) {
        Ok(ClaudeMessage::Assistant(_)) => true,
        Ok(ClaudeMessage::User(msg)) => {
            if msg.content.is_some() {
                return false;
            }
            if let Some(message) = &msg.message {
                if let Some(blocks) = &message.content {
                    return !blocks.is_empty()
                        && blocks
                            .iter()
                            .all(|b| matches!(b, ContentBlock::ToolResult { .. }));
                }
            }
            false
        }
        _ => false,
    }
}

/// Group consecutive assistant messages (and their tool results) together
pub fn group_messages(messages: &[String]) -> Vec<MessageGroup> {
    let mut groups = Vec::new();
    let mut current_assistant_group: Vec<String> = Vec::new();

    for json in messages {
        if should_group_with_assistant(json) {
            current_assistant_group.push(json.clone());
        } else {
            if !current_assistant_group.is_empty() {
                groups.push(MessageGroup::AssistantGroup(std::mem::take(
                    &mut current_assistant_group,
                )));
            }
            groups.push(MessageGroup::Single(json.clone()));
        }
    }

    if !current_assistant_group.is_empty() {
        groups.push(MessageGroup::AssistantGroup(current_assistant_group));
    }

    groups
}

// --- Message types ---

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ClaudeMessage {
    #[serde(rename = "system")]
    System(SystemMessage),
    #[serde(rename = "assistant")]
    Assistant(AssistantMessage),
    #[serde(rename = "result")]
    Result(ResultMessage),
    #[serde(rename = "user")]
    User(UserMessage),
    #[serde(rename = "error")]
    Error(ErrorMessage),
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UserMessage {
    pub content: Option<String>,
    pub message: Option<UserMessageContent>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UserMessageContent {
    pub content: Option<Vec<ContentBlock>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ErrorDetails {
    #[serde(rename = "type")]
    pub error_type: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ErrorMessage {
    pub message: Option<String>,
    pub error: Option<ErrorDetails>,
    pub request_id: Option<String>,
}

impl ErrorMessage {
    pub fn is_overload(&self) -> bool {
        self.error
            .as_ref()
            .and_then(|e| e.error_type.as_deref())
            .map(|t| t == "overloaded_error")
            .unwrap_or(false)
    }

    pub fn display_message(&self) -> &str {
        self.error
            .as_ref()
            .and_then(|e| e.message.as_deref())
            .or(self.message.as_deref())
            .unwrap_or("Unknown error")
    }

    pub fn error_type(&self) -> Option<&str> {
        self.error.as_ref().and_then(|e| e.error_type.as_deref())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SystemMessage {
    pub subtype: Option<String>,
    pub session_id: Option<String>,
    pub model: Option<String>,
    pub cwd: Option<String>,
    pub claude_code_version: Option<String>,
    pub tools: Option<Vec<String>>,
    pub agents: Option<Vec<String>>,
    pub skills: Option<Vec<String>>,
    pub slash_commands: Option<Vec<String>>,
    pub mcp_servers: Option<Vec<Value>>,
    pub plugins: Option<Vec<Value>>,
    pub summary: Option<String>,
    pub leaf_message_count: Option<u32>,
    pub duration_ms: Option<u64>,
    #[serde(flatten)]
    pub extra: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AssistantMessage {
    pub message: Option<MessageContent>,
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MessageContent {
    pub id: Option<String>,
    pub model: Option<String>,
    pub role: Option<String>,
    pub content: Option<Vec<ContentBlock>>,
    pub usage: Option<UsageInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: Option<ToolResultContent>,
        #[serde(default)]
        is_error: bool,
    },
    #[serde(rename = "thinking")]
    Thinking { thinking: String },
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UsageInfo {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_input_tokens: Option<u64>,
    pub cache_creation_input_tokens: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ResultMessage {
    pub subtype: Option<String>,
    pub session_id: Option<String>,
    pub result: Option<String>,
    pub is_error: Option<bool>,
    pub duration_ms: Option<u64>,
    pub duration_api_ms: Option<u64>,
    pub total_cost_usd: Option<f64>,
    pub num_turns: Option<u64>,
    pub usage: Option<UsageInfo>,
}

// --- Components ---

#[derive(Properties, PartialEq)]
pub struct MessageRendererProps {
    pub json: String,
    #[prop_or_default]
    pub session_id: Option<Uuid>,
}

#[function_component(MessageRenderer)]
pub fn message_renderer(props: &MessageRendererProps) -> Html {
    let parsed: Result<ClaudeMessage, _> = serde_json::from_str(&props.json);

    match parsed {
        Ok(ClaudeMessage::System(msg)) => render_system_message(&msg),
        Ok(ClaudeMessage::Assistant(msg)) => render_assistant_message(&msg),
        Ok(ClaudeMessage::Result(msg)) => render_result_message(&msg),
        Ok(ClaudeMessage::User(msg)) => render_user_message(&msg),
        Ok(ClaudeMessage::Error(msg)) => render_error_message(&msg),
        Ok(ClaudeMessage::Unknown) | Err(_) => {
            html! { <RawMessageRenderer json={props.json.clone()} session_id={props.session_id} /> }
        }
    }
}

#[derive(Properties, PartialEq)]
pub struct MessageGroupRendererProps {
    pub group: MessageGroup,
    #[prop_or_default]
    pub session_id: Option<Uuid>,
}

#[function_component(MessageGroupRenderer)]
pub fn message_group_renderer(props: &MessageGroupRendererProps) -> Html {
    match &props.group {
        MessageGroup::Single(json) => {
            html! { <MessageRenderer json={json.clone()} session_id={props.session_id} /> }
        }
        MessageGroup::AssistantGroup(messages) => render_assistant_group(messages),
    }
}

// --- Message renderers ---

fn render_assistant_group(messages: &[String]) -> Html {
    let mut all_blocks: Vec<ContentBlock> = Vec::new();
    let mut total_output_tokens: u64 = 0;
    let mut total_input_tokens: u64 = 0;
    let mut total_cache_read: u64 = 0;
    let mut total_cache_created: u64 = 0;
    let mut model_name = String::new();

    for json in messages {
        match serde_json::from_str::<ClaudeMessage>(json) {
            Ok(ClaudeMessage::Assistant(msg)) => {
                if let Some(message) = &msg.message {
                    if let Some(blocks) = &message.content {
                        all_blocks.extend(blocks.clone());
                    }
                    if let Some(usage) = &message.usage {
                        total_output_tokens += usage.output_tokens.unwrap_or(0);
                        total_input_tokens += usage.input_tokens.unwrap_or(0);
                        total_cache_read += usage.cache_read_input_tokens.unwrap_or(0);
                        total_cache_created += usage.cache_creation_input_tokens.unwrap_or(0);
                    }
                    if model_name.is_empty() {
                        if let Some(m) = &message.model {
                            model_name = m.clone();
                        }
                    }
                }
            }
            Ok(ClaudeMessage::User(msg)) => {
                if let Some(message) = &msg.message {
                    if let Some(blocks) = &message.content {
                        all_blocks.extend(blocks.clone());
                    }
                }
            }
            _ => {}
        }
    }

    let count = messages.len();
    let usage_tooltip = format!(
        "Input: {} | Output: {} | Cache read: {} | Cache created: {} | {} messages",
        total_input_tokens, total_output_tokens, total_cache_read, total_cache_created, count
    );

    html! {
        <div class="claude-message assistant-message">
            <div class="message-header">
                <span class="message-type-badge assistant">{ "Assistant" }</span>
                {
                    if count > 1 {
                        html! { <span class="message-count" title={format!("{} consecutive messages", count)}>{ format!("{} messages", count) }</span> }
                    } else {
                        html! {}
                    }
                }
                {
                    if let Some(short_name) = shorten_model_name(&model_name) {
                        html! { <span class="model-name" title={model_name.clone()}>{ short_name }</span> }
                    } else {
                        html! {}
                    }
                }
                {
                    if total_input_tokens > 0 || total_output_tokens > 0 {
                        html! {
                            <span class="usage-badge" title={usage_tooltip}>
                                <span class="token-count">{ format!("{}↓ {}↑", total_input_tokens, total_output_tokens) }</span>
                            </span>
                        }
                    } else {
                        html! {}
                    }
                }
            </div>
            <div class="message-body">
                { render_content_blocks(&all_blocks) }
            </div>
        </div>
    }
}

fn render_user_message(msg: &UserMessage) -> Html {
    if let Some(text) = &msg.content {
        html! {
            <div class="claude-message user-message">
                <div class="message-header">
                    <span class="message-type-badge user">{ "You" }</span>
                </div>
                <div class="message-body">
                    <div class="user-text">{ render_markdown(text) }</div>
                </div>
            </div>
        }
    } else if let Some(message) = &msg.message {
        let blocks = message.content.as_ref().cloned().unwrap_or_default();

        let text_content: String = blocks
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text } => Some(text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");

        let has_tool_results = blocks
            .iter()
            .any(|b| matches!(b, ContentBlock::ToolResult { .. }));

        if has_tool_results {
            html! {
                <div class="claude-message user-message tool-result-message">
                    <div class="message-body">
                        { render_content_blocks(&blocks) }
                    </div>
                </div>
            }
        } else if !text_content.is_empty() {
            html! {
                <div class="claude-message user-message">
                    <div class="message-header">
                        <span class="message-type-badge user">{ "You" }</span>
                    </div>
                    <div class="message-body">
                        <div class="user-text">{ render_markdown(&text_content) }</div>
                    </div>
                </div>
            }
        } else {
            html! {}
        }
    } else {
        html! {}
    }
}

fn render_error_message(msg: &ErrorMessage) -> Html {
    if msg.is_overload() {
        return render_overload_error(msg);
    }

    let message = msg.display_message();
    let error_type = msg.error_type();

    html! {
        <div class="claude-message error-message-display">
            <div class="message-header">
                <span class="message-type-badge result error">{ "Error" }</span>
                {
                    if let Some(err_type) = error_type {
                        html! { <span class="error-type">{ err_type }</span> }
                    } else {
                        html! {}
                    }
                }
            </div>
            <div class="message-body">
                <div class="error-text">{ message }</div>
            </div>
        </div>
    }
}

fn render_overload_error(msg: &ErrorMessage) -> Html {
    let request_id = msg.request_id.as_deref().unwrap_or("unknown");

    html! {
        <div class="claude-message overload-message">
            <div class="message-header">
                <span class="message-type-badge overload">{ "API Busy" }</span>
            </div>
            <div class="message-body">
                <div class="overload-content">
                    <div class="overload-icon">{ "⏳" }</div>
                    <div class="overload-text">
                        <div class="overload-title">{ "Claude API is temporarily overloaded" }</div>
                        <div class="overload-description">
                            { "The API is experiencing high demand. Claude Code will automatically retry the request. Please wait a moment." }
                        </div>
                    </div>
                </div>
                <div class="overload-details">
                    <span class="request-id" title="Request ID for debugging">{ format!("Request: {}", request_id) }</span>
                </div>
            </div>
        </div>
    }
}

fn render_system_message(msg: &SystemMessage) -> Html {
    let subtype = msg.subtype.as_deref().unwrap_or("system");

    if subtype == "init" || subtype == "status" {
        return html! {};
    }

    if subtype == "summary" || subtype == "compaction" || subtype == "context_compaction" {
        return render_compaction_message(msg);
    }

    html! {
        <div class="claude-message system-message compact">
            <span class="message-type-badge system">{ subtype }</span>
        </div>
    }
}

fn render_compaction_message(msg: &SystemMessage) -> Html {
    let summary_text = msg.summary.as_deref().or_else(|| {
        msg.extra.as_ref().and_then(|v| {
            v.get("summary")
                .and_then(|s| s.as_str())
                .or_else(|| v.get("content").and_then(|s| s.as_str()))
                .or_else(|| v.get("text").and_then(|s| s.as_str()))
        })
    });

    let leaf_count = msg.leaf_message_count.or_else(|| {
        msg.extra.as_ref().and_then(|v| {
            v.get("leaf_message_count")
                .and_then(|n| n.as_u64())
                .map(|n| n as u32)
                .or_else(|| {
                    v.get("message_count")
                        .and_then(|n| n.as_u64())
                        .map(|n| n as u32)
                })
        })
    });

    let duration = msg.duration_ms.or_else(|| {
        msg.extra
            .as_ref()
            .and_then(|v| v.get("duration_ms").and_then(|n| n.as_u64()))
    });

    html! {
        <div class="claude-message compaction-message">
            <div class="message-header">
                <span class="message-type-badge compaction">{ "Context Compacted" }</span>
                {
                    if let Some(count) = leaf_count {
                        html! {
                            <span class="compaction-stat" title="Messages summarized">
                                { format!("{} messages", count) }
                            </span>
                        }
                    } else {
                        html! {}
                    }
                }
                {
                    if let Some(ms) = duration {
                        html! {
                            <span class="compaction-stat" title="Compaction duration">
                                { format_duration(ms) }
                            </span>
                        }
                    } else {
                        html! {}
                    }
                }
            </div>
            <div class="message-body">
                <div class="compaction-content">
                    <div class="compaction-icon">{ "📦" }</div>
                    <div class="compaction-text">
                        {
                            if let Some(summary) = summary_text {
                                html! {
                                    <div class="compaction-summary">
                                        <div class="summary-label">{ "Summary:" }</div>
                                        <div class="summary-text">{ render_markdown(summary) }</div>
                                    </div>
                                }
                            } else {
                                html! {
                                    <div class="compaction-description">
                                        { "The conversation context has been summarized to free up space. Previous messages have been condensed while preserving important context." }
                                    </div>
                                }
                            }
                        }
                    </div>
                </div>
            </div>
        </div>
    }
}

fn render_assistant_message(msg: &AssistantMessage) -> Html {
    let blocks = msg
        .message
        .as_ref()
        .and_then(|m| m.content.as_ref())
        .cloned()
        .unwrap_or_default();

    let usage = msg.message.as_ref().and_then(|m| m.usage.as_ref());
    let model = msg
        .message
        .as_ref()
        .and_then(|m| m.model.as_ref())
        .map(|s| s.as_str())
        .unwrap_or("");

    let usage_tooltip = usage
        .map(|u| {
            format!(
                "Input: {} | Output: {} | Cache read: {} | Cache created: {}",
                u.input_tokens.unwrap_or(0),
                u.output_tokens.unwrap_or(0),
                u.cache_read_input_tokens.unwrap_or(0),
                u.cache_creation_input_tokens.unwrap_or(0)
            )
        })
        .unwrap_or_default();

    html! {
        <div class="claude-message assistant-message">
            <div class="message-header">
                <span class="message-type-badge assistant">{ "Assistant" }</span>
                {
                    if let Some(short_name) = shorten_model_name(model) {
                        html! { <span class="model-name" title={model.to_string()}>{ short_name }</span> }
                    } else {
                        html! {}
                    }
                }
                {
                    if let Some(u) = usage {
                        html! {
                            <span class="usage-badge" title={usage_tooltip}>
                                <span class="token-count">{ format!("{}↓ {}↑", u.input_tokens.unwrap_or(0), u.output_tokens.unwrap_or(0)) }</span>
                            </span>
                        }
                    } else {
                        html! {}
                    }
                }
            </div>
            <div class="message-body">
                { render_content_blocks(&blocks) }
            </div>
        </div>
    }
}

fn render_content_blocks(blocks: &[ContentBlock]) -> Html {
    html! {
        <>
            {
                blocks.iter().map(|block| {
                    match block {
                        ContentBlock::Text { text } => {
                            html! { <div class="assistant-text">{ render_markdown(text) }</div> }
                        }
                        ContentBlock::ToolUse { id: _, name, input } => {
                            render_tool_use(name, input)
                        }
                        ContentBlock::ToolResult { tool_use_id: _, content, is_error } => {
                            let class = if *is_error { "tool-result error" } else { "tool-result" };
                            match content {
                                Some(ToolResultContent::Text(s)) => {
                                    let display = if s.len() > 500 {
                                        format!("{}...", truncate_str(s, 500))
                                    } else {
                                        s.clone()
                                    };
                                    html! {
                                        <div class={class}>
                                            <pre class="tool-result-content">{ display }</pre>
                                        </div>
                                    }
                                }
                                Some(ToolResultContent::Structured(blocks)) => {
                                    html! {
                                        <div class={class}>
                                            { for blocks.iter().map(render_structured_block) }
                                        </div>
                                    }
                                }
                                None => html! { <div class={class}></div> },
                            }
                        }
                        ContentBlock::Thinking { thinking } => {
                            html! {
                                <div class="thinking-block">
                                    <span class="thinking-label">{ "thinking" }</span>
                                    <div class="thinking-content">{ thinking }</div>
                                </div>
                            }
                        }
                        ContentBlock::Other => html! {},
                    }
                }).collect::<Html>()
            }
        </>
    }
}

const ALLOWED_IMAGE_MEDIA_TYPES: &[&str] = &[
    "image/png",
    "image/jpeg",
    "image/gif",
    "image/webp",
    "image/svg+xml",
];

fn render_structured_block(block: &Value) -> Html {
    let block_type = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
    match block_type {
        "image" => {
            let source = block.get("source");
            let media_type = source
                .and_then(|s| s.get("media_type"))
                .and_then(|m| m.as_str())
                .unwrap_or("image/png");
            if !ALLOWED_IMAGE_MEDIA_TYPES.contains(&media_type) {
                return html! {
                    <pre class="tool-result-content">
                        { format!("[unsupported image type: {}]", media_type) }
                    </pre>
                };
            }
            let data = source
                .and_then(|s| s.get("data"))
                .and_then(|d| d.as_str())
                .unwrap_or("");
            let src = format!("data:{};base64,{}", media_type, data);
            html! {
                <div class="tool-result-image">
                    <img src={src} alt="Tool result image" />
                </div>
            }
        }
        "text" => {
            let text = block.get("text").and_then(|t| t.as_str()).unwrap_or("");
            let display = if text.len() > 500 {
                format!("{}...", truncate_str(text, 500))
            } else {
                text.to_string()
            };
            html! { <pre class="tool-result-content">{ display }</pre> }
        }
        _ => {
            let json = serde_json::to_string_pretty(block).unwrap_or_default();
            html! { <pre class="tool-result-content">{ json }</pre> }
        }
    }
}

fn render_result_message(msg: &ResultMessage) -> Html {
    let is_error = msg.is_error.unwrap_or(false);
    let status_class = if is_error { "error" } else { "success" };

    let duration_ms = msg.duration_ms.unwrap_or(0);
    let api_ms = msg.duration_api_ms.unwrap_or(0);
    let turns = msg.num_turns.unwrap_or(0);

    let timing_tooltip = format!(
        "Total: {}ms | API: {}ms | Turns: {}",
        duration_ms, api_ms, turns
    );

    if is_error {
        if let Some(error_html) = try_render_api_error(msg.result.as_deref()) {
            return html! {
                <>
                    { error_html }
                    <div class={classes!("claude-message", "result-message", status_class)}>
                        <div class="result-stats-bar">
                            <span class={classes!("result-status", status_class)}>{ "✗" }</span>
                            <span class="stat-item duration" title={timing_tooltip.clone()}>
                                { format_duration(duration_ms) }
                            </span>
                        </div>
                    </div>
                </>
            };
        }
    }

    html! {
        <div class={classes!("claude-message", "result-message", status_class)}>
            <div class="result-stats-bar">
                <span class={classes!("result-status", status_class)}>
                    { if is_error { "✗" } else { "✓" } }
                </span>
                <span class="stat-item duration" title={timing_tooltip.clone()}>
                    { format_duration(duration_ms) }
                </span>
                {
                    if let Some(usage) = &msg.usage {
                        html! {
                            <>
                                <span class="stat-item tokens" title="Input tokens">
                                    { format!("{}↓", usage.input_tokens.unwrap_or(0)) }
                                </span>
                                <span class="stat-item tokens" title="Output tokens">
                                    { format!("{}↑", usage.output_tokens.unwrap_or(0)) }
                                </span>
                            </>
                        }
                    } else {
                        html! {}
                    }
                }
                {
                    if turns > 1 {
                        html! {
                            <span class="stat-item turns" title="API turns">
                                { format!("{} turns", turns) }
                            </span>
                        }
                    } else {
                        html! {}
                    }
                }
            </div>
        </div>
    }
}

// --- API error rendering ---

#[derive(Debug, Deserialize)]
struct AnthropicApiError {
    #[serde(rename = "type")]
    error_type: Option<String>,
    error: Option<AnthropicErrorDetails>,
    request_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AnthropicErrorDetails {
    #[serde(rename = "type")]
    error_type: Option<String>,
    message: Option<String>,
}

fn try_render_api_error(result_text: Option<&str>) -> Option<Html> {
    let text = result_text?;

    let json_start = text.find('{')?;
    let json_str = &text[json_start..];

    let api_error: AnthropicApiError = serde_json::from_str(json_str).ok()?;

    if api_error.error_type.as_deref() != Some("error") {
        return None;
    }

    let error_details = api_error.error.as_ref();
    let error_type = error_details
        .and_then(|e| e.error_type.as_deref())
        .unwrap_or("unknown_error");
    let error_message = error_details
        .and_then(|e| e.message.as_deref())
        .unwrap_or("An error occurred");
    let request_id = api_error.request_id.as_deref();

    let http_status = if text.starts_with("API Error:") {
        text.split_whitespace()
            .nth(2)
            .and_then(|s| s.parse::<u16>().ok())
    } else {
        None
    };

    let display_type = format_error_type(error_type);

    Some(html! {
        <div class="claude-message anthropic-error-message">
            <div class="message-header">
                <span class="message-type-badge anthropic-error">{ "Anthropic API Error" }</span>
                {
                    if let Some(status) = http_status {
                        html! { <span class="http-status">{ format!("HTTP {}", status) }</span> }
                    } else {
                        html! {}
                    }
                }
            </div>
            <div class="message-body">
                <div class="anthropic-error-content">
                    <div class="error-icon">{ "⚠" }</div>
                    <div class="error-details">
                        <div class="error-type-display">{ display_type }</div>
                        <div class="error-message-text">{ error_message }</div>
                    </div>
                </div>
                {
                    if let Some(req_id) = request_id {
                        html! {
                            <div class="error-request-id">
                                <span class="request-id-label">{ "Request ID: " }</span>
                                <code class="request-id-value">{ req_id }</code>
                            </div>
                        }
                    } else {
                        html! {}
                    }
                }
            </div>
        </div>
    })
}

fn format_error_type(error_type: &str) -> String {
    match error_type {
        "api_error" => "Internal Server Error".to_string(),
        "authentication_error" => "Authentication Failed".to_string(),
        "invalid_request_error" => "Invalid Request".to_string(),
        "not_found_error" => "Not Found".to_string(),
        "overloaded_error" => "API Overloaded".to_string(),
        "permission_error" => "Permission Denied".to_string(),
        "rate_limit_error" => "Rate Limited".to_string(),
        "request_too_large" => "Request Too Large".to_string(),
        other => other.replace('_', " ").to_string(),
    }
}

// --- Raw message logging ---

#[derive(Serialize)]
struct LogRawMessageRequest {
    session_id: Option<Uuid>,
    message_content: Value,
    message_source: String,
    render_reason: Option<String>,
}

fn log_raw_message(session_id: Option<Uuid>, json: &str, reason: &str) {
    let message_content =
        serde_json::from_str::<Value>(json).unwrap_or_else(|_| Value::String(json.to_string()));

    let request_body = LogRawMessageRequest {
        session_id,
        message_content,
        message_source: "frontend".to_string(),
        render_reason: Some(reason.to_string()),
    };

    spawn_local(async move {
        let result = Request::post("/api/raw-messages")
            .header("Content-Type", "application/json")
            .json(&request_body)
            .map_err(|e| format!("Failed to serialize: {:?}", e))
            .map(|req| req.send());

        if let Ok(future) = result {
            if let Err(e) = future.await {
                log::warn!("Failed to log raw message: {:?}", e);
            }
        }
    });
}

#[derive(Properties, PartialEq)]
pub struct RawMessageRendererProps {
    pub json: String,
    #[prop_or_default]
    pub session_id: Option<Uuid>,
}

#[function_component(RawMessageRenderer)]
pub fn raw_message_renderer(props: &RawMessageRendererProps) -> Html {
    let json = props.json.clone();
    let session_id = props.session_id;

    use_effect_with(json.clone(), move |json| {
        let reason = match serde_json::from_str::<Value>(json) {
            Ok(val) => {
                if let Some(msg_type) = val.get("type").and_then(|t| t.as_str()) {
                    format!("Unknown message type: {}", msg_type)
                } else {
                    "Message has no 'type' field".to_string()
                }
            }
            Err(e) => format!("JSON parse error: {}", e),
        };

        log_raw_message(session_id, json, &reason);
        || ()
    });

    render_raw_json(&props.json)
}

fn render_raw_json(json: &str) -> Html {
    let display = serde_json::from_str::<Value>(json)
        .ok()
        .and_then(|v| serde_json::to_string_pretty(&v).ok())
        .unwrap_or_else(|| json.to_string());

    html! {
        <div class="claude-message raw-message">
            <div class="message-header">
                <span class="message-type-badge raw">{ "Raw" }</span>
            </div>
            <div class="message-body">
                <pre class="raw-json">{ display }</pre>
            </div>
        </div>
    }
}

// --- Utility functions (used by tool_renderers) ---

pub fn truncate_str(s: &str, max_len: usize) -> &str {
    if s.len() <= max_len {
        s
    } else {
        let mut end = max_len;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        &s[..end]
    }
}

fn shorten_model_name(model: &str) -> Option<String> {
    if model.is_empty() || model.starts_with('<') {
        return None;
    }

    // Extract version from model strings like:
    // - "claude-opus-4-5-20251101" -> "4.5"
    // - "claude-sonnet-4-5-20250929" -> "4.5"
    // - "claude-3-5-sonnet-20241022" -> "3.5"
    let extract_version = |model: &str| -> Option<String> {
        let parts: Vec<&str> = model.split('-').collect();
        // Look for two consecutive numeric parts (e.g. "4-6" in "claude-opus-4-6")
        for i in 0..parts.len().saturating_sub(1) {
            if let (Ok(major), Ok(minor)) = (parts[i].parse::<u32>(), parts[i + 1].parse::<u32>()) {
                // Skip if minor looks like a date (8+ digits)
                if parts[i + 1].len() >= 8 {
                    continue;
                }
                return Some(format!("{}.{}", major, minor));
            }
        }
        None
    };

    let version = extract_version(model);

    Some(if model.contains("opus") {
        match version {
            Some(v) => format!("Opus {}", v),
            None => "Opus".to_string(),
        }
    } else if model.contains("sonnet") {
        match version {
            Some(v) => format!("Sonnet {}", v),
            None => "Sonnet".to_string(),
        }
    } else if model.contains("haiku") {
        match version {
            Some(v) => format!("Haiku {}", v),
            None => "Haiku".to_string(),
        }
    } else {
        model.split('-').next().unwrap_or(model).to_string()
    })
}

pub fn format_duration(ms: u64) -> String {
    if ms < 1000 {
        format!("{}ms", ms)
    } else if ms < 60000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        let mins = ms / 60000;
        let secs = (ms % 60000) / 1000;
        format!("{}m {}s", mins, secs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_message_overload_detection() {
        let msg = ErrorMessage {
            message: None,
            error: Some(ErrorDetails {
                error_type: Some("overloaded_error".to_string()),
                message: Some("Overloaded".to_string()),
            }),
            request_id: Some("req_123".to_string()),
        };
        assert!(msg.is_overload());
        assert_eq!(msg.display_message(), "Overloaded");
        assert_eq!(msg.error_type(), Some("overloaded_error"));
    }

    #[test]
    fn test_error_message_regular_error() {
        let msg = ErrorMessage {
            message: Some("Something went wrong".to_string()),
            error: None,
            request_id: None,
        };
        assert!(!msg.is_overload());
        assert_eq!(msg.display_message(), "Something went wrong");
        assert_eq!(msg.error_type(), None);
    }

    #[test]
    fn test_error_message_api_error() {
        let msg = ErrorMessage {
            message: None,
            error: Some(ErrorDetails {
                error_type: Some("invalid_request_error".to_string()),
                message: Some("Invalid API key".to_string()),
            }),
            request_id: Some("req_456".to_string()),
        };
        assert!(!msg.is_overload());
        assert_eq!(msg.display_message(), "Invalid API key");
        assert_eq!(msg.error_type(), Some("invalid_request_error"));
    }

    #[test]
    fn test_error_message_empty() {
        let msg = ErrorMessage::default();
        assert!(!msg.is_overload());
        assert_eq!(msg.display_message(), "Unknown error");
        assert_eq!(msg.error_type(), None);
    }

    #[test]
    fn test_shorten_model_name() {
        // Standard model names with version
        assert_eq!(
            shorten_model_name("claude-opus-4-5-20251101"),
            Some("Opus 4.5".to_string())
        );
        assert_eq!(
            shorten_model_name("claude-sonnet-4-5-20250929"),
            Some("Sonnet 4.5".to_string())
        );
        assert_eq!(
            shorten_model_name("claude-haiku-4-5-20251001"),
            Some("Haiku 4.5".to_string())
        );

        // Older model format
        assert_eq!(
            shorten_model_name("claude-3-5-sonnet-20241022"),
            Some("Sonnet 3.5".to_string())
        );

        // Model IDs without date suffix
        assert_eq!(
            shorten_model_name("claude-opus-4-6"),
            Some("Opus 4.6".to_string())
        );
        assert_eq!(
            shorten_model_name("claude-sonnet-4-5"),
            Some("Sonnet 4.5".to_string())
        );

        // No version found - fallback
        assert_eq!(shorten_model_name("claude-opus"), Some("Opus".to_string()));

        // Empty or invalid
        assert_eq!(shorten_model_name(""), None);
        assert_eq!(shorten_model_name("<unknown>"), None);

        // Unknown model
        assert_eq!(shorten_model_name("gpt-4-turbo"), Some("gpt".to_string()));
    }
}
