use serde::{Deserialize, Serialize};
use serde_json::Value;
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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ErrorMessage {
    pub message: Option<String>,
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
        content: Option<String>,
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
        Ok(ClaudeMessage::Unknown) | Err(_) => render_raw_json(&props.json),
    }
}

#[derive(Properties, PartialEq)]
pub struct MessageGroupRendererProps {
    pub group: MessageGroup,
}

#[function_component(MessageGroupRenderer)]
pub fn message_group_renderer(props: &MessageGroupRendererProps) -> Html {
    match &props.group {
        MessageGroup::Single(json) => {
            html! { <MessageRenderer json={json.clone()} /> }
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
                    if total_output_tokens > 0 {
                        html! {
                            <span class="usage-badge" title={usage_tooltip}>
                                <span class="token-count">{ format!("{}", total_output_tokens) }</span>
                                <span class="token-label">{ "tokens" }</span>
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
                    <div class="user-text">{ linkify_text(text) }</div>
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
                        <div class="user-text">{ linkify_text(&text_content) }</div>
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
    let message = msg.message.as_deref().unwrap_or("Unknown error");

    html! {
        <div class="claude-message error-message-display">
            <div class="message-header">
                <span class="message-type-badge result error">{ "Error" }</span>
            </div>
            <div class="message-body">
                <div class="error-text">{ message }</div>
            </div>
        </div>
    }
}

fn render_system_message(msg: &SystemMessage) -> Html {
    let subtype = msg.subtype.as_deref().unwrap_or("system");

    // Hide init messages - they're not informative to users
    if subtype == "init" {
        return html! {};
    }

    html! {
        <div class="claude-message system-message compact">
            <span class="message-type-badge system">{ subtype }</span>
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
                                <span class="token-count">{ format!("{}", u.output_tokens.unwrap_or(0)) }</span>
                                <span class="token-label">{ "tokens" }</span>
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
                            html! { <div class="assistant-text">{ linkify_text(text) }</div> }
                        }
                        ContentBlock::ToolUse { id: _, name, input } => {
                            render_tool_use(name, input)
                        }
                        ContentBlock::ToolResult { tool_use_id: _, content, is_error } => {
                            let class = if *is_error { "tool-result error" } else { "tool-result" };
                            let text = content.as_deref().unwrap_or("");
                            // Truncate long results (using safe UTF-8 boundary)
                            let display = if text.len() > 500 {
                                format!("{}...", truncate_str(text, 500))
                            } else {
                                text.to_string()
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

/// Render Bash command with syntax highlighting
fn render_bash_tool(input: &Value) -> Html {
    let command = input.get("command").and_then(|v| v.as_str()).unwrap_or("");
    let description = input.get("description").and_then(|v| v.as_str());
    let timeout = input.get("timeout").and_then(|v| v.as_u64());
    let background = input
        .get("run_in_background")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    html! {
        <div class="tool-use bash-tool">
            <div class="tool-use-header">
                <span class="tool-icon">{ "$" }</span>
                <span class="tool-name">{ "Bash" }</span>
                {
                    if background {
                        html! { <span class="tool-badge background">{ "background" }</span> }
                    } else {
                        html! {}
                    }
                }
                {
                    if let Some(t) = timeout {
                        html! { <span class="tool-meta">{ format!("timeout: {}ms", t) }</span> }
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
            <pre class="bash-command">{ format!("$ {}", command) }</pre>
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
                {
                    if let Some(range) = range_info {
                        html! { <span class="tool-meta">{ range }</span> }
                    } else {
                        html! {}
                    }
                }
            </div>
            <div class="file-path">{ file_path }</div>
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
            </div>
            <div class="glob-pattern">{ pattern }</div>
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
                {
                    if case_insensitive {
                        html! { <span class="tool-badge">{ "-i" }</span> }
                    } else {
                        html! {}
                    }
                }
            </div>
            <div class="grep-pattern">{ format!("/{}/", pattern) }</div>
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

/// Represents a segment of text that may or may not be a URL
#[derive(Debug, Clone, PartialEq)]
pub enum TextSegment {
    Text(String),
    Url(String),
}

/// Characters that can appear in a URL path/query but shouldn't end a URL
const URL_END_CHARS: &[char] = &['.', ',', ')', ']', '>', ';', ':', '!', '?', '"', '\''];

/// Parse text and extract URLs, returning segments of plain text and URLs
pub fn parse_urls(text: &str) -> Vec<TextSegment> {
    let mut segments = Vec::new();
    let mut remaining = text;

    while !remaining.is_empty() {
        // Find the next URL start
        let url_start = find_url_start(remaining);

        match url_start {
            Some((prefix, url_begin)) => {
                // Add any text before the URL
                if !prefix.is_empty() {
                    segments.push(TextSegment::Text(prefix.to_string()));
                }

                // Extract the URL
                let after_prefix = &remaining[prefix.len()..];
                let url_end = find_url_end(after_prefix);
                let url = &after_prefix[..url_end];

                // Trim trailing punctuation that's likely not part of the URL
                let url = trim_url_end(url);

                if !url.is_empty() && is_valid_url(url) {
                    segments.push(TextSegment::Url(url.to_string()));
                    remaining = &remaining[prefix.len() + url.len()..];
                } else {
                    // Not a valid URL, treat as text
                    segments.push(TextSegment::Text(url_begin.to_string()));
                    remaining = &remaining[prefix.len() + url_begin.len()..];
                }
            }
            None => {
                // No more URLs, add remaining text
                segments.push(TextSegment::Text(remaining.to_string()));
                break;
            }
        }
    }

    // Merge adjacent text segments
    merge_text_segments(segments)
}

/// Find the start of a URL in text, returns (prefix_before_url, url_start_pattern)
fn find_url_start(text: &str) -> Option<(&str, &str)> {
    let patterns = ["https://", "http://"];

    let mut earliest: Option<(usize, &str)> = None;

    for pattern in patterns {
        if let Some(pos) = text.find(pattern) {
            match earliest {
                None => earliest = Some((pos, pattern)),
                Some((earliest_pos, _)) if pos < earliest_pos => {
                    earliest = Some((pos, pattern));
                }
                _ => {}
            }
        }
    }

    earliest.map(|(pos, pattern)| (&text[..pos], pattern))
}

/// Find the end of a URL (where it stops being URL-like)
fn find_url_end(text: &str) -> usize {
    let mut end = 0;
    let chars = text.chars().peekable();
    let mut paren_depth = 0;
    let mut bracket_depth = 0;

    for c in chars {
        match c {
            // Whitespace ends URL
            ' ' | '\t' | '\n' | '\r' => break,
            // Track parentheses for URLs like Wikipedia links
            '(' => {
                paren_depth += 1;
                end += c.len_utf8();
            }
            ')' => {
                if paren_depth > 0 {
                    paren_depth -= 1;
                    end += c.len_utf8();
                } else {
                    break;
                }
            }
            // Track brackets
            '[' => {
                bracket_depth += 1;
                end += c.len_utf8();
            }
            ']' => {
                if bracket_depth > 0 {
                    bracket_depth -= 1;
                    end += c.len_utf8();
                } else {
                    break;
                }
            }
            // Common URL-safe characters
            'a'..='z'
            | 'A'..='Z'
            | '0'..='9'
            | '-'
            | '_'
            | '.'
            | '~'
            | '/'
            | '?'
            | '#'
            | '&'
            | '='
            | '+'
            | '%'
            | '@'
            | ':'
            | '!'
            | '$'
            | '\''
            | '*'
            | ','
            | ';' => {
                end += c.len_utf8();
            }
            // Stop on other characters
            _ => break,
        }
    }

    end
}

/// Trim trailing punctuation that's commonly not part of URLs
/// Handles balanced parentheses (e.g., Wikipedia links)
fn trim_url_end(url: &str) -> &str {
    let mut url = url;

    while let Some(c) = url.chars().last() {
        // For closing parens/brackets, only trim if unbalanced
        if c == ')' {
            let open_count = url.chars().filter(|&ch| ch == '(').count();
            let close_count = url.chars().filter(|&ch| ch == ')').count();
            if close_count > open_count {
                url = &url[..url.len() - c.len_utf8()];
                continue;
            } else {
                break;
            }
        }
        if c == ']' {
            let open_count = url.chars().filter(|&ch| ch == '[').count();
            let close_count = url.chars().filter(|&ch| ch == ']').count();
            if close_count > open_count {
                url = &url[..url.len() - c.len_utf8()];
                continue;
            } else {
                break;
            }
        }
        // Trim other trailing punctuation
        if URL_END_CHARS.contains(&c) && c != ')' && c != ']' {
            url = &url[..url.len() - c.len_utf8()];
        } else {
            break;
        }
    }
    url
}

/// Check if a string looks like a valid URL
fn is_valid_url(url: &str) -> bool {
    // Must start with http:// or https://
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return false;
    }

    // Must have something after the protocol
    let after_protocol = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or("");

    // Must have at least a domain-like part
    if after_protocol.is_empty() {
        return false;
    }

    // Should have at least one dot in domain (or be localhost)
    let domain_end = after_protocol.find('/').unwrap_or(after_protocol.len());
    let domain = &after_protocol[..domain_end];

    domain.contains('.') || domain.starts_with("localhost")
}

/// Merge adjacent Text segments
fn merge_text_segments(segments: Vec<TextSegment>) -> Vec<TextSegment> {
    let mut result = Vec::new();
    let mut current_text = String::new();

    for segment in segments {
        match segment {
            TextSegment::Text(t) => {
                current_text.push_str(&t);
            }
            TextSegment::Url(u) => {
                if !current_text.is_empty() {
                    result.push(TextSegment::Text(std::mem::take(&mut current_text)));
                }
                result.push(TextSegment::Url(u));
            }
        }
    }

    if !current_text.is_empty() {
        result.push(TextSegment::Text(current_text));
    }

    result
}

/// Render text with URLs converted to clickable links
pub fn linkify_text(text: &str) -> Html {
    let segments = parse_urls(text);

    html! {
        <>
            {
                segments.into_iter().map(|segment| {
                    match segment {
                        TextSegment::Text(t) => html! { <>{ t }</> },
                        TextSegment::Url(url) => html! {
                            <a href={url.clone()} target="_blank" rel="noopener noreferrer" class="message-link">
                                { &url }
                            </a>
                        },
                    }
                }).collect::<Html>()
            }
        </>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_urls() {
        let result = parse_urls("Hello world, no links here!");
        assert_eq!(
            result,
            vec![TextSegment::Text("Hello world, no links here!".to_string())]
        );
    }

    #[test]
    fn test_single_https_url() {
        let result = parse_urls("Check out https://example.com for more info.");
        assert_eq!(
            result,
            vec![
                TextSegment::Text("Check out ".to_string()),
                TextSegment::Url("https://example.com".to_string()),
                TextSegment::Text(" for more info.".to_string()),
            ]
        );
    }

    #[test]
    fn test_single_http_url() {
        let result = parse_urls("Visit http://example.com today");
        assert_eq!(
            result,
            vec![
                TextSegment::Text("Visit ".to_string()),
                TextSegment::Url("http://example.com".to_string()),
                TextSegment::Text(" today".to_string()),
            ]
        );
    }

    #[test]
    fn test_url_with_path() {
        let result = parse_urls("See https://example.com/path/to/page.html for details");
        assert_eq!(
            result,
            vec![
                TextSegment::Text("See ".to_string()),
                TextSegment::Url("https://example.com/path/to/page.html".to_string()),
                TextSegment::Text(" for details".to_string()),
            ]
        );
    }

    #[test]
    fn test_url_with_query_params() {
        let result = parse_urls("Link: https://example.com/search?q=test&page=1");
        assert_eq!(
            result,
            vec![
                TextSegment::Text("Link: ".to_string()),
                TextSegment::Url("https://example.com/search?q=test&page=1".to_string()),
            ]
        );
    }

    #[test]
    fn test_url_with_fragment() {
        let result = parse_urls("Go to https://example.com/page#section");
        assert_eq!(
            result,
            vec![
                TextSegment::Text("Go to ".to_string()),
                TextSegment::Url("https://example.com/page#section".to_string()),
            ]
        );
    }

    #[test]
    fn test_url_at_start() {
        let result = parse_urls("https://example.com is the site");
        assert_eq!(
            result,
            vec![
                TextSegment::Url("https://example.com".to_string()),
                TextSegment::Text(" is the site".to_string()),
            ]
        );
    }

    #[test]
    fn test_url_at_end() {
        let result = parse_urls("The site is https://example.com");
        assert_eq!(
            result,
            vec![
                TextSegment::Text("The site is ".to_string()),
                TextSegment::Url("https://example.com".to_string()),
            ]
        );
    }

    #[test]
    fn test_multiple_urls() {
        let result = parse_urls("Check https://one.com and https://two.com for info");
        assert_eq!(
            result,
            vec![
                TextSegment::Text("Check ".to_string()),
                TextSegment::Url("https://one.com".to_string()),
                TextSegment::Text(" and ".to_string()),
                TextSegment::Url("https://two.com".to_string()),
                TextSegment::Text(" for info".to_string()),
            ]
        );
    }

    #[test]
    fn test_url_with_trailing_period() {
        let result = parse_urls("Visit https://example.com.");
        assert_eq!(
            result,
            vec![
                TextSegment::Text("Visit ".to_string()),
                TextSegment::Url("https://example.com".to_string()),
                TextSegment::Text(".".to_string()),
            ]
        );
    }

    #[test]
    fn test_url_with_trailing_comma() {
        let result = parse_urls("See https://example.com, or https://other.com");
        assert_eq!(
            result,
            vec![
                TextSegment::Text("See ".to_string()),
                TextSegment::Url("https://example.com".to_string()),
                TextSegment::Text(", or ".to_string()),
                TextSegment::Url("https://other.com".to_string()),
            ]
        );
    }

    #[test]
    fn test_url_in_parentheses() {
        let result = parse_urls("More info (https://example.com) here");
        assert_eq!(
            result,
            vec![
                TextSegment::Text("More info (".to_string()),
                TextSegment::Url("https://example.com".to_string()),
                TextSegment::Text(") here".to_string()),
            ]
        );
    }

    #[test]
    fn test_wikipedia_url_with_parens() {
        let result =
            parse_urls("See https://en.wikipedia.org/wiki/Rust_(programming_language) for info");
        assert_eq!(
            result,
            vec![
                TextSegment::Text("See ".to_string()),
                TextSegment::Url(
                    "https://en.wikipedia.org/wiki/Rust_(programming_language)".to_string()
                ),
                TextSegment::Text(" for info".to_string()),
            ]
        );
    }

    #[test]
    fn test_url_with_port() {
        let result = parse_urls("Server at https://localhost:8080/api");
        assert_eq!(
            result,
            vec![
                TextSegment::Text("Server at ".to_string()),
                TextSegment::Url("https://localhost:8080/api".to_string()),
            ]
        );
    }

    #[test]
    fn test_url_with_encoded_chars() {
        let result = parse_urls("Link: https://example.com/path%20with%20spaces");
        assert_eq!(
            result,
            vec![
                TextSegment::Text("Link: ".to_string()),
                TextSegment::Url("https://example.com/path%20with%20spaces".to_string()),
            ]
        );
    }

    #[test]
    fn test_invalid_url_no_domain() {
        let result = parse_urls("Not valid: https://");
        assert_eq!(
            result,
            vec![TextSegment::Text("Not valid: https://".to_string()),]
        );
    }

    #[test]
    fn test_localhost_url() {
        let result = parse_urls("Dev server: http://localhost:3000");
        assert_eq!(
            result,
            vec![
                TextSegment::Text("Dev server: ".to_string()),
                TextSegment::Url("http://localhost:3000".to_string()),
            ]
        );
    }

    #[test]
    fn test_url_followed_by_newline() {
        let result = parse_urls("Link: https://example.com\nNext line");
        assert_eq!(
            result,
            vec![
                TextSegment::Text("Link: ".to_string()),
                TextSegment::Url("https://example.com".to_string()),
                TextSegment::Text("\nNext line".to_string()),
            ]
        );
    }

    #[test]
    fn test_only_url() {
        let result = parse_urls("https://example.com");
        assert_eq!(
            result,
            vec![TextSegment::Url("https://example.com".to_string()),]
        );
    }

    #[test]
    fn test_empty_string() {
        let result = parse_urls("");
        assert_eq!(result, Vec::<TextSegment>::new());
    }

    #[test]
    fn test_url_with_subdomain() {
        let result = parse_urls("API docs: https://api.example.com/v1/docs");
        assert_eq!(
            result,
            vec![
                TextSegment::Text("API docs: ".to_string()),
                TextSegment::Url("https://api.example.com/v1/docs".to_string()),
            ]
        );
    }
}
