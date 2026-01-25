use super::markdown::render_markdown;
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
            // Check if this is a tool result message (no direct content, has message.content with tool_results)
            if msg.content.is_some() {
                return false; // Has direct text content = real user message
            }
            if let Some(message) = &msg.message {
                if let Some(blocks) = &message.content {
                    // If all blocks are tool results, group with assistant
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
            // Flush any pending assistant group
            if !current_assistant_group.is_empty() {
                groups.push(MessageGroup::AssistantGroup(std::mem::take(
                    &mut current_assistant_group,
                )));
            }
            groups.push(MessageGroup::Single(json.clone()));
        }
    }

    // Don't forget trailing assistant messages
    if !current_assistant_group.is_empty() {
        groups.push(MessageGroup::AssistantGroup(current_assistant_group));
    }

    groups
}

/// Parsed message types from Claude Code
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
    /// Simple text content (for user input messages)
    pub content: Option<String>,
    /// Nested message structure (for tool result messages)
    pub message: Option<UserMessageContent>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UserMessageContent {
    pub content: Option<Vec<ContentBlock>>,
}

/// Inner error details from API errors
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ErrorDetails {
    #[serde(rename = "type")]
    pub error_type: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ErrorMessage {
    /// Direct message field (for simple errors)
    pub message: Option<String>,
    /// Nested error object (for API errors like overload)
    pub error: Option<ErrorDetails>,
    /// Request ID for API errors
    pub request_id: Option<String>,
}

impl ErrorMessage {
    /// Check if this is an overload error
    pub fn is_overload(&self) -> bool {
        self.error
            .as_ref()
            .and_then(|e| e.error_type.as_deref())
            .map(|t| t == "overloaded_error")
            .unwrap_or(false)
    }

    /// Get the display message
    pub fn display_message(&self) -> &str {
        // Try nested error message first, then direct message
        self.error
            .as_ref()
            .and_then(|e| e.message.as_deref())
            .or(self.message.as_deref())
            .unwrap_or("Unknown error")
    }

    /// Get the error type for display
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
    /// Summary text for compaction messages
    pub summary: Option<String>,
    /// Number of leaf messages compacted
    pub leaf_message_count: Option<u32>,
    /// Duration in ms for compaction
    pub duration_ms: Option<u64>,
    /// Catch-all for other fields we might not know about
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

#[derive(Properties, PartialEq)]
pub struct MessageRendererProps {
    pub json: String,
    /// Optional session ID for logging raw messages
    #[prop_or_default]
    pub session_id: Option<Uuid>,
}

#[function_component(MessageRenderer)]
pub fn message_renderer(props: &MessageRendererProps) -> Html {
    // Try to parse as a known message type
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
    /// Optional session ID for logging raw messages
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

/// Render a group of consecutive assistant messages (and tool results) in a single frame
fn render_assistant_group(messages: &[String]) -> Html {
    // Parse all messages to extract content and sum tokens
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
                    // Collect content blocks
                    if let Some(blocks) = &message.content {
                        all_blocks.extend(blocks.clone());
                    }
                    // Sum up usage
                    if let Some(usage) = &message.usage {
                        total_output_tokens += usage.output_tokens.unwrap_or(0);
                        total_input_tokens += usage.input_tokens.unwrap_or(0);
                        total_cache_read += usage.cache_read_input_tokens.unwrap_or(0);
                        total_cache_created += usage.cache_creation_input_tokens.unwrap_or(0);
                    }
                    // Use the model from the first message that has one
                    if model_name.is_empty() {
                        if let Some(m) = &message.model {
                            model_name = m.clone();
                        }
                    }
                }
            }
            Ok(ClaudeMessage::User(msg)) => {
                // Tool result messages - extract content blocks
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
    // Check if this is a simple text message or a structured message
    if let Some(text) = &msg.content {
        // Simple user input (legacy format)
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

        // Extract text content for display
        let text_content: String = blocks
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text } => Some(text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");

        // Check if this is a tool result or regular user input
        let has_tool_results = blocks
            .iter()
            .any(|b| matches!(b, ContentBlock::ToolResult { .. }));

        if has_tool_results {
            // Tool result message - render compactly
            html! {
                <div class="claude-message user-message tool-result-message">
                    <div class="message-body">
                        { render_content_blocks(&blocks) }
                    </div>
                </div>
            }
        } else if !text_content.is_empty() {
            // Regular user input with text
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
        // Empty message
        html! {}
    }
}

fn render_error_message(msg: &ErrorMessage) -> Html {
    // Check for special error types
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

/// Render a special message for API overload errors
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

    // Hide uninformative system messages
    // - "init": Session initialization (no useful info)
    // - "status": Bare status updates with no content
    if subtype == "init" || subtype == "status" {
        return html! {};
    }

    // Handle compaction/summary messages with special rendering
    if subtype == "summary" || subtype == "compaction" || subtype == "context_compaction" {
        return render_compaction_message(msg);
    }

    html! {
        <div class="claude-message system-message compact">
            <span class="message-type-badge system">{ subtype }</span>
        </div>
    }
}

/// Render a compaction/summary message with a clean, informative display
fn render_compaction_message(msg: &SystemMessage) -> Html {
    // Extract summary text if available
    let summary_text = msg.summary.as_deref().or_else(|| {
        // Try to get summary from extra fields
        msg.extra.as_ref().and_then(|v| {
            v.get("summary")
                .and_then(|s| s.as_str())
                .or_else(|| v.get("content").and_then(|s| s.as_str()))
                .or_else(|| v.get("text").and_then(|s| s.as_str()))
        })
    });

    // Extract statistics if available
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
                            // Extract text from ToolResultContent (can be plain string or array of content blocks)
                            let text = match content {
                                Some(ToolResultContent::Text(s)) => s.clone(),
                                Some(ToolResultContent::Structured(blocks)) => {
                                    // Extract text from content blocks array
                                    blocks
                                        .iter()
                                        .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                                        .collect::<Vec<_>>()
                                        .join("\n")
                                }
                                None => String::new(),
                            };
                            // Truncate long results (using safe UTF-8 boundary)
                            let display = if text.len() > 500 {
                                format!("{}...", truncate_str(&text, 500))
                            } else {
                                text
                            };
                            html! {
                                <div class={class}>
                                    <pre class="tool-result-content">{ display }</pre>
                                </div>
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

/// Render a tool use block with special handling for various tools
/// Registry pattern - add new tool renderers here
fn render_tool_use(name: &str, input: &Value) -> Html {
    match name {
        "Edit" => render_edit_tool_diff(input),
        "Write" => render_write_tool(input),
        "TodoWrite" => render_todowrite_tool(input),
        "AskUserQuestion" => render_askuserquestion_tool(input),
        "ExitPlanMode" => render_exitplanmode_tool(input),
        "Bash" => render_bash_tool(input),
        "Read" => render_read_tool(input),
        "Glob" => render_glob_tool(input),
        "Grep" => render_grep_tool(input),
        "Task" => render_task_tool(input),
        "WebFetch" => render_webfetch_tool(input),
        "WebSearch" => render_websearch_tool(input),
        _ => render_generic_tool(name, input),
    }
}

/// Generic tool renderer for unrecognized tools
fn render_generic_tool(name: &str, input: &Value) -> Html {
    let input_preview = format_tool_input(name, input);
    html! {
        <div class="tool-use">
            <div class="tool-use-header">
                <span class="tool-icon">{ "⚡" }</span>
                <span class="tool-name">{ name }</span>
            </div>
            <pre class="tool-args">{ input_preview }</pre>
        </div>
    }
}

/// Render TodoWrite with status icons and task list
fn render_todowrite_tool(input: &Value) -> Html {
    let todos = input
        .get("todos")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    html! {
        <div class="tool-use todowrite-tool">
            <div class="tool-use-header">
                <span class="tool-icon">{ "📋" }</span>
                <span class="tool-name">{ "TodoWrite" }</span>
                <span class="tool-meta">{ format!("({} items)", todos.len()) }</span>
            </div>
            <div class="todo-list">
                {
                    todos.iter().map(|todo| {
                        let status = todo.get("status").and_then(|s| s.as_str()).unwrap_or("pending");
                        let content = todo.get("content").and_then(|c| c.as_str()).unwrap_or("");
                        let (icon, class) = match status {
                            "completed" => ("✓", "completed"),
                            "in_progress" => ("→", "in-progress"),
                            _ => ("○", "pending"),
                        };
                        html! {
                            <div class={format!("todo-item {}", class)}>
                                <span class="todo-status">{ icon }</span>
                                <span class="todo-content">{ content }</span>
                            </div>
                        }
                    }).collect::<Html>()
                }
            </div>
        </div>
    }
}

/// Render AskUserQuestion with question cards and options
fn render_askuserquestion_tool(input: &Value) -> Html {
    let questions = input
        .get("questions")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let answers = input.get("answers").and_then(|v| v.as_object());

    html! {
        <div class="tool-use askuserquestion-tool">
            <div class="tool-use-header">
                <span class="tool-icon">{ "❓" }</span>
                <span class="tool-name">{ "AskUserQuestion" }</span>
                <span class="tool-meta">{ format!("({} question{})", questions.len(), if questions.len() == 1 { "" } else { "s" }) }</span>
            </div>
            <div class="question-list">
                {
                    questions.iter().map(|q| {
                        let header = q.get("header").and_then(|h| h.as_str()).unwrap_or("");
                        let question = q.get("question").and_then(|q| q.as_str()).unwrap_or("");
                        let multi_select = q.get("multiSelect").and_then(|m| m.as_bool()).unwrap_or(false);
                        let options = q.get("options").and_then(|o| o.as_array()).cloned().unwrap_or_default();

                        // Check if this question has an answer
                        let answer = answers.and_then(|a| a.get(question)).and_then(|v| v.as_str());

                        html! {
                            <div class="question-card">
                                <div class="question-header">
                                    {
                                        if !header.is_empty() {
                                            html! { <span class="question-badge">{ header }</span> }
                                        } else {
                                            html! {}
                                        }
                                    }
                                    {
                                        if multi_select {
                                            html! { <span class="multi-select-badge">{ "multi-select" }</span> }
                                        } else {
                                            html! {}
                                        }
                                    }
                                </div>
                                <div class="question-text">{ question }</div>
                                <div class="question-options">
                                    {
                                        options.iter().map(|opt| {
                                            let label = opt.get("label").and_then(|l| l.as_str()).unwrap_or("");
                                            let description = opt.get("description").and_then(|d| d.as_str()).unwrap_or("");

                                            // Check if this option was selected
                                            let is_selected = answer.map(|a| {
                                                // For multi-select, answers are comma-separated
                                                a.split(',').map(|s| s.trim()).any(|s| s == label)
                                            }).unwrap_or(false);

                                            let option_class = if is_selected { "option-item selected" } else { "option-item" };
                                            let icon = if is_selected {
                                                if multi_select { "☑" } else { "●" }
                                            } else if multi_select {
                                                "☐"
                                            } else {
                                                "○"
                                            };

                                            html! {
                                                <div class={option_class}>
                                                    <span class="option-icon">{ icon }</span>
                                                    <div class="option-content">
                                                        <span class="option-label">{ label }</span>
                                                        {
                                                            if !description.is_empty() {
                                                                html! { <span class="option-description">{ description }</span> }
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
                                    if let Some(ans) = answer {
                                        html! {
                                            <div class="question-answer">
                                                <span class="answer-label">{ "Answer: " }</span>
                                                <span class="answer-value">{ ans }</span>
                                            </div>
                                        }
                                    } else {
                                        html! {}
                                    }
                                }
                            </div>
                        }
                    }).collect::<Html>()
                }
            </div>
        </div>
    }
}

/// Render ExitPlanMode with formatted plan and permissions list
fn render_exitplanmode_tool(input: &Value) -> Html {
    let allowed_prompts = input
        .get("allowedPrompts")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    html! {
        <div class="tool-use exitplanmode-tool">
            <div class="tool-use-header">
                <span class="tool-icon">{ "📋" }</span>
                <span class="tool-name">{ "Plan Complete" }</span>
            </div>
            {
                if !allowed_prompts.is_empty() {
                    html! {
                        <div class="permissions-section">
                            <div class="permissions-header">{ "Requested Permissions:" }</div>
                            <div class="permissions-list">
                                {
                                    allowed_prompts.iter().map(|p| {
                                        let tool = p.get("tool").and_then(|t| t.as_str()).unwrap_or("Unknown");
                                        let prompt = p.get("prompt").and_then(|p| p.as_str()).unwrap_or("");
                                        html! {
                                            <div class="permission-item">
                                                <span class="permission-bullet">{ "•" }</span>
                                                <span class="permission-tool">{ tool }</span>
                                                <span class="permission-separator">{ ": " }</span>
                                                <span class="permission-prompt">{ prompt }</span>
                                            </div>
                                        }
                                    }).collect::<Html>()
                                }
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

/// Render Bash command with syntax highlighting
fn render_bash_tool(input: &Value) -> Html {
    let command = input.get("command").and_then(|v| v.as_str()).unwrap_or("");
    let description = input.get("description").and_then(|v| v.as_str());
    let timeout = input.get("timeout").and_then(|v| v.as_u64());
    let background = input
        .get("run_in_background")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    // Format timeout with appropriate units (ms for <1s, seconds, minutes)
    let timeout_str = timeout.map(format_duration);

    html! {
        <div class="tool-use bash-tool">
            <div class="tool-use-header">
                <span class="tool-icon">{ "$" }</span>
                <span class="tool-name">{ "Bash" }</span>
                <code class="bash-command-inline">{ command }</code>
                <span class="tool-header-spacer"></span>
                {
                    if background {
                        html! { <span class="tool-badge background">{ "background" }</span> }
                    } else {
                        html! {}
                    }
                }
                {
                    if let Some(t) = timeout_str {
                        html! { <span class="tool-meta timeout">{ t }</span> }
                    } else {
                        html! {}
                    }
                }
            </div>
            {
                if let Some(desc) = description {
                    html! { <div class="bash-description">{ desc }</div> }
                } else {
                    html! {}
                }
            }
        </div>
    }
}

/// Render Read tool with file path and range info
fn render_read_tool(input: &Value) -> Html {
    let file_path = input
        .get("file_path")
        .and_then(|v| v.as_str())
        .unwrap_or("?");
    let offset = input.get("offset").and_then(|v| v.as_i64());
    let limit = input.get("limit").and_then(|v| v.as_i64());

    let range_info = match (offset, limit) {
        (Some(o), Some(l)) => Some(format!("lines {}-{}", o, o + l)),
        (Some(o), None) => Some(format!("from line {}", o)),
        (None, Some(l)) => Some(format!("first {} lines", l)),
        _ => None,
    };

    html! {
        <div class="tool-use read-tool">
            <div class="tool-use-header">
                <span class="tool-icon">{ "📖" }</span>
                <span class="tool-name">{ "Read" }</span>
                <span class="read-file-path">{ file_path }</span>
                {
                    if let Some(range) = range_info {
                        html! { <span class="tool-meta">{ range }</span> }
                    } else {
                        html! {}
                    }
                }
            </div>
        </div>
    }
}

/// Render Glob tool with pattern and path
fn render_glob_tool(input: &Value) -> Html {
    let pattern = input.get("pattern").and_then(|v| v.as_str()).unwrap_or("?");
    let path = input.get("path").and_then(|v| v.as_str());

    html! {
        <div class="tool-use glob-tool">
            <div class="tool-use-header">
                <span class="tool-icon">{ "🔍" }</span>
                <span class="tool-name">{ "Glob" }</span>
                <code class="glob-pattern-inline">{ pattern }</code>
            </div>
            {
                if let Some(p) = path {
                    html! { <div class="glob-path">{ format!("in {}", p) }</div> }
                } else {
                    html! {}
                }
            }
        </div>
    }
}

/// Render Grep tool with pattern, options, and path
fn render_grep_tool(input: &Value) -> Html {
    let pattern = input.get("pattern").and_then(|v| v.as_str()).unwrap_or("?");
    let path = input.get("path").and_then(|v| v.as_str());
    let glob = input.get("glob").and_then(|v| v.as_str());
    let file_type = input.get("type").and_then(|v| v.as_str());
    let case_insensitive = input.get("-i").and_then(|v| v.as_bool()).unwrap_or(false);

    html! {
        <div class="tool-use grep-tool">
            <div class="tool-use-header">
                <span class="tool-icon">{ "🔎" }</span>
                <span class="tool-name">{ "Grep" }</span>
                <code class="grep-pattern-inline">{ format!("/{}/", pattern) }</code>
                {
                    if case_insensitive {
                        html! { <span class="tool-badge">{ "-i" }</span> }
                    } else {
                        html! {}
                    }
                }
            </div>
            <div class="grep-options">
                {
                    if let Some(g) = glob {
                        html! { <span class="grep-option">{ format!("--glob={}", g) }</span> }
                    } else {
                        html! {}
                    }
                }
                {
                    if let Some(t) = file_type {
                        html! { <span class="grep-option">{ format!("--type={}", t) }</span> }
                    } else {
                        html! {}
                    }
                }
                {
                    if let Some(p) = path {
                        html! { <span class="grep-option">{ format!("in {}", p) }</span> }
                    } else {
                        html! {}
                    }
                }
            </div>
        </div>
    }
}

/// Render Task tool with agent type and description
fn render_task_tool(input: &Value) -> Html {
    let description = input
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("?");
    let agent_type = input
        .get("subagent_type")
        .and_then(|v| v.as_str())
        .unwrap_or("agent");
    let background = input
        .get("run_in_background")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    html! {
        <div class="tool-use task-tool">
            <div class="tool-use-header">
                <span class="tool-icon">{ "🤖" }</span>
                <span class="tool-name">{ "Task" }</span>
                <span class="task-agent-type">{ agent_type }</span>
                {
                    if background {
                        html! { <span class="tool-badge background">{ "background" }</span> }
                    } else {
                        html! {}
                    }
                }
            </div>
            <div class="task-description">{ description }</div>
        </div>
    }
}

/// Render WebFetch tool with URL
fn render_webfetch_tool(input: &Value) -> Html {
    let url = input.get("url").and_then(|v| v.as_str()).unwrap_or("?");
    let prompt = input.get("prompt").and_then(|v| v.as_str());

    html! {
        <div class="tool-use webfetch-tool">
            <div class="tool-use-header">
                <span class="tool-icon">{ "🌐" }</span>
                <span class="tool-name">{ "WebFetch" }</span>
            </div>
            <div class="webfetch-url">
                <a href={url.to_string()} target="_blank" rel="noopener noreferrer">{ url }</a>
            </div>
            {
                if let Some(p) = prompt {
                    html! { <div class="webfetch-prompt">{ truncate_str(p, 100) }</div> }
                } else {
                    html! {}
                }
            }
        </div>
    }
}

/// Render WebSearch tool with query
fn render_websearch_tool(input: &Value) -> Html {
    let query = input.get("query").and_then(|v| v.as_str()).unwrap_or("?");

    html! {
        <div class="tool-use websearch-tool">
            <div class="tool-use-header">
                <span class="tool-icon">{ "🔍" }</span>
                <span class="tool-name">{ "WebSearch" }</span>
            </div>
            <div class="websearch-query">{ format!("\"{}\"", query) }</div>
        </div>
    }
}

/// Render the Edit tool with a proper diff view
fn render_edit_tool_diff(input: &Value) -> Html {
    let file_path = input
        .get("file_path")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown file");
    let old_string = input
        .get("old_string")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let new_string = input
        .get("new_string")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let replace_all = input
        .get("replace_all")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    // Generate diff lines
    let diff_html = render_diff_lines(old_string, new_string);

    html! {
        <div class="tool-use edit-tool">
            <div class="tool-use-header">
                <span class="tool-icon">{ "✏️" }</span>
                <span class="tool-name">{ "Edit" }</span>
                <span class="edit-file-path">{ file_path }</span>
                {
                    if replace_all {
                        html! { <span class="edit-replace-all">{ "(replace all)" }</span> }
                    } else {
                        html! {}
                    }
                }
            </div>
            <div class="diff-container">
                { diff_html }
            </div>
        </div>
    }
}

/// Render the Write tool with file content preview
fn render_write_tool(input: &Value) -> Html {
    let file_path = input
        .get("file_path")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown file");
    let content = input.get("content").and_then(|v| v.as_str()).unwrap_or("");

    // Show a preview of the content (first N lines)
    let preview_lines: Vec<&str> = content.lines().take(20).collect();
    let total_lines = content.lines().count();
    let truncated = total_lines > 20;

    html! {
        <div class="tool-use write-tool">
            <div class="tool-use-header">
                <span class="tool-icon">{ "📝" }</span>
                <span class="tool-name">{ "Write" }</span>
                <span class="write-file-path">{ file_path }</span>
                <span class="write-size">{ format!("({} lines, {} bytes)", total_lines, content.len()) }</span>
            </div>
            <div class="write-preview">
                <pre class="write-content">
                    {
                        preview_lines.iter().enumerate().map(|(i, line)| {
                            html! {
                                <div class="write-line">
                                    <span class="line-number">{ format!("{:>4}", i + 1) }</span>
                                    <span class="line-content">{ *line }</span>
                                </div>
                            }
                        }).collect::<Html>()
                    }
                    {
                        if truncated {
                            html! {
                                <div class="write-truncated">
                                    { format!("... {} more lines", total_lines - 20) }
                                </div>
                            }
                        } else {
                            html! {}
                        }
                    }
                </pre>
            </div>
        </div>
    }
}

/// Generate diff view HTML from old and new strings
fn render_diff_lines(old_string: &str, new_string: &str) -> Html {
    let old_lines: Vec<&str> = old_string.lines().collect();
    let new_lines: Vec<&str> = new_string.lines().collect();

    // Simple line-by-line diff using longest common subsequence approach
    let diff = compute_line_diff(&old_lines, &new_lines);

    html! {
        <div class="diff-view">
            {
                diff.iter().map(|change| {
                    match change {
                        DiffLine::Context(line) => html! {
                            <div class="diff-line context">
                                <span class="diff-marker">{ " " }</span>
                                <span class="diff-content">{ *line }</span>
                            </div>
                        },
                        DiffLine::Removed(line) => html! {
                            <div class="diff-line removed">
                                <span class="diff-marker">{ "-" }</span>
                                <span class="diff-content">{ *line }</span>
                            </div>
                        },
                        DiffLine::Added(line) => html! {
                            <div class="diff-line added">
                                <span class="diff-marker">{ "+" }</span>
                                <span class="diff-content">{ *line }</span>
                            </div>
                        },
                    }
                }).collect::<Html>()
            }
        </div>
    }
}

#[derive(Debug, Clone)]
enum DiffLine<'a> {
    Context(&'a str),
    Removed(&'a str),
    Added(&'a str),
}

/// Compute a line-based diff between old and new content
fn compute_line_diff<'a>(old_lines: &[&'a str], new_lines: &[&'a str]) -> Vec<DiffLine<'a>> {
    // Use a simple LCS-based diff algorithm
    let lcs = longest_common_subsequence(old_lines, new_lines);

    let mut result = Vec::new();
    let mut old_idx = 0;
    let mut new_idx = 0;
    let mut lcs_idx = 0;

    while old_idx < old_lines.len() || new_idx < new_lines.len() {
        if lcs_idx < lcs.len() {
            let (lcs_old, lcs_new) = lcs[lcs_idx];

            // Add removed lines before the next common line
            while old_idx < lcs_old {
                result.push(DiffLine::Removed(old_lines[old_idx]));
                old_idx += 1;
            }

            // Add added lines before the next common line
            while new_idx < lcs_new {
                result.push(DiffLine::Added(new_lines[new_idx]));
                new_idx += 1;
            }

            // Add the common line as context
            result.push(DiffLine::Context(old_lines[old_idx]));
            old_idx += 1;
            new_idx += 1;
            lcs_idx += 1;
        } else {
            // No more common lines - add remaining as removed/added
            while old_idx < old_lines.len() {
                result.push(DiffLine::Removed(old_lines[old_idx]));
                old_idx += 1;
            }
            while new_idx < new_lines.len() {
                result.push(DiffLine::Added(new_lines[new_idx]));
                new_idx += 1;
            }
        }
    }

    result
}

/// Compute longest common subsequence indices for line diff
fn longest_common_subsequence(old: &[&str], new: &[&str]) -> Vec<(usize, usize)> {
    let m = old.len();
    let n = new.len();

    if m == 0 || n == 0 {
        return Vec::new();
    }

    // Build LCS length table
    let mut dp = vec![vec![0usize; n + 1]; m + 1];

    for i in 1..=m {
        for j in 1..=n {
            if old[i - 1] == new[j - 1] {
                dp[i][j] = dp[i - 1][j - 1] + 1;
            } else {
                dp[i][j] = dp[i - 1][j].max(dp[i][j - 1]);
            }
        }
    }

    // Backtrack to find LCS indices
    let mut result = Vec::new();
    let mut i = m;
    let mut j = n;

    while i > 0 && j > 0 {
        if old[i - 1] == new[j - 1] {
            result.push((i - 1, j - 1));
            i -= 1;
            j -= 1;
        } else if dp[i - 1][j] > dp[i][j - 1] {
            i -= 1;
        } else {
            j -= 1;
        }
    }

    result.reverse();
    result
}

fn format_tool_input(tool_name: &str, input: &Value) -> String {
    match tool_name {
        "Bash" => input
            .get("command")
            .and_then(|v| v.as_str())
            .map(|s| format!("$ {}", s))
            .unwrap_or_else(|| format_generic_input(input)),
        "Read" => {
            let path = input
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let mut result = path.to_string();
            if let Some(offset) = input.get("offset").and_then(|v| v.as_i64()) {
                if let Some(limit) = input.get("limit").and_then(|v| v.as_i64()) {
                    result.push_str(&format!(" [lines {}-{}]", offset, offset + limit));
                }
            }
            result
        }
        "Edit" => {
            let path = input
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let old_len = input
                .get("old_string")
                .and_then(|v| v.as_str())
                .map(|s| s.len())
                .unwrap_or(0);
            let new_len = input
                .get("new_string")
                .and_then(|v| v.as_str())
                .map(|s| s.len())
                .unwrap_or(0);
            format!("{}\n-{} chars +{} chars", path, old_len, new_len)
        }
        "Write" => {
            let path = input
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let content_len = input
                .get("content")
                .and_then(|v| v.as_str())
                .map(|s| s.len())
                .unwrap_or(0);
            format!("{} ({} bytes)", path, content_len)
        }
        "Glob" => {
            let pattern = input.get("pattern").and_then(|v| v.as_str()).unwrap_or("?");
            let path = input.get("path").and_then(|v| v.as_str());
            match path {
                Some(p) => format!("{} in {}", pattern, p),
                None => pattern.to_string(),
            }
        }
        "Grep" => {
            let pattern = input.get("pattern").and_then(|v| v.as_str()).unwrap_or("?");
            let path = input.get("path").and_then(|v| v.as_str());
            let glob = input.get("glob").and_then(|v| v.as_str());
            let mut result = format!("/{}/", pattern);
            if let Some(g) = glob {
                result.push_str(&format!(" --glob={}", g));
            }
            if let Some(p) = path {
                result.push_str(&format!(" in {}", p));
            }
            result
        }
        "Task" => {
            let desc = input
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let agent = input
                .get("subagent_type")
                .and_then(|v| v.as_str())
                .unwrap_or("agent");
            format!("[{}] {}", agent, desc)
        }
        "WebFetch" => input
            .get("url")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| format_generic_input(input)),
        "WebSearch" => input
            .get("query")
            .and_then(|v| v.as_str())
            .map(|s| format!("\"{}\"", s))
            .unwrap_or_else(|| format_generic_input(input)),
        "TodoWrite" => input
            .get("todos")
            .and_then(|v| v.as_array())
            .map(|arr| format!("{} items", arr.len()))
            .unwrap_or_else(|| format_generic_input(input)),
        "AskUserQuestion" => input
            .get("questions")
            .and_then(|v| v.as_array())
            .map(|arr| {
                format!(
                    "{} question{}",
                    arr.len(),
                    if arr.len() == 1 { "" } else { "s" }
                )
            })
            .unwrap_or_else(|| format_generic_input(input)),
        _ => format_generic_input(input),
    }
}

fn format_generic_input(input: &Value) -> String {
    if let Some(obj) = input.as_object() {
        let parts: Vec<String> = obj
            .iter()
            .filter(|(_, v)| v.is_string() || v.is_number() || v.is_boolean())
            .take(3)
            .map(|(k, v)| {
                let val = match v {
                    Value::String(s) => truncate_str(s, 40).to_string(),
                    other => other.to_string(),
                };
                format!("{}={}", k, val)
            })
            .collect();
        if parts.is_empty() {
            "...".to_string()
        } else {
            parts.join(", ")
        }
    } else {
        "...".to_string()
    }
}

fn truncate_str(s: &str, max_len: usize) -> &str {
    if s.len() <= max_len {
        s
    } else {
        // Find a safe UTF-8 boundary to avoid panics on multi-byte characters
        let mut end = max_len;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        &s[..end]
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

    // Result message is just a compact stats bar (cost shown in session header instead)
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

/// Request body for logging raw messages
#[derive(Serialize)]
struct LogRawMessageRequest {
    session_id: Option<Uuid>,
    message_content: Value,
    message_source: String,
    render_reason: Option<String>,
}

/// Log a raw message to the backend for debugging
fn log_raw_message(session_id: Option<Uuid>, json: &str, reason: &str) {
    // Parse the JSON content or wrap as string if invalid
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

/// Props for raw message renderer with logging
#[derive(Properties, PartialEq)]
pub struct RawMessageRendererProps {
    pub json: String,
    #[prop_or_default]
    pub session_id: Option<Uuid>,
}

/// Component that renders raw JSON and logs it to the backend
#[function_component(RawMessageRenderer)]
pub fn raw_message_renderer(props: &RawMessageRendererProps) -> Html {
    let json = props.json.clone();
    let session_id = props.session_id;

    // Log the raw message once when first rendered
    use_effect_with(json.clone(), move |json| {
        // Determine the reason for raw rendering
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
    // Try to pretty-print, otherwise show as-is
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

fn shorten_model_name(model: &str) -> Option<String> {
    // Skip synthetic/placeholder model names
    if model.is_empty() || model.starts_with('<') {
        return None;
    }

    Some(if model.contains("opus") {
        "Opus".to_string()
    } else if model.contains("sonnet") {
        "Sonnet".to_string()
    } else if model.contains("haiku") {
        "Haiku".to_string()
    } else {
        model.split('-').next().unwrap_or(model).to_string()
    })
}

fn format_duration(ms: u64) -> String {
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

    // Error message tests

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
}
