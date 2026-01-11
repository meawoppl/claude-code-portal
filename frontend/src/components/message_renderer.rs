use serde::{Deserialize, Serialize};
use serde_json::Value;
use yew::prelude::*;

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
                    <div class="user-text">{ text }</div>
                </div>
            </div>
        }
    } else if let Some(message) = &msg.message {
        let blocks = message.content.as_ref().cloned().unwrap_or_default();

        // Extract text content for display
        let text_content: String = blocks.iter()
            .filter_map(|block| {
                match block {
                    ContentBlock::Text { text } => Some(text.clone()),
                    _ => None,
                }
            })
            .collect::<Vec<_>>()
            .join("\n");

        // Check if this is a tool result or regular user input
        let has_tool_results = blocks.iter().any(|b| matches!(b, ContentBlock::ToolResult { .. }));

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
                        <div class="user-text">{ text_content }</div>
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

    // For init messages, show a compact one-liner
    if subtype == "init" {
        let model_short = msg.model.as_ref()
            .and_then(|m| shorten_model_name(m))
            .unwrap_or_else(|| "unknown".to_string());

        html! {
            <div class="claude-message system-message compact">
                <span class="message-type-badge system">{ "init" }</span>
                <span class="model-name">{ model_short }</span>
            </div>
        }
    } else {
        html! {
            <div class="claude-message system-message compact">
                <span class="message-type-badge system">{ subtype }</span>
            </div>
        }
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
                            html! { <div class="assistant-text">{ text }</div> }
                        }
                        ContentBlock::ToolUse { id: _, name, input } => {
                            let input_preview = format_tool_input(name, input);
                            html! {
                                <div class="tool-use">
                                    <span class="tool-name">{ name }</span>
                                    <span class="tool-input">{ input_preview }</span>
                                </div>
                            }
                        }
                        ContentBlock::ToolResult { tool_use_id: _, content, is_error } => {
                            let class = if *is_error { "tool-result error" } else { "tool-result" };
                            let text = content.as_deref().unwrap_or("");
                            // Truncate long results
                            let display = if text.len() > 500 {
                                format!("{}...", &text[..500])
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

fn format_tool_input(tool_name: &str, input: &Value) -> String {
    match tool_name {
        "Bash" => {
            input.get("command")
                .and_then(|v| v.as_str())
                .map(|s| format!("$ {}", truncate_str(s, 100)))
                .unwrap_or_else(|| "...".to_string())
        }
        "Read" => {
            input.get("file_path")
                .and_then(|v| v.as_str())
                .map(|s| truncate_str(s, 80).to_string())
                .unwrap_or_else(|| "...".to_string())
        }
        "Edit" => {
            input.get("file_path")
                .and_then(|v| v.as_str())
                .map(|s| truncate_str(s, 80).to_string())
                .unwrap_or_else(|| "...".to_string())
        }
        "Write" => {
            input.get("file_path")
                .and_then(|v| v.as_str())
                .map(|s| truncate_str(s, 80).to_string())
                .unwrap_or_else(|| "...".to_string())
        }
        "Glob" | "Grep" => {
            input.get("pattern")
                .and_then(|v| v.as_str())
                .map(|s| truncate_str(s, 60).to_string())
                .unwrap_or_else(|| "...".to_string())
        }
        "Task" => {
            input.get("description")
                .and_then(|v| v.as_str())
                .map(|s| truncate_str(s, 80).to_string())
                .unwrap_or_else(|| "...".to_string())
        }
        _ => {
            // Generic: try to show first string value
            if let Some(obj) = input.as_object() {
                obj.values()
                    .find_map(|v| v.as_str())
                    .map(|s| truncate_str(s, 60).to_string())
                    .unwrap_or_else(|| "...".to_string())
            } else {
                "...".to_string()
            }
        }
    }
}

fn truncate_str(s: &str, max_len: usize) -> &str {
    if s.len() <= max_len {
        s
    } else {
        &s[..max_len]
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

fn format_cost(cost: f64) -> String {
    if cost < 0.01 {
        format!("${:.4}", cost)
    } else if cost < 1.0 {
        format!("${:.3}", cost)
    } else {
        format!("${:.2}", cost)
    }
}
