# Voice Input Implementation Plan

This document outlines the implementation strategy for adding voice-to-text input to claude-code-portal using Google Cloud Speech-to-Text streaming API.

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────────────────┐
│                              Browser                                     │
│  ┌──────────────┐    ┌───────────────┐    ┌──────────────────────────┐  │
│  │  Microphone  │───▶│ AudioWorklet  │───▶│  WebSocket (existing)    │  │
│  │ getUserMedia │    │ PCM16 chunks  │    │  ProxyMessage::Audio     │  │
│  └──────────────┘    └───────────────┘    └────────────┬─────────────┘  │
│                                                         │                │
│  ┌──────────────────────────────────────────────────────┴─────────────┐  │
│  │  Transcription results displayed in input field                    │  │
│  │  Interim results shown with visual indicator                       │  │
│  └────────────────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────────────┘
                                    │
                                    ▼ WebSocket
┌─────────────────────────────────────────────────────────────────────────┐
│                         Backend (Rust/Axum)                              │
│  ┌────────────────────┐    ┌──────────────────────────────────────────┐ │
│  │  WebSocket Handler │───▶│  Google Speech-to-Text gRPC Client       │ │
│  │  audio chunks in   │    │  (google-cognitive-apis or tonic)        │ │
│  │  transcripts out   │◀───│  bidirectional streaming                 │ │
│  └────────────────────┘    └──────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────────────────┘
                                    │
                                    ▼ gRPC
                    ┌───────────────────────────────┐
                    │  Google Cloud Speech-to-Text  │
                    │  Streaming Recognition API    │
                    └───────────────────────────────┘
```

## Why This Architecture?

Google Cloud Speech-to-Text uses **gRPC**, which is not directly accessible from browsers. The browser cannot:
- Make gRPC calls (no browser gRPC support without grpc-web proxy)
- Safely embed GCP credentials (would expose service account)

Therefore, we proxy through the backend:
1. Browser captures audio, streams PCM chunks via WebSocket
2. Backend authenticates with GCP and streams to Speech-to-Text API
3. Backend forwards transcription results back to browser

## Implementation Phases

### Phase 1: Browser Audio Capture (Frontend/Yew)

#### 1.1 Web Audio API Setup

```javascript
// Conceptual flow - will be implemented in Rust/WASM via web-sys

// Request microphone access
const stream = await navigator.mediaDevices.getUserMedia({ audio: true });

// Create audio context and source
const audioContext = new AudioContext({ sampleRate: 16000 });
const source = audioContext.createMediaStreamSource(stream);

// Load PCM processor worklet
await audioContext.audioWorklet.addModule('pcm-processor.js');
const processor = new AudioWorkletNode(audioContext, 'pcm-processor');

// Connect: microphone -> processor -> (WebSocket)
source.connect(processor);

// Receive PCM chunks from processor
processor.port.onmessage = (event) => {
    const pcmData = event.data; // Int16Array
    websocket.send(pcmData);    // Binary frame
};
```

#### 1.2 AudioWorklet Processor (pcm-processor.js)

```javascript
class PCMProcessor extends AudioWorkletProcessor {
    constructor() {
        super();
        this.bufferSize = 4096;  // ~256ms at 16kHz
        this.buffer = new Float32Array(this.bufferSize);
        this.bufferIndex = 0;
    }

    process(inputs, outputs, parameters) {
        const input = inputs[0];
        if (!input || !input[0]) return true;

        const samples = input[0];

        for (let i = 0; i < samples.length; i++) {
            this.buffer[this.bufferIndex++] = samples[i];

            if (this.bufferIndex >= this.bufferSize) {
                // Convert Float32 [-1, 1] to Int16 [-32768, 32767]
                const pcm16 = new Int16Array(this.bufferSize);
                for (let j = 0; j < this.bufferSize; j++) {
                    pcm16[j] = Math.max(-32768, Math.min(32767,
                        Math.floor(this.buffer[j] * 32768)));
                }

                this.port.postMessage(pcm16.buffer, [pcm16.buffer]);
                this.buffer = new Float32Array(this.bufferSize);
                this.bufferIndex = 0;
            }
        }

        return true;
    }
}

registerProcessor('pcm-processor', PCMProcessor);
```

#### 1.3 Yew Component Integration

New component: `frontend/src/components/voice_input.rs`

```rust
// Pseudocode - actual implementation will use web-sys bindings

use web_sys::{AudioContext, MediaDevices, AudioWorkletNode};
use wasm_bindgen_futures::spawn_local;

pub struct VoiceInput {
    is_recording: bool,
    audio_context: Option<AudioContext>,
    interim_transcript: String,
}

pub enum VoiceInputMsg {
    StartRecording,
    StopRecording,
    AudioChunk(Vec<u8>),        // PCM data to send
    InterimTranscript(String),   // Partial result
    FinalTranscript(String),     // Final result
    Error(String),
}
```

### Phase 2: Backend Audio Relay (Rust/Axum)

#### 2.1 New Protocol Messages

Add to `shared/src/lib.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ProxyMessage {
    // ... existing variants ...

    /// Start voice recording session
    StartVoice {
        session_id: Uuid,
        language_code: Option<String>,  // Default: "en-US"
    },

    /// Audio chunk (binary, sent as separate binary WebSocket frame)
    AudioChunk {
        session_id: Uuid,
        // Actual audio data sent as binary frame, not in JSON
    },

    /// Stop voice recording
    StopVoice {
        session_id: Uuid,
    },

    /// Transcription result from backend
    Transcription {
        session_id: Uuid,
        transcript: String,
        is_final: bool,
        confidence: f32,
    },
}
```

#### 2.2 Google Speech-to-Text Client

New module: `backend/src/speech.rs`

Using `google-cognitive-apis` crate:

```rust
use google_cognitive_apis::speechtotext::recognizer::Recognizer;

pub struct SpeechClient {
    recognizer: Recognizer,
}

impl SpeechClient {
    pub async fn new(credentials_path: &str) -> Result<Self> {
        let recognizer = Recognizer::create_streaming_recognizer(
            credentials_path,
            StreamingRecognitionConfig {
                config: RecognitionConfig {
                    encoding: AudioEncoding::Linear16,
                    sample_rate_hertz: 16000,
                    language_code: "en-US".to_string(),
                    enable_automatic_punctuation: true,
                    ..Default::default()
                },
                interim_results: true,  // Get partial results
                ..Default::default()
            },
        ).await?;

        Ok(Self { recognizer })
    }

    pub async fn stream_audio(
        &mut self,
        audio_rx: mpsc::Receiver<Vec<u8>>,
        result_tx: mpsc::Sender<TranscriptionResult>,
    ) -> Result<()> {
        let audio_sink = self.recognizer.take_audio_sink();

        // Forward audio chunks to Google
        tokio::spawn(async move {
            while let Some(chunk) = audio_rx.recv().await {
                if audio_sink.send(chunk).is_err() {
                    break;
                }
            }
        });

        // Receive transcription results
        while let Some(result) = self.recognizer.receive().await {
            result_tx.send(result).await?;
        }

        Ok(())
    }
}
```

#### 2.3 WebSocket Handler Updates

Update `backend/src/handlers/websocket.rs`:

```rust
// Handle binary frames for audio data
Message::Binary(data) => {
    if let Some(speech_session) = &client.speech_session {
        speech_session.audio_tx.send(data).await?;
    }
}

// Handle voice control messages
ProxyMessage::StartVoice { session_id, language_code } => {
    let speech_client = SpeechClient::new(&app_state.gcp_credentials).await?;
    let (audio_tx, audio_rx) = mpsc::channel(100);
    let (result_tx, mut result_rx) = mpsc::channel(100);

    // Spawn speech streaming task
    tokio::spawn(async move {
        speech_client.stream_audio(audio_rx, result_tx).await;
    });

    // Forward results to WebSocket
    tokio::spawn(async move {
        while let Some(result) = result_rx.recv().await {
            let msg = ProxyMessage::Transcription {
                session_id,
                transcript: result.transcript,
                is_final: result.is_final,
                confidence: result.confidence,
            };
            ws_tx.send(Message::Text(serde_json::to_string(&msg)?)).await?;
        }
    });

    client.speech_session = Some(SpeechSession { audio_tx });
}

ProxyMessage::StopVoice { session_id } => {
    client.speech_session = None;  // Dropping closes the channel
}
```

### Phase 3: UI Integration

#### 3.1 Voice Button in SessionView

Add microphone button next to input field:

```rust
html! {
    <div class="input-area">
        <input type="text" ... />
        <button
            class={classes!("voice-btn", if self.is_recording { "recording" } else { "" })}
            onclick={link.callback(|_| SessionViewMsg::ToggleVoice)}
        >
            { if self.is_recording { "🔴" } else { "🎤" } }
        </button>
        <button type="submit">{ "Send" }</button>
    </div>
}
```

#### 3.2 Visual Feedback

- **Recording indicator**: Pulsing red dot or border
- **Interim transcripts**: Show in input field with italic/gray styling
- **Final transcript**: Replace input value, auto-focus for editing
- **Audio level meter**: Optional visual indicator of mic input level

### Phase 4: Configuration & Credentials

#### 4.1 Environment Variables

```bash
# Backend .env
GOOGLE_APPLICATION_CREDENTIALS=/path/to/service-account.json
# OR
GOOGLE_CLOUD_PROJECT=my-project-id
SPEECH_TO_TEXT_ENABLED=true
```

#### 4.2 GCP Setup Requirements

1. Enable Cloud Speech-to-Text API in GCP Console
2. Create service account with `roles/speech.client` role
3. Download JSON key file
4. Set `GOOGLE_APPLICATION_CREDENTIALS` environment variable

#### 4.3 1Password Integration (Production)

Update `backend/.env.example`:
```bash
GOOGLE_APPLICATION_CREDENTIALS=op://claude-code-portal/gcp-speech/credential
```

## Audio Format Specifications

| Parameter | Value | Notes |
|-----------|-------|-------|
| Encoding | LINEAR16 (PCM) | 16-bit signed little-endian |
| Sample Rate | 16000 Hz | Required for streaming |
| Channels | 1 (Mono) | Stereo not supported for streaming |
| Chunk Size | 4096 samples | ~256ms per chunk |
| Chunk Duration | 100-500ms | Balance latency vs overhead |

## Dependencies

### Frontend (Cargo.toml)
```toml
[dependencies]
web-sys = { version = "0.3", features = [
    "Navigator",
    "MediaDevices",
    "MediaStream",
    "AudioContext",
    "AudioWorkletNode",
    "AudioWorkletGlobalScope",
    "MediaStreamAudioSourceNode",
] }
wasm-bindgen = "0.2"
js-sys = "0.3"
```

### Backend (Cargo.toml)
```toml
[dependencies]
google-cognitive-apis = "0.2"  # Or tonic + prost for raw gRPC
tokio = { version = "1", features = ["sync", "rt-multi-thread"] }
```

## Security Considerations

1. **Microphone permissions**: Only request when user initiates voice input
2. **Audio not stored**: Stream directly, don't persist audio files
3. **Credential security**: Never expose GCP credentials to frontend
4. **Rate limiting**: Limit concurrent speech sessions per user
5. **Data privacy**: Consider GCP data processing terms for audio

## Testing Strategy

### Unit Tests
- PCM conversion accuracy
- WebSocket message serialization
- Audio chunk buffering logic

### Integration Tests
- Mock GCP API for CI/CD
- End-to-end WebSocket flow
- Audio capture in headless browser (Playwright)

### Manual Testing
- Various microphones/browsers
- Background noise handling
- Network latency simulation
- Mobile browser support

## Rollout Plan

1. **MVP**: Basic voice input with English only
2. **V2**: Multi-language support, interim results
3. **V3**: Voice activity detection (VAD), auto-stop
4. **V4**: Hotword activation ("Hey Claude")

## Alternative Approaches Considered

### Browser Web Speech API
- **Pros**: No backend needed, free
- **Cons**: Chrome-only, inconsistent quality, no interim results control

### Whisper (Local)
- **Pros**: Offline, private, free
- **Cons**: Higher latency, requires model download, CPU intensive

### AWS Transcribe
- **Pros**: Similar quality to Google
- **Cons**: Different API, already using GCP for other services

## References

- [Google Cloud Speech-to-Text Streaming](https://cloud.google.com/speech-to-text/docs/streaming-recognize)
- [google-cognitive-apis Rust crate](https://lib.rs/crates/google-cognitive-apis)
- [Web Audio API AudioWorklet](https://developer.mozilla.org/en-US/docs/Web/API/Web_Audio_API/Using_AudioWorklet)
- [web.dev Microphone Processing](https://web.dev/patterns/media/microphone-process/)
- [PCM Audio Streaming Example](https://medium.com/developer-rants/streaming-audio-with-16-bit-mono-pcm-encoding-from-the-browser-and-how-to-mix-audio-while-we-are-f6a160409135)
- [AWS Multi-channel Audio Streaming](https://aws.amazon.com/blogs/machine-learning/stream-multi-channel-audio-to-amazon-transcribe-using-the-web-audio-api/)
