//! Proxy Token Setup Component
//!
//! Displays instructions for setting up the proxy CLI with a pre-authenticated token.

use crate::components::CopyCommand;
use crate::utils;
use gloo_net::http::Request;
use shared::{CreateProxyTokenRequest, CreateProxyTokenResponse, ProxyTokenListResponse};
use yew::prelude::*;

#[derive(Clone, PartialEq)]
enum TokenState {
    Loading,
    NoToken,
    HasToken(CreateProxyTokenResponse),
    Error(String),
}

#[function_component(ProxyTokenSetup)]
pub fn proxy_token_setup() -> Html {
    let token_state = use_state(|| TokenState::Loading);
    let creating = use_state(|| false);

    // Check for existing tokens on mount
    {
        let token_state = token_state.clone();

        use_effect_with((), move |_| {
            let token_state = token_state.clone();

            wasm_bindgen_futures::spawn_local(async move {
                let api_endpoint = utils::api_url("/api/proxy-tokens");

                match Request::get(&api_endpoint).send().await {
                    Ok(response) => {
                        if let Ok(data) = response.json::<ProxyTokenListResponse>().await {
                            // Check if there are any non-revoked tokens
                            let active_token = data.tokens.iter().find(|t| !t.revoked);

                            if active_token.is_some() {
                                // User already has tokens, show the create new option
                                token_state.set(TokenState::NoToken);
                            } else {
                                token_state.set(TokenState::NoToken);
                            }
                        } else {
                            token_state.set(TokenState::NoToken);
                        }
                    }
                    Err(e) => {
                        log::error!("Failed to fetch tokens: {:?}", e);
                        token_state.set(TokenState::NoToken);
                    }
                }
            });

            || ()
        });
    }

    let on_create_token = {
        let token_state = token_state.clone();
        let creating = creating.clone();

        Callback::from(move |_: MouseEvent| {
            let token_state = token_state.clone();
            let creating = creating.clone();

            creating.set(true);

            wasm_bindgen_futures::spawn_local(async move {
                let api_endpoint = utils::api_url("/api/proxy-tokens");

                let request_body = CreateProxyTokenRequest {
                    name: format!("CLI Setup - {}", js_sys::Date::new_0().to_locale_string("en-US", &js_sys::Object::new())),
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
                                token_state.set(TokenState::Error("Failed to parse response".to_string()));
                            }
                        } else {
                            token_state.set(TokenState::Error(format!("Server error: {}", response.status())));
                        }
                    }
                    Err(e) => {
                        token_state.set(TokenState::Error(format!("Request failed: {:?}", e)));
                    }
                }

                creating.set(false);
            });
        })
    };

    match (*token_state).clone() {
        TokenState::Loading => {
            html! {
                <div class="proxy-setup loading">
                    <div class="spinner-small"></div>
                    <span>{ "Loading..." }</span>
                </div>
            }
        }
        TokenState::NoToken => {
            html! {
                <div class="proxy-setup">
                    <h3>{ "Start a Session" }</h3>
                    <p class="setup-description">
                        { "Generate a secure token to connect the Claude proxy from your machine." }
                    </p>
                    <button
                        class="create-token-button"
                        onclick={on_create_token}
                        disabled={*creating}
                    >
                        if *creating {
                            <span class="spinner-small"></span>
                            { " Creating..." }
                        } else {
                            { "Generate Setup Command" }
                        }
                    </button>
                </div>
            }
        }
        TokenState::HasToken(token_response) => {
            // Check if we're in dev mode (localhost)
            let is_dev = web_sys::window()
                .and_then(|w| w.location().hostname().ok())
                .map(|h| h == "localhost" || h == "127.0.0.1")
                .unwrap_or(false);

            let (init_command, run_command) = if is_dev {
                (
                    format!("cargo run -p proxy -- --init \"{}\"", token_response.init_url),
                    "cargo run -p proxy".to_string(),
                )
            } else {
                (
                    format!("claude-proxy --init \"{}\"", token_response.init_url),
                    "claude-proxy".to_string(),
                )
            };

            html! {
                <div class="proxy-setup has-token">
                    <h3>{ "Setup Command Ready" }</h3>
                    <p class="setup-description">
                        { "Run this command on the machine where you want to use Claude:" }
                    </p>

                    <CopyCommand
                        command={init_command}
                        label={Some("One-time setup:".to_string())}
                    />

                    <div class="setup-notes">
                        <p class="note">
                            <span class="note-icon">{ "i" }</span>
                            { "After setup, just run " }
                            <code>{ run_command }</code>
                            { " to start a session." }
                        </p>
                        <p class="note expiry">
                            <span class="note-icon">{ "!" }</span>
                            { format!("This token expires: {}", format_expiry(&token_response.expires_at)) }
                        </p>
                    </div>

                    <button
                        class="create-another-button"
                        onclick={on_create_token}
                        disabled={*creating}
                    >
                        { "Generate New Token" }
                    </button>
                </div>
            }
        }
        TokenState::Error(error) => {
            html! {
                <div class="proxy-setup error">
                    <h3>{ "Error" }</h3>
                    <p class="error-message">{ error }</p>
                    <button
                        class="retry-button"
                        onclick={on_create_token}
                    >
                        { "Try Again" }
                    </button>
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
