use serde_json::Value;
use yew::prelude::*;

pub fn render_task_tool(input: &Value) -> Html {
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
