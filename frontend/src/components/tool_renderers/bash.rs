use serde_json::Value;
use yew::prelude::*;

use crate::components::message_renderer::format_duration;

pub fn render_bash_tool(input: &Value) -> Html {
    let command = input.get("command").and_then(|v| v.as_str()).unwrap_or("");
    let description = input.get("description").and_then(|v| v.as_str());
    let timeout = input.get("timeout").and_then(|v| v.as_u64());
    let background = input
        .get("run_in_background")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

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
                        html! { <span class="tool-meta timeout">{ format!("timeout={}", t) }</span> }
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
