use crate::utils;
use gloo::console;
use yew::prelude::*;

#[function_component(SplashPage)]
pub fn splash_page() -> Html {
    let handle_login = Callback::from(|_| {
        console::log!("Redirecting to Google OAuth...");
        // Redirect to backend OAuth endpoint
        let window = web_sys::window().expect("no global `window` exists");
        let location = window.location();
        let auth_url = utils::api_url("/auth/google");
        let _ = location.set_href(&auth_url);
    });

    html! {
        <div class="splash-container">
            <div class="splash-content">
                <div class="splash-header">
                    <h1>{ "Claude Code Proxy" }</h1>
                    <p class="tagline">
                        { "Access your remote Claude Code sessions from anywhere" }
                    </p>
                </div>

                <div class="splash-hero">
                    <div class="terminal-preview">
                        <div class="terminal-header">
                            <span class="terminal-dot red"></span>
                            <span class="terminal-dot yellow"></span>
                            <span class="terminal-dot green"></span>
                        </div>
                        <div class="terminal-body">
                            <div class="terminal-line">
                                <span class="prompt">{ "$ " }</span>
                                <span class="command">{ "claude-proxy --session my-dev-machine" }</span>
                            </div>
                            <div class="terminal-line">
                                <span class="output">{ "✓ Connected to backend" }</span>
                            </div>
                            <div class="terminal-line">
                                <span class="output">{ "✓ Session registered" }</span>
                            </div>
                            <div class="terminal-line">
                                <span class="prompt">{ "$ " }</span>
                                <span class="cursor">{ "▊" }</span>
                            </div>
                        </div>
                    </div>
                </div>

                <div class="splash-features">
                    <div class="feature">
                        <h3>{ "🌐 Remote Access" }</h3>
                        <p>{ "Connect to your dedicated development machines from any browser" }</p>
                    </div>
                    <div class="feature">
                        <h3>{ "🔄 Multiple Sessions" }</h3>
                        <p>{ "Manage and switch between multiple Claude Code sessions" }</p>
                    </div>
                    <div class="feature">
                        <h3>{ "🔒 Secure" }</h3>
                        <p>{ "Google OAuth authentication and encrypted connections" }</p>
                    </div>
                </div>

                <button class="login-button" onclick={handle_login}>
                    <span class="google-icon">{ "G" }</span>
                    { " Sign in with Google" }
                </button>
            </div>
        </div>
    }
}
