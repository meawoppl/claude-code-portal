use super::markdown::render_markdown;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use yew::prelude::*;

// Local deserialization types mirroring codex-codes, using Option wrappers
// for lenient parsing (same strategy as message_renderer.rs for Claude).

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CodexEvent {
    #[serde(rename = "thread.started")]
    ThreadStarted {
        thread_id: Option<String>,
    },
    #[serde(rename = "turn.started")]
    TurnStarted {},
    #[serde(rename = "turn.completed")]
    TurnCompleted {
        usage: Option<CodexUsage>,
    },
    #[serde(rename = "turn.failed")]
    TurnFailed {
        error: Option<CodexError>,
    },
    #[serde(rename = "item.started")]
    ItemStarted {
        item: Option<CodexItem>,
    },
    #[serde(rename = "item.updated")]
    ItemUpdated {
        item: Option<CodexItem>,
    },
    #[serde(rename = "item.completed")]
    ItemCompleted {
        item: Option<CodexItem>,
    },
    Error {
        message: Option<String>,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CodexUsage {
    pub input_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CodexError {
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CodexItem {
    #[serde(alias = "agentMessage")]
    AgentMessage {
        id: Option<String>,
        text: Option<String>,
    },
    #[serde(alias = "reasoning")]
    Reasoning {
        id: Option<String>,
        text: Option<String>,
    },
    #[serde(alias = "commandExecution")]
    CommandExecution {
        id: Option<String>,
        command: Option<String>,
        #[serde(alias = "aggregatedOutput")]
        aggregated_output: Option<String>,
        #[serde(alias = "exitCode")]
        exit_code: Option<i32>,
        status: Option<String>,
    },
    #[serde(alias = "fileChange")]
    FileChange {
        id: Option<String>,
        changes: Option<Vec<FileChange>>,
        status: Option<String>,
    },
    #[serde(alias = "mcpToolCall")]
    McpToolCall {
        id: Option<String>,
        server: Option<String>,
        tool: Option<String>,
        arguments: Option<Value>,
        status: Option<String>,
    },
    #[serde(alias = "webSearch")]
    WebSearch {
        id: Option<String>,
        query: Option<String>,
    },
    #[serde(alias = "todoList")]
    TodoList {
        id: Option<String>,
        items: Option<Vec<TodoEntry>>,
    },
    #[serde(alias = "error")]
    Error {
        id: Option<String>,
        message: Option<String>,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChange {
    pub path: Option<String>,
    pub kind: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoEntry {
    pub text: Option<String>,
    pub completed: Option<bool>,
}

// --- Components ---

#[derive(Properties, PartialEq)]
pub struct CodexMessageRendererProps {
    pub json: String,
}

#[function_component(CodexMessageRenderer)]
pub fn codex_message_renderer(props: &CodexMessageRendererProps) -> Html {
    let parsed: Result<CodexEvent, _> = serde_json::from_str(&props.json);

    match parsed {
        Ok(CodexEvent::ThreadStarted { .. }) => html! {},
        Ok(CodexEvent::TurnStarted {}) => html! {},
        Ok(CodexEvent::TurnCompleted { usage }) => render_turn_completed(usage.as_ref()),
        Ok(CodexEvent::TurnFailed { error }) => render_turn_failed(error.as_ref()),
        Ok(CodexEvent::ItemStarted { item }) | Ok(CodexEvent::ItemUpdated { item }) => {
            render_item(item.as_ref(), false)
        }
        Ok(CodexEvent::ItemCompleted { item }) => render_item(item.as_ref(), true),
        Ok(CodexEvent::Error { message }) => render_thread_error(message.as_deref()),
        Ok(CodexEvent::Unknown) | Err(_) => render_raw_codex(&props.json),
    }
}

fn render_item(item: Option<&CodexItem>, completed: bool) -> Html {
    let Some(item) = item else {
        return html! {};
    };
    match item {
        CodexItem::AgentMessage { text, .. } => render_agent_message(text.as_deref()),
        CodexItem::Reasoning { text, .. } => render_reasoning(text.as_deref()),
        CodexItem::CommandExecution {
            command,
            aggregated_output,
            exit_code,
            status,
            ..
        } => render_command_execution(
            command.as_deref(),
            aggregated_output.as_deref(),
            *exit_code,
            status.as_deref(),
            completed,
        ),
        CodexItem::FileChange {
            changes, status, ..
        } => render_file_change(changes.as_deref(), status.as_deref()),
        CodexItem::McpToolCall {
            server,
            tool,
            status,
            ..
        } => render_mcp_tool_call(server.as_deref(), tool.as_deref(), status.as_deref()),
        CodexItem::WebSearch { query, .. } => render_web_search(query.as_deref()),
        CodexItem::TodoList { items, .. } => render_todo_list(items.as_deref()),
        CodexItem::Error { message, .. } => render_item_error(message.as_deref()),
        CodexItem::Unknown => html! {},
    }
}

fn render_agent_message(text: Option<&str>) -> Html {
    let text = text.unwrap_or("");
    if text.is_empty() {
        return html! {};
    }
    html! {
        <div class="claude-message assistant-message">
            <div class="message-header">
                <span class="message-type-badge assistant">{ "Codex" }</span>
            </div>
            <div class="message-body">
                <div class="assistant-text">{ render_markdown(text) }</div>
            </div>
        </div>
    }
}

fn render_reasoning(text: Option<&str>) -> Html {
    let text = text.unwrap_or("");
    if text.is_empty() {
        return html! {};
    }
    html! {
        <div class="claude-message assistant-message">
            <div class="message-body">
                <div class="thinking-block">
                    <span class="thinking-label">{ "reasoning" }</span>
                    <div class="thinking-content">{ text }</div>
                </div>
            </div>
        </div>
    }
}

fn render_command_execution(
    command: Option<&str>,
    output: Option<&str>,
    exit_code: Option<i32>,
    status: Option<&str>,
    completed: bool,
) -> Html {
    let cmd = command.unwrap_or("(unknown command)");
    let out = output.unwrap_or("");

    let status_text = if completed {
        match exit_code {
            Some(0) => "completed".to_string(),
            Some(code) => format!("exit {}", code),
            None => status.unwrap_or("completed").to_string(),
        }
    } else {
        "running...".to_string()
    };

    let is_error = exit_code.is_some_and(|c| c != 0);

    html! {
        <div class="claude-message assistant-message">
            <div class="message-body">
                <div class="tool-use-section">
                    <div class="tool-use-header">
                        <span class="tool-icon">{ "$" }</span>
                        <span class="tool-name">{ "Bash" }</span>
                        <span class="tool-meta">{ &status_text }</span>
                    </div>
                    <pre class="tool-input-content">{ cmd }</pre>
                    {
                        if !out.is_empty() {
                            let class = if is_error { "tool-result error" } else { "tool-result" };
                            html! {
                                <div class={class}>
                                    <pre class="tool-result-content">{ out }</pre>
                                </div>
                            }
                        } else {
                            html! {}
                        }
                    }
                </div>
            </div>
        </div>
    }
}

fn render_file_change(changes: Option<&[FileChange]>, status: Option<&str>) -> Html {
    let changes = changes.unwrap_or(&[]);
    if changes.is_empty() {
        return html! {};
    }

    let status_label = status.unwrap_or("completed");

    html! {
        <div class="claude-message assistant-message">
            <div class="message-body">
                <div class="tool-use-section">
                    <div class="tool-use-header">
                        <span class="tool-icon">{ "\u{1f4dd}" }</span>
                        <span class="tool-name">{ "File Changes" }</span>
                        <span class="tool-meta">{ status_label }</span>
                    </div>
                    <div class="file-changes-list">
                        { for changes.iter().map(|c| {
                            let path = c.path.as_deref().unwrap_or("(unknown)");
                            let kind = c.kind.as_deref().unwrap_or("update");
                            let kind_class = format!("file-change-kind {}", kind);
                            html! {
                                <div class="file-change-entry">
                                    <span class={kind_class}>{ kind }</span>
                                    <span class="file-change-path">{ path }</span>
                                </div>
                            }
                        })}
                    </div>
                </div>
            </div>
        </div>
    }
}

fn render_mcp_tool_call(server: Option<&str>, tool: Option<&str>, status: Option<&str>) -> Html {
    let server = server.unwrap_or("(unknown)");
    let tool = tool.unwrap_or("(unknown)");
    let status = status.unwrap_or("in_progress");

    html! {
        <div class="claude-message assistant-message">
            <div class="message-body">
                <div class="tool-use-section">
                    <div class="tool-use-header">
                        <span class="tool-icon">{ "\u{1f50c}" }</span>
                        <span class="tool-name">{ format!("{} / {}", server, tool) }</span>
                        <span class="tool-meta">{ status }</span>
                    </div>
                </div>
            </div>
        </div>
    }
}

fn render_web_search(query: Option<&str>) -> Html {
    let query = query.unwrap_or("(no query)");
    html! {
        <div class="claude-message assistant-message">
            <div class="message-body">
                <div class="tool-use-section">
                    <div class="tool-use-header">
                        <span class="tool-icon">{ "\u{1f50d}" }</span>
                        <span class="tool-name">{ "Web Search" }</span>
                    </div>
                    <pre class="tool-input-content">{ query }</pre>
                </div>
            </div>
        </div>
    }
}

fn render_todo_list(items: Option<&[TodoEntry]>) -> Html {
    let items = items.unwrap_or(&[]);
    if items.is_empty() {
        return html! {};
    }
    html! {
        <div class="claude-message assistant-message">
            <div class="message-body">
                <div class="tool-use-section">
                    <div class="tool-use-header">
                        <span class="tool-icon">{ "\u{2611}" }</span>
                        <span class="tool-name">{ "Todo List" }</span>
                    </div>
                    <div class="codex-todo-list">
                        { for items.iter().map(|item| {
                            let text = item.text.as_deref().unwrap_or("");
                            let done = item.completed.unwrap_or(false);
                            let marker = if done { "\u{2611}" } else { "\u{2610}" };
                            let class = if done { "codex-todo done" } else { "codex-todo" };
                            html! {
                                <div class={class}>
                                    <span class="codex-todo-marker">{ marker }</span>
                                    <span class="codex-todo-text">{ text }</span>
                                </div>
                            }
                        })}
                    </div>
                </div>
            </div>
        </div>
    }
}

fn render_turn_completed(usage: Option<&CodexUsage>) -> Html {
    let input = usage.and_then(|u| u.input_tokens).unwrap_or(0);
    let output = usage.and_then(|u| u.output_tokens).unwrap_or(0);
    let cached = usage.and_then(|u| u.cached_input_tokens).unwrap_or(0);

    let tooltip = format!("Input: {} | Output: {} | Cached: {}", input, output, cached);

    html! {
        <div class="claude-message result-message success">
            <div class="result-stats-bar">
                <span class="result-status success">{ "\u{2713}" }</span>
                {
                    if input > 0 || output > 0 {
                        html! {
                            <>
                                <span class="stat-item tokens" title={tooltip}>
                                    { format!("{}\u{2193} {}\u{2191}", input, output) }
                                </span>
                            </>
                        }
                    } else {
                        html! {}
                    }
                }
            </div>
        </div>
    }
}

fn render_turn_failed(error: Option<&CodexError>) -> Html {
    let message = error
        .and_then(|e| e.message.as_deref())
        .unwrap_or("Turn failed");

    html! {
        <div class="claude-message error-message-display">
            <div class="message-header">
                <span class="message-type-badge result error">{ "Turn Failed" }</span>
            </div>
            <div class="message-body">
                <div class="error-text">{ message }</div>
            </div>
        </div>
    }
}

fn render_thread_error(message: Option<&str>) -> Html {
    let message = message.unwrap_or("Unknown error");
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

fn render_item_error(message: Option<&str>) -> Html {
    let message = message.unwrap_or("Unknown error");
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

fn render_raw_codex(json: &str) -> Html {
    let display = serde_json::from_str::<Value>(json)
        .ok()
        .and_then(|v| serde_json::to_string_pretty(&v).ok())
        .unwrap_or_else(|| json.to_string());

    html! {
        <div class="claude-message raw-message">
            <div class="message-header">
                <span class="message-type-badge raw">{ "Codex Raw" }</span>
            </div>
            <div class="message-body">
                <pre class="raw-json">{ display }</pre>
            </div>
        </div>
    }
}

/// Check if a Codex message indicates "awaiting" (turn complete or turn failed)
pub fn is_codex_terminal_event(json: &str) -> Option<bool> {
    let val: Value = serde_json::from_str(json).ok()?;
    let event_type = val.get("type")?.as_str()?;
    match event_type {
        "turn.completed" | "turn.failed" => Some(true),
        "item.started" | "item.updated" | "item.completed" | "turn.started" | "thread.started" => {
            Some(false)
        }
        _ => None,
    }
}
