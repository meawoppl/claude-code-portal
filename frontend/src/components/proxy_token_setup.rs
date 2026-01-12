//! Proxy Token Setup Component
//!
//! Displays instructions for setting up the proxy CLI with a pre-authenticated token.

use crate::components::CopyCommand;
use crate::utils;
use gloo::utils::window;
use gloo_net::http::Request;
use shared::{CreateProxyTokenRequest, CreateProxyTokenResponse};
use yew::prelude::*;

#[derive(Clone, PartialEq)]
enum TokenState {
    Loading,
    HasToken(CreateProxyTokenResponse),
    Error(String),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Platform {
    Linux,
    MacOS,
    Windows,
}

impl Platform {
    fn label(&self) -> &'static str {
        match self {
            Platform::Linux => "Linux",
            Platform::MacOS => "macOS",
            Platform::Windows => "Windows",
        }
    }
}

fn detect_platform() -> Platform {
    let user_agent = window()
        .navigator()
        .user_agent()
        .unwrap_or_default()
        .to_lowercase();

    if user_agent.contains("win") {
        Platform::Windows
    } else if user_agent.contains("mac") {
        Platform::MacOS
    } else {
        Platform::Linux
    }
}

#[function_component(ProxyTokenSetup)]
pub fn proxy_token_setup() -> Html {
    let token_state = use_state(|| TokenState::Loading);
    let detected = detect_platform();
    let selected_platform = use_state(|| detected);

    // Get the base URL for the install script
    let base_url = web_sys::window()
        .and_then(|w| w.location().origin().ok())
        .unwrap_or_else(|| "http://localhost:3000".to_string());

    // Auto-generate token on mount
    {
        let token_state = token_state.clone();

        use_effect_with((), move |_| {
            let token_state = token_state.clone();

            wasm_bindgen_futures::spawn_local(async move {
                let api_endpoint = utils::api_url("/api/proxy-tokens");

                let request_body = CreateProxyTokenRequest {
                    name: format!(
                        "CLI Setup - {}",
                        js_sys::Date::new_0().to_locale_string("en-US", &js_sys::Object::new())
                    ),
                    expires_in_days: 30,
                };

                match Request::post(&api_endpoint)
                    .json(&request_body)
                    .expect("Failed to serialize request")
                    .send()
                    .await
                {
                    Ok(response) => {
                        if response.ok() {
                            if let Ok(data) = response.json::<CreateProxyTokenResponse>().await {
                                token_state.set(TokenState::HasToken(data));
                            } else {
                                token_state
                                    .set(TokenState::Error("Failed to parse response".to_string()));
                            }
                        } else if response.status() == 401 {
                            // Session invalid - redirect to logout
                            if let Some(window) = web_sys::window() {
                                let _ = window.location().set_href("/api/auth/logout");
                            }
                        } else {
                            token_state.set(TokenState::Error(format!(
                                "Server error: {}",
                                response.status()
                            )));
                        }
                    }
                    Err(e) => {
                        token_state.set(TokenState::Error(format!("Request failed: {:?}", e)));
                    }
                }
            });

            || ()
        });
    }

    let platforms = [Platform::Linux, Platform::MacOS, Platform::Windows];

    match (*token_state).clone() {
        TokenState::Loading => {
            html! {
                <div class="proxy-setup loading">
                    <div class="spinner-small"></div>
                    <span>{ "Generating setup command..." }</span>
                </div>
            }
        }
        TokenState::HasToken(token_response) => {
            // URL-encode the init_url for the query parameter
            let encoded_init_url = js_sys::encode_uri_component(&token_response.init_url);

            // Derive WebSocket URL from current origin (http->ws, https->wss)
            let ws_backend_url = base_url
                .replace("https://", "wss://")
                .replace("http://", "ws://");
            let encoded_backend_url = js_sys::encode_uri_component(&ws_backend_url);

            let install_command = match *selected_platform {
                Platform::Linux | Platform::MacOS => format!(
                    "curl -fsSL \"{}/api/download/install.sh?init_url={}&backend_url={}\" | bash",
                    base_url, encoded_init_url, encoded_backend_url
                ),
                Platform::Windows => format!(
                    "# Download from GitHub releases, then run:\n.\\claude-proxy.exe --init \"{}\" --backend-url \"{}\"",
                    token_response.init_url, ws_backend_url
                ),
            };
            let run_command = match *selected_platform {
                Platform::Linux | Platform::MacOS => "claude-proxy".to_string(),
                Platform::Windows => ".\\claude-proxy.exe".to_string(),
            };

            html! {
                <div class="proxy-setup has-token">
                    <h3>{ "Quick Setup" }</h3>

                    <div class="platform-selector">
                        {for platforms.iter().map(|platform| {
                            let is_selected = *selected_platform == *platform;
                            let platform = *platform;
                            let selected_platform = selected_platform.clone();

                            let class = classes!(
                                "platform-option",
                                is_selected.then_some("selected")
                            );

                            let onclick = Callback::from(move |_| {
                                selected_platform.set(platform);
                            });

                            html! {
                                <button {class} {onclick}>
                                    {platform.label()}
                                </button>
                            }
                        })}
                    </div>

                    <div class="setup-step">
                        <span class="step-number">{ "1" }</span>
                        <div class="step-content">
                            <p class="step-label">{ "Install and initialize:" }</p>
                            <CopyCommand command={install_command} />
                        </div>
                    </div>

                    <div class="setup-step">
                        <span class="step-number">{ "2" }</span>
                        <div class="step-content">
                            <p class="step-label">{ "Start a session:" }</p>
                            <CopyCommand command={run_command} />
                        </div>
                    </div>

                    <div class="setup-notes">
                        <p class="note expiry">
                            <span class="note-icon">{ "!" }</span>
                            { format!("Token expires: {}", format_expiry(&token_response.expires_at)) }
                        </p>
                        {if *selected_platform == Platform::Windows {
                            html! {
                                <p class="note windows-note">
                                    <span class="note-icon">{ "!" }</span>
                                    { "Download Windows binary from " }
                                    <a href="https://github.com/meawoppl/cc-proxy/releases/latest" target="_blank">
                                        { "GitHub releases" }
                                    </a>
                                </p>
                            }
                        } else {
                            html! {}
                        }}
                    </div>
                </div>
            }
        }
        TokenState::Error(error) => {
            html! {
                <div class="proxy-setup error">
                    <h3>{ "Error" }</h3>
                    <p class="error-message">{ error }</p>
                    <p class="setup-description">{ "Close and try again." }</p>
                </div>
            }
        }
    }
}

fn format_expiry(timestamp: &str) -> String {
    use js_sys::Date;

    let parsed = Date::parse(timestamp);
    if parsed.is_nan() {
        return timestamp.to_string();
    }

    let date = Date::new(&wasm_bindgen::JsValue::from_f64(parsed));
    date.to_locale_date_string("en-US", &js_sys::Object::new())
        .as_string()
        .unwrap_or_else(|| timestamp.to_string())
}
