//! Proxy Token Setup Component
//!
//! Displays instructions for setting up the proxy CLI with device flow authentication.

use crate::components::CopyCommand;
use gloo::utils::window;
use yew::prelude::*;

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
    let detected = detect_platform();
    let selected_platform = use_state(|| detected);

    // Get the base URL for the install script
    let base_url = web_sys::window()
        .and_then(|w| w.location().origin().ok())
        .unwrap_or_else(|| "http://localhost:3000".to_string());

    let platforms = [Platform::Linux, Platform::MacOS, Platform::Windows];

    // Derive WebSocket URL from current origin (http->ws, https->wss)
    let ws_backend_url = base_url
        .replace("https://", "wss://")
        .replace("http://", "ws://");

    let install_command = match *selected_platform {
        Platform::Linux | Platform::MacOS => {
            format!("curl -fsSL \"{}/api/download/install.sh\" | bash", base_url)
        }
        Platform::Windows => format!(
            "# Download from GitHub releases, then run:\n.\\claude-portal.exe --backend-url \"{}\"",
            ws_backend_url
        ),
    };
    let run_command = match *selected_platform {
        Platform::Linux | Platform::MacOS => "claude-portal".to_string(),
        Platform::Windows => ".\\claude-portal.exe".to_string(),
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
                    <p class="step-label">{ "Install:" }</p>
                    <CopyCommand command={install_command} />
                </div>
            </div>

            <div class="setup-step">
                <span class="step-number">{ "2" }</span>
                <div class="step-content">
                    <p class="step-label">{ "Start a session:" }</p>
                    <CopyCommand command={run_command} />
                    <p class="step-hint">{ "(Opens browser to authenticate on first run)" }</p>
                </div>
            </div>

            <div class="setup-notes">
                {if *selected_platform == Platform::Windows {
                    html! {
                        <>
                            <p class="note warning-note">
                                <span class="note-icon">{ "!" }</span>
                                { "Windows support is experimental and largely untested. " }
                                <a href="https://github.com/meawoppl/claude-code-portal/issues" target="_blank">
                                    { "Please report issues!" }
                                </a>
                            </p>
                            <p class="note windows-note">
                                <span class="note-icon">{ "!" }</span>
                                { "Download Windows binary from " }
                                <a href="https://github.com/meawoppl/claude-code-portal/releases/latest" target="_blank">
                                    { "GitHub releases" }
                                </a>
                            </p>
                        </>
                    }
                } else if *selected_platform == Platform::MacOS {
                    html! {
                        <p class="note warning-note">
                            <span class="note-icon">{ "!" }</span>
                            { "macOS support is experimental and largely untested. " }
                            <a href="https://github.com/meawoppl/claude-code-portal/issues" target="_blank">
                                { "Please report issues!" }
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
