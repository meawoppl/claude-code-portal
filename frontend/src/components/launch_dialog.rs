use gloo_net::http::Request;
use shared::LauncherInfo;
use uuid::Uuid;
use wasm_bindgen_futures::spawn_local;
use web_sys::HtmlInputElement;
use yew::prelude::*;

#[derive(Properties, PartialEq)]
pub struct LaunchDialogProps {
    pub on_close: Callback<()>,
}

#[function_component(LaunchDialog)]
pub fn launch_dialog(props: &LaunchDialogProps) -> Html {
    let launchers = use_state(Vec::<LauncherInfo>::new);
    let selected_launcher = use_state(|| None::<Uuid>);
    let working_dir = use_state(String::new);
    let session_name = use_state(String::new);
    let launching = use_state(|| false);
    let error_msg = use_state(|| None::<String>);
    let success_msg = use_state(|| None::<String>);

    // Fetch launchers on mount
    {
        let launchers = launchers.clone();
        let selected_launcher = selected_launcher.clone();
        use_effect_with((), move |_| {
            spawn_local(async move {
                if let Ok(resp) = Request::get("/api/launchers").send().await {
                    if let Ok(data) = resp.json::<Vec<LauncherInfo>>().await {
                        if let Some(first) = data.first() {
                            selected_launcher.set(Some(first.launcher_id));
                        }
                        launchers.set(data);
                    }
                }
            });
            || ()
        });
    }

    let on_dir_input = {
        let working_dir = working_dir.clone();
        Callback::from(move |e: InputEvent| {
            if let Some(input) = e.target_dyn_into::<HtmlInputElement>() {
                working_dir.set(input.value());
            }
        })
    };

    let on_name_input = {
        let session_name = session_name.clone();
        Callback::from(move |e: InputEvent| {
            if let Some(input) = e.target_dyn_into::<HtmlInputElement>() {
                session_name.set(input.value());
            }
        })
    };

    let on_launch = {
        let working_dir = working_dir.clone();
        let session_name = session_name.clone();
        let selected_launcher = selected_launcher.clone();
        let launching = launching.clone();
        let error_msg = error_msg.clone();
        let success_msg = success_msg.clone();
        Callback::from(move |_| {
            let dir = (*working_dir).clone();
            if dir.is_empty() {
                error_msg.set(Some("Working directory is required".to_string()));
                return;
            }

            let name = if session_name.is_empty() {
                None
            } else {
                Some((*session_name).clone())
            };

            let launcher_id = *selected_launcher;
            let launching = launching.clone();
            let error_msg = error_msg.clone();
            let success_msg = success_msg.clone();

            launching.set(true);
            error_msg.set(None);
            success_msg.set(None);

            spawn_local(async move {
                let body = serde_json::json!({
                    "working_directory": dir,
                    "session_name": name,
                    "launcher_id": launcher_id,
                    "claude_args": [],
                });

                match Request::post("/api/launch")
                    .json(&body)
                    .unwrap()
                    .send()
                    .await
                {
                    Ok(resp) if resp.ok() => {
                        success_msg.set(Some("Session launching...".to_string()));
                    }
                    Ok(resp) => {
                        let status = resp.status();
                        let text = resp.text().await.unwrap_or_default();
                        if status == 404 {
                            error_msg.set(Some("No connected launchers".to_string()));
                        } else {
                            error_msg.set(Some(format!("Error {}: {}", status, text)));
                        }
                    }
                    Err(e) => {
                        error_msg.set(Some(format!("Request failed: {}", e)));
                    }
                }
                launching.set(false);
            });
        })
    };

    let on_backdrop = {
        let on_close = props.on_close.clone();
        Callback::from(move |_| on_close.emit(()))
    };

    html! {
        <div class="launch-dialog-backdrop" onclick={on_backdrop}>
            <div class="launch-dialog" onclick={Callback::from(|e: MouseEvent| e.stop_propagation())}>
                <h3>{ "Launch Session" }</h3>

                if launchers.is_empty() {
                    <p class="launch-no-launchers">
                        { "No launchers connected. Run " }
                        <code>{ "claude-portal-launcher" }</code>
                        { " on your machine." }
                    </p>
                } else {
                    <div class="launch-field">
                        <label>{ "Launcher" }</label>
                        <div class="launcher-list">
                            { launchers.iter().map(|launcher| {
                                let is_selected = *selected_launcher == Some(launcher.launcher_id);
                                let card_class = classes!(
                                    "launcher-card",
                                    if is_selected { Some("selected") } else { None },
                                );
                                let launcher_id = launcher.launcher_id;
                                let on_select = {
                                    let selected_launcher = selected_launcher.clone();
                                    Callback::from(move |_: MouseEvent| {
                                        selected_launcher.set(Some(launcher_id));
                                    })
                                };
                                html! {
                                    <div class={card_class} onclick={on_select}>
                                        <div class="launcher-card-header">
                                            <span class="launcher-card-name">{ &launcher.launcher_name }</span>
                                            <span class="launcher-card-sessions">
                                                { format!("{} running", launcher.running_sessions) }
                                            </span>
                                        </div>
                                        <span class="launcher-card-hostname">{ &launcher.hostname }</span>
                                    </div>
                                }
                            }).collect::<Html>() }
                        </div>
                    </div>

                    <div class="launch-field">
                        <label>{ "Working Directory" }</label>
                        <input
                            type="text"
                            placeholder="/home/user/project"
                            value={(*working_dir).clone()}
                            oninput={on_dir_input}
                        />
                    </div>

                    <div class="launch-field">
                        <label>{ "Session Name (optional)" }</label>
                        <input
                            type="text"
                            placeholder="my-feature"
                            value={(*session_name).clone()}
                            oninput={on_name_input}
                        />
                    </div>

                    if let Some(ref err) = *error_msg {
                        <p class="launch-error">{ err }</p>
                    }

                    if let Some(ref msg) = *success_msg {
                        <p class="launch-success">{ msg }</p>
                    }

                    <button
                        class="launch-button"
                        onclick={on_launch}
                        disabled={*launching}
                    >
                        { if *launching { "Launching..." } else { "Launch" } }
                    </button>
                }
            </div>
        </div>
    }
}
