//! Voice Input Component
//!
//! Provides voice-to-text input using the Web Audio API and AudioWorklet.
//! Audio is captured from the microphone, converted to PCM16 at 16kHz,
//! and sent via a dedicated WebSocket to the backend for speech-to-text processing.

use futures_util::{SinkExt, StreamExt};
use gloo::utils::window;
use gloo_net::websocket::{futures::WebSocket, Message};
use shared::ProxyMessage;
use std::cell::RefCell;
use std::rc::Rc;
use uuid::Uuid;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    AudioContext, AudioContextOptions, AudioWorkletNode, AudioWorkletNodeOptions, MediaStream,
    MediaStreamAudioSourceNode, MediaStreamConstraints, MessageEvent,
};
use yew::prelude::*;

/// Check if the browser supports AudioWorklet (required for voice input)
fn is_audio_worklet_supported() -> bool {
    if let Some(window) = web_sys::window() {
        // Check if AudioContext exists and has audioWorklet property
        let audio_ctx = js_sys::Reflect::get(&window, &JsValue::from_str("AudioContext"))
            .or_else(|_| js_sys::Reflect::get(&window, &JsValue::from_str("webkitAudioContext")));

        if let Ok(ctx_constructor) = audio_ctx {
            if !ctx_constructor.is_undefined() && !ctx_constructor.is_null() {
                // Check if prototype has audioWorklet
                if let Ok(proto) =
                    js_sys::Reflect::get(&ctx_constructor, &JsValue::from_str("prototype"))
                {
                    return js_sys::Reflect::has(&proto, &JsValue::from_str("audioWorklet"))
                        .unwrap_or(false);
                }
            }
        }
    }
    false
}

/// Props for the VoiceInput component
#[derive(Properties, PartialEq)]
pub struct VoiceInputProps {
    /// Session ID to associate voice input with
    pub session_id: Uuid,
    /// Callback when recording state changes
    pub on_recording_change: Callback<bool>,
    /// Callback when final transcription is received
    pub on_transcription: Callback<String>,
    /// Callback when interim (partial) transcription is received
    #[prop_or_default]
    pub on_interim_transcription: Option<Callback<String>>,
    /// Callback when an error occurs
    pub on_error: Callback<String>,
    /// Whether the component is disabled
    #[prop_or(false)]
    pub disabled: bool,
}

/// Voice input state
pub enum VoiceInputMsg {
    StartRecording,
    StopRecording,
    RecordingStarted(VoiceSession),
    WebSocketMessage(ProxyMessage),
    Error(String),
}

/// State for active recording session
pub struct VoiceRecordingState {
    audio_context: AudioContext,
    worklet_node: AudioWorkletNode,
    source_node: MediaStreamAudioSourceNode,
    _media_stream: MediaStream,
}

impl Drop for VoiceRecordingState {
    fn drop(&mut self) {
        // Stop the worklet
        if let Ok(port) = self.worklet_node.port() {
            let _ = port.post_message(&JsValue::from_str(r#"{"command":"stop"}"#));
        }

        // Disconnect nodes
        self.source_node.disconnect().ok();
        self.worklet_node.disconnect().ok();

        // Close audio context
        let _ = self.audio_context.close();
    }
}

/// Channel for sending audio data to the WebSocket
type AudioSender = Rc<RefCell<Option<futures_channel::mpsc::UnboundedSender<Vec<u8>>>>>;

/// Combined voice session state
pub struct VoiceSession {
    /// Held to keep audio resources alive - Drop handles cleanup
    _recording_state: VoiceRecordingState,
    audio_sender: AudioSender,
}

/// Voice input component with microphone button
pub struct VoiceInput {
    is_recording: bool,
    voice_session: Option<VoiceSession>,
    browser_supported: bool,
}

impl Component for VoiceInput {
    type Message = VoiceInputMsg;
    type Properties = VoiceInputProps;

    fn create(_ctx: &Context<Self>) -> Self {
        Self {
            is_recording: false,
            voice_session: None,
            browser_supported: is_audio_worklet_supported(),
        }
    }

    fn update(&mut self, ctx: &Context<Self>, msg: Self::Message) -> bool {
        match msg {
            VoiceInputMsg::StartRecording => {
                if self.is_recording {
                    return false;
                }

                // Check browser support first
                if !self.browser_supported {
                    ctx.props().on_error.emit("Voice input is not supported in this browser. Please use Chrome, Edge, or another modern browser.".to_string());
                    return false;
                }

                let link = ctx.link().clone();
                let session_id = ctx.props().session_id;
                let on_error = ctx.props().on_error.clone();

                wasm_bindgen_futures::spawn_local(async move {
                    match start_voice_session(session_id, link.clone()).await {
                        Ok(session) => {
                            link.send_message(VoiceInputMsg::RecordingStarted(session));
                        }
                        Err(e) => {
                            on_error.emit(e.clone());
                            link.send_message(VoiceInputMsg::Error(e));
                        }
                    }
                });

                false
            }
            VoiceInputMsg::StopRecording => {
                if !self.is_recording {
                    return false;
                }

                // Signal the audio sender to stop (dropping it will close the channel)
                if let Some(session) = &self.voice_session {
                    session.audio_sender.borrow_mut().take();
                }

                // Drop the voice session to clean up
                self.voice_session = None;
                self.is_recording = false;
                ctx.props().on_recording_change.emit(false);
                true
            }
            VoiceInputMsg::RecordingStarted(session) => {
                self.voice_session = Some(session);
                self.is_recording = true;
                ctx.props().on_recording_change.emit(true);
                true
            }
            VoiceInputMsg::WebSocketMessage(proxy_msg) => {
                match proxy_msg {
                    ProxyMessage::Transcription {
                        transcript,
                        is_final,
                        ..
                    } => {
                        if is_final {
                            ctx.props().on_transcription.emit(transcript);
                        } else if let Some(ref callback) = ctx.props().on_interim_transcription {
                            callback.emit(transcript);
                        }
                    }
                    ProxyMessage::VoiceError { message, .. } => {
                        ctx.props().on_error.emit(message);
                    }
                    _ => {}
                }
                false
            }
            VoiceInputMsg::Error(msg) => {
                log::error!("Voice input error: {}", msg);
                self.voice_session = None;
                self.is_recording = false;
                ctx.props().on_recording_change.emit(false);
                true
            }
        }
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        let onclick = if self.is_recording {
            ctx.link().callback(|_| VoiceInputMsg::StopRecording)
        } else {
            ctx.link().callback(|_| VoiceInputMsg::StartRecording)
        };

        let disabled = ctx.props().disabled || !self.browser_supported;
        let button_class = classes!(
            "voice-button",
            self.is_recording.then_some("recording"),
            disabled.then_some("disabled"),
            (!self.browser_supported).then_some("unsupported"),
        );

        let title = if !self.browser_supported {
            "Voice input not supported in this browser"
        } else if self.is_recording {
            "Stop recording"
        } else {
            "Start voice input"
        };

        html! {
            <button
                class={button_class}
                onclick={onclick}
                disabled={disabled}
                title={title}
                type="button"
            >
                if self.is_recording {
                    <span class="voice-icon recording-icon">{ "\u{1F534}" }</span> // Red circle
                } else if !self.browser_supported {
                    <span class="voice-icon mic-icon unsupported">{ "\u{1F507}" }</span> // Muted speaker
                } else {
                    <span class="voice-icon mic-icon">{ "\u{1F3A4}" }</span> // Microphone
                }
            </button>
        }
    }

    fn destroy(&mut self, _ctx: &Context<Self>) {
        // Clean up voice session when component is destroyed
        self.voice_session = None;
    }
}

/// Build the WebSocket URL for voice endpoint
fn build_voice_ws_url(session_id: Uuid) -> String {
    let location = window().location();
    let protocol = location.protocol().unwrap_or_else(|_| "http:".to_string());
    let host = location
        .host()
        .unwrap_or_else(|_| "localhost:3000".to_string());
    let ws_protocol = if protocol == "https:" { "wss:" } else { "ws:" };
    format!("{}//{}/ws/voice/{}", ws_protocol, host, session_id)
}

/// Start a voice recording session with WebSocket connection
async fn start_voice_session(
    session_id: Uuid,
    link: yew::html::Scope<VoiceInput>,
) -> Result<VoiceSession, String> {
    // Connect to voice WebSocket
    let ws_url = build_voice_ws_url(session_id);
    let ws = WebSocket::open(&ws_url)
        .map_err(|e| format!("Failed to connect to voice WebSocket: {:?}", e))?;
    let (mut ws_sender, mut ws_receiver) = ws.split();

    // Send StartVoice message
    let start_msg = ProxyMessage::StartVoice {
        session_id,
        language_code: "en-US".to_string(),
    };
    let start_json =
        serde_json::to_string(&start_msg).map_err(|_| "Failed to serialize StartVoice message")?;
    ws_sender
        .send(Message::Text(start_json))
        .await
        .map_err(|_| "Failed to send StartVoice message")?;

    // Create channel for audio data
    let (audio_tx, mut audio_rx) = futures_channel::mpsc::unbounded::<Vec<u8>>();
    let audio_sender: AudioSender = Rc::new(RefCell::new(Some(audio_tx)));

    // Spawn task to handle incoming WebSocket messages
    let link_for_ws = link.clone();
    wasm_bindgen_futures::spawn_local(async move {
        while let Some(msg) = ws_receiver.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    if let Ok(proxy_msg) = serde_json::from_str::<ProxyMessage>(&text) {
                        link_for_ws.send_message(VoiceInputMsg::WebSocketMessage(proxy_msg));
                    }
                }
                Err(e) => {
                    log::error!("Voice WebSocket error: {:?}", e);
                    link_for_ws.send_message(VoiceInputMsg::Error(
                        "WebSocket connection lost".to_string(),
                    ));
                    break;
                }
                _ => {}
            }
        }
    });

    // Spawn task to send audio data over WebSocket
    wasm_bindgen_futures::spawn_local(async move {
        while let Some(audio_data) = audio_rx.next().await {
            if ws_sender.send(Message::Bytes(audio_data)).await.is_err() {
                break;
            }
        }
        // Send StopVoice when audio channel closes
        let stop_msg = ProxyMessage::StopVoice { session_id };
        if let Ok(json) = serde_json::to_string(&stop_msg) {
            let _ = ws_sender.send(Message::Text(json)).await;
        }
        let _ = ws_sender.close().await;
    });

    // Start audio recording
    let recording_state = start_recording(audio_sender.clone()).await?;

    Ok(VoiceSession {
        _recording_state: recording_state,
        audio_sender,
    })
}

/// Start recording audio from the microphone
async fn start_recording(audio_sender: AudioSender) -> Result<VoiceRecordingState, String> {
    // Get user media (microphone)
    let navigator = window().navigator();
    let media_devices = navigator
        .media_devices()
        .map_err(|_| "Failed to get media devices")?;

    let constraints = MediaStreamConstraints::new();
    constraints.set_audio(&JsValue::TRUE);
    constraints.set_video(&JsValue::FALSE);

    let media_stream_promise = media_devices
        .get_user_media_with_constraints(&constraints)
        .map_err(|_| "Failed to request microphone access")?;

    let media_stream: MediaStream = JsFuture::from(media_stream_promise)
        .await
        .map_err(|e| format!("Microphone access denied: {:?}", e))?
        .dyn_into()
        .map_err(|_| "Invalid media stream")?;

    // Create audio context at 16kHz (matching Speech-to-Text requirement)
    let audio_options = AudioContextOptions::new();
    audio_options.set_sample_rate(16000.0);

    let audio_context = AudioContext::new_with_context_options(&audio_options)
        .map_err(|_| "Failed to create audio context")?;

    // Load the PCM processor worklet
    let worklet = audio_context
        .audio_worklet()
        .map_err(|_| "AudioWorklet not supported")?;

    JsFuture::from(
        worklet
            .add_module("/pcm-processor.js")
            .map_err(|_| "Failed to get module promise")?,
    )
    .await
    .map_err(|e| format!("Failed to load PCM processor: {:?}", e))?;

    // Create worklet node
    let worklet_options = AudioWorkletNodeOptions::new();
    let worklet_node =
        AudioWorkletNode::new_with_options(&audio_context, "pcm-processor", &worklet_options)
            .map_err(|_| "Failed to create worklet node")?;

    // Set up message handler for audio data from worklet
    let onmessage = Closure::wrap(Box::new(move |event: MessageEvent| {
        if let Ok(data) = event.data().dyn_into::<js_sys::Object>() {
            if let Ok(audio_buffer) = js_sys::Reflect::get(&data, &JsValue::from_str("audioData")) {
                if let Ok(array_buffer) = audio_buffer.dyn_into::<js_sys::ArrayBuffer>() {
                    let uint8_array = js_sys::Uint8Array::new(&array_buffer);
                    let mut bytes = vec![0u8; uint8_array.length() as usize];
                    uint8_array.copy_to(&mut bytes);
                    // Send audio data through the channel
                    if let Some(ref sender) = *audio_sender.borrow() {
                        let _ = sender.unbounded_send(bytes);
                    }
                }
            }
        }
    }) as Box<dyn FnMut(MessageEvent)>);

    if let Ok(port) = worklet_node.port() {
        port.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
    }
    onmessage.forget(); // Prevent closure from being dropped

    // Create source node from microphone stream
    let source_node = audio_context
        .create_media_stream_source(&media_stream)
        .map_err(|_| "Failed to create media stream source")?;

    // Connect: microphone -> worklet (worklet doesn't need to connect to destination)
    source_node
        .connect_with_audio_node(&worklet_node)
        .map_err(|_| "Failed to connect audio nodes")?;

    Ok(VoiceRecordingState {
        audio_context,
        worklet_node,
        source_node,
        _media_stream: media_stream,
    })
}
