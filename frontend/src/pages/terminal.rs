use yew::prelude::*;
use yew_router::prelude::*;
use gloo_net::websocket::{futures::WebSocket, Message};
use futures_util::{SinkExt, StreamExt};
use shared::ProxyMessage;
use wasm_bindgen_futures::spawn_local;

#[derive(Properties, PartialEq)]
pub struct TerminalPageProps {
    pub session_id: String,
}

pub enum TerminalMsg {
    SendInput(String),
    ReceivedOutput(String),
    WebSocketConnected,
    WebSocketError(String),
}

pub struct TerminalPage {
    messages: Vec<(String, String)>, // (role, content)
    input_value: String,
    ws_connected: bool,
}

impl Component for TerminalPage {
    type Message = TerminalMsg;
    type Properties = TerminalPageProps;

    fn create(ctx: &Context<Self>) -> Self {
        // Connect to WebSocket
        let link = ctx.link().clone();

        spawn_local(async move {
            match WebSocket::open("ws://localhost:3000/ws/client") {
                Ok(mut ws) => {
                    // Send registration message
                    let register_msg = ProxyMessage::Register {
                        session_name: "web-client".to_string(),
                        auth_token: None,
                        working_directory: "".to_string(),
                    };

                    if let Ok(json) = serde_json::to_string(&register_msg) {
                        let _ = ws.send(Message::Text(json)).await;
                    }

                    link.send_message(TerminalMsg::WebSocketConnected);

                    // Listen for messages
                    while let Some(msg) = ws.next().await {
                        match msg {
                            Ok(Message::Text(text)) => {
                                if let Ok(proxy_msg) = serde_json::from_str::<ProxyMessage>(&text) {
                                    if let ProxyMessage::ClaudeOutput { content } = proxy_msg {
                                        link.send_message(TerminalMsg::ReceivedOutput(
                                            serde_json::to_string_pretty(&content).unwrap_or_default()
                                        ));
                                    }
                                }
                            }
                            Err(e) => {
                                log::error!("WebSocket error: {:?}", e);
                                link.send_message(TerminalMsg::WebSocketError(format!("{:?}", e)));
                                break;
                            }
                            _ => {}
                        }
                    }
                }
                Err(e) => {
                    log::error!("Failed to connect WebSocket: {:?}", e);
                    link.send_message(TerminalMsg::WebSocketError(format!("{:?}", e)));
                }
            }
        });

        Self {
            messages: vec![],
            input_value: String::new(),
            ws_connected: false,
        }
    }

    fn update(&mut self, _ctx: &Context<Self>, msg: Self::Message) -> bool {
        match msg {
            TerminalMsg::SendInput(input) => {
                self.messages.push(("user".to_string(), input.clone()));
                self.input_value.clear();
                // TODO: Send to WebSocket
                true
            }
            TerminalMsg::ReceivedOutput(output) => {
                self.messages.push(("assistant".to_string(), output));
                true
            }
            TerminalMsg::WebSocketConnected => {
                self.ws_connected = true;
                true
            }
            TerminalMsg::WebSocketError(err) => {
                self.messages.push(("system".to_string(), format!("Error: {}", err)));
                self.ws_connected = false;
                true
            }
        }
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        let handle_back = Callback::from(|_| {
            // Navigation would be handled via router or a message
            log::info!("Back button clicked");
        });

        let handle_submit = {
            let link = ctx.link().clone();
            let input_value = self.input_value.clone();

            Callback::from(move |e: SubmitEvent| {
                e.prevent_default();
                if !input_value.trim().is_empty() {
                    link.send_message(TerminalMsg::SendInput(input_value.clone()));
                }
            })
        };

        let handle_input = {
            let link = ctx.link().clone();
            Callback::from(move |e: InputEvent| {
                if let Some(input) = e.target_dyn_into::<web_sys::HtmlInputElement>() {
                    // Update input value somehow - need mutable access
                    // This is a simplified version
                }
            })
        };

        html! {
            <div class="terminal-page">
                <header class="terminal-header">
                    <button class="back-button" onclick={handle_back}>
                        { "← Back to Dashboard" }
                    </button>
                    <div class="session-info">
                        <span class="session-id">{ "Session: " }{ &ctx.props().session_id }</span>
                        <span class={if self.ws_connected { "status connected" } else { "status disconnected" }}>
                            { if self.ws_connected { "● Connected" } else { "○ Disconnected" } }
                        </span>
                    </div>
                </header>

                <div class="terminal-content">
                    <div class="messages">
                        {
                            self.messages.iter().map(|(role, content)| {
                                let class = format!("message {}", role);
                                html! {
                                    <div class={class}>
                                        <div class="message-role">{ role }</div>
                                        <div class="message-content">
                                            <pre>{ content }</pre>
                                        </div>
                                    </div>
                                }
                            }).collect::<Html>()
                        }
                    </div>

                    <form class="input-form" onsubmit={handle_submit}>
                        <input
                            type="text"
                            class="message-input"
                            placeholder="Type your message to Claude..."
                            value={self.input_value.clone()}
                            oninput={handle_input}
                            disabled={!self.ws_connected}
                        />
                        <button type="submit" class="send-button" disabled={!self.ws_connected}>
                            { "Send" }
                        </button>
                    </form>
                </div>
            </div>
        }
    }
}
