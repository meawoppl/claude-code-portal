mod bash;
mod edit;
mod interactive;
mod search;
mod task;

use serde_json::Value;
use yew::prelude::*;

use self::bash::render_bash_tool;
use self::edit::{render_edit_tool, render_write_tool};
use self::interactive::{
    render_askuserquestion_tool, render_exitplanmode_tool, render_todowrite_tool,
};
use self::search::{
    render_glob_tool, render_grep_tool, render_webfetch_tool, render_websearch_tool,
};
use self::task::render_task_tool;
use super::message_renderer::truncate_str;

/// Render a tool use block with special handling for various tools
pub fn render_tool_use(name: &str, input: &Value) -> Html {
    match name {
        "Edit" => render_edit_tool(input),
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

pub fn format_tool_input(tool_name: &str, input: &Value) -> String {
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
