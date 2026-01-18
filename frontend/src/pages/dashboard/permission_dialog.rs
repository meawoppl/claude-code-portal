//! Permission dialog components for tool authorization and user questions

use std::collections::HashSet;
use web_sys::KeyboardEvent;
use yew::prelude::*;

use super::types::{
    format_permission_input, parse_ask_user_question, AskUserQuestionInput, PendingPermission,
};

/// Props for the PermissionDialog component
#[derive(Properties, PartialEq)]
pub struct PermissionDialogProps {
    /// The pending permission request to display
    pub permission: PendingPermission,
    /// Currently selected option index
    pub selected: usize,
    /// For multi-select questions: which options are selected
    #[prop_or_default]
    pub multi_select_options: HashSet<usize>,
    /// Reference to the dialog for focus management
    pub dialog_ref: NodeRef,
    /// Callback when user navigates up
    pub on_select_up: Callback<()>,
    /// Callback when user navigates down
    pub on_select_down: Callback<()>,
    /// Callback when user confirms selection
    pub on_confirm: Callback<()>,
    /// Callback when user selects and confirms an option by index (for click)
    pub on_select_and_confirm: Callback<usize>,
    /// Callback when user answers a question (for AskUserQuestion)
    pub on_answer: Callback<String>,
    /// Callback to toggle a multi-select option
    pub on_toggle_option: Callback<usize>,
}

/// Permission dialog component - handles both regular permissions and AskUserQuestion
#[function_component(PermissionDialog)]
pub fn permission_dialog(props: &PermissionDialogProps) -> Html {
    let perm = &props.permission;

    // Check if this is an AskUserQuestion
    if perm.tool_name == "AskUserQuestion" {
        if let Some(parsed) = parse_ask_user_question(&perm.input) {
            return render_ask_user_question(props, &parsed);
        }
    }

    // Regular permission dialog
    render_standard_permission(props)
}

/// Render the standard permission dialog (Allow/Deny)
fn render_standard_permission(props: &PermissionDialogProps) -> Html {
    let perm = &props.permission;
    let input_preview = format_permission_input(&perm.tool_name, &perm.input);
    let has_suggestions = !perm.permission_suggestions.is_empty();

    let on_select_up = props.on_select_up.clone();
    let on_select_down = props.on_select_down.clone();
    let on_confirm = props.on_confirm.clone();

    let onkeydown = Callback::from(move |e: KeyboardEvent| match e.key().as_str() {
        "ArrowUp" | "k" => {
            e.prevent_default();
            on_select_up.emit(());
        }
        "ArrowDown" | "j" => {
            e.prevent_default();
            on_select_down.emit(());
        }
        "Enter" | " " => {
            e.prevent_default();
            on_confirm.emit(());
        }
        _ => {}
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
            ref={props.dialog_ref.clone()}
            tabindex="0"
            {onkeydown}
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
                        let is_selected = i == props.selected;
                        let cursor = if is_selected { ">" } else { " " };
                        let item_class = if is_selected {
                            format!("permission-option selected {}", class)
                        } else {
                            format!("permission-option {}", class)
                        };
                        let on_select_and_confirm = props.on_select_and_confirm.clone();
                        let onclick = Callback::from(move |_| {
                            on_select_and_confirm.emit(i);
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

/// Render the AskUserQuestion specialized UI
fn render_ask_user_question(props: &PermissionDialogProps, parsed: &AskUserQuestionInput) -> Html {
    let on_select_up = props.on_select_up.clone();
    let on_select_down = props.on_select_down.clone();
    let on_confirm = props.on_confirm.clone();

    let onkeydown = Callback::from(move |e: KeyboardEvent| match e.key().as_str() {
        "ArrowUp" | "k" => {
            e.prevent_default();
            on_select_up.emit(());
        }
        "ArrowDown" | "j" => {
            e.prevent_default();
            on_select_down.emit(());
        }
        "Enter" | " " => {
            e.prevent_default();
            on_confirm.emit(());
        }
        _ => {}
    });

    html! {
        <div
            class="permission-prompt ask-user-question"
            ref={props.dialog_ref.clone()}
            tabindex="0"
            {onkeydown}
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
                                            props.multi_select_options.contains(&i)
                                        } else {
                                            i == props.selected
                                        };
                                        let item_class = if is_selected {
                                            "question-option selected"
                                        } else {
                                            "question-option"
                                        };
                                        let label_clone = opt.label.clone();
                                        let on_answer = props.on_answer.clone();
                                        let on_toggle = props.on_toggle_option.clone();
                                        let onclick = if is_multi {
                                            Callback::from(move |_| on_toggle.emit(i))
                                        } else {
                                            Callback::from(move |_| on_answer.emit(label_clone.clone()))
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
                                    let multi_select_clone = props.multi_select_options.clone();
                                    let on_answer = props.on_answer.clone();
                                    let onclick = Callback::from(move |_| {
                                        // Build comma-separated answer from selected indices
                                        let answer: String = multi_select_clone
                                            .iter()
                                            .filter_map(|&idx| options_clone.get(idx).map(|o| o.label.clone()))
                                            .collect::<Vec<_>>()
                                            .join(", ");
                                        on_answer.emit(answer);
                                    });
                                    html! {
                                        <button class="submit-answer" {onclick} disabled={props.multi_select_options.is_empty()}>
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
}
