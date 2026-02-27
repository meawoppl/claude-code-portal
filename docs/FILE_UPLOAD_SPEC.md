# File Upload Feature Specification

> **Note**: This document was written as a design spec for a WebRTC-based approach. The actual implementation uses chunked WebSocket transfer via `FileUploadStart`/`FileUploadChunk` messages in `shared/src/endpoints.rs`, not WebRTC. The WebRTC approach described below was not implemented.

This document outlines how to implement drag-and-drop file uploads from the web dashboard to the Claude Code client running on the user's machine. All file transfers use WebRTC DataChannels for direct peer-to-peer transfer, bypassing the server entirely. The WebSocket connection is used only for signaling.

## Architecture

### WebRTC Direct Transfer

```
Browser ←──WebRTC DataChannel──→ Proxy
         (signaling via WebSocket)
```

**Pros**: Direct peer-to-peer, no server bandwidth, lower latency, progress tracking built-in
**Cons**: NAT traversal complexity, requires STUN/TURN

All files are transferred via WebRTC regardless of size. This keeps the architecture simple (one code path) and avoids server bandwidth costs entirely.

## Protocol Messages

### WebRTC Signaling Messages

Add to `shared/src/lib.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ProxyMessage {
    // ... existing variants ...

    // ========== WebRTC Signaling ==========

    /// Browser → Proxy: Request to start file upload
    FileUploadRequest {
        /// Unique ID for this upload
        upload_id: String,
        /// Relative path within working directory
        path: String,
        /// Total file size in bytes
        total_size: u64,
        /// MIME type if known
        mime_type: Option<String>,
    },

    /// Proxy → Browser: Accept/reject upload request
    FileUploadRequestAck {
        upload_id: String,
        accepted: bool,
        error: Option<String>,
    },

    /// Browser → Proxy: Offer to establish WebRTC connection
    WebRTCOffer {
        upload_id: String,
        sdp: String,
    },

    /// Proxy → Browser: Answer to WebRTC offer
    WebRTCAnswer {
        upload_id: String,
        sdp: String,
    },

    /// Both directions: ICE candidate exchange
    WebRTCIceCandidate {
        upload_id: String,
        candidate: String,
        sdp_mid: Option<String>,
        sdp_m_line_index: Option<u16>,
    },

    /// Proxy → Browser: Upload complete or failed
    FileUploadResult {
        upload_id: String,
        path: String,
        success: bool,
        error: Option<String>,
    },
}
```

## Upload Flow

```
┌─────────────────────────────────────────────────────────────┐
│                  Complete Upload Flow                        │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│  Browser                    Backend                  Proxy   │
│     │                          │                       │     │
│     │──FileUploadRequest─────►│──────────────────────►│     │
│     │                          │   (validate path,     │     │
│     │                          │    check disk space)  │     │
│     │◄─FileUploadRequestAck───│◄──────────────────────│     │
│     │                          │                       │     │
│     │ Create RTCPeerConnection │                       │     │
│     │ Create DataChannel       │                       │     │
│     │ createOffer()            │                       │     │
│     │                          │                       │     │
│     │──WebRTCOffer────────────►│──────────────────────►│     │
│     │                          │       Create RTCPeer  │     │
│     │                          │       setRemoteDesc   │     │
│     │                          │       createAnswer()  │     │
│     │◄─WebRTCAnswer────────────│◄──────────────────────│     │
│     │                          │                       │     │
│     │◄─►WebRTCIceCandidate────►│◄─────────────────────►│     │
│     │   (multiple exchanges)   │                       │     │
│     │                          │                       │     │
│     │════════ DataChannel Connected ═══════════════════│     │
│     │                          │                       │     │
│     │──────── Binary chunks ─────────────────────────►│     │
│     │◄─────── Progress acks  ────────────────────────│     │
│     │                          │                       │     │
│     │◄─FileUploadResult────────│◄──────────────────────│     │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

## DataChannel Protocol

Once the WebRTC DataChannel is established, binary data flows directly between browser and proxy:

### Message Format (Binary)

```rust
// Header (8 bytes) + Data
struct ChunkMessage {
    chunk_index: u32,    // 4 bytes, big-endian
    chunk_size: u32,     // 4 bytes, big-endian
    data: Vec<u8>,       // Variable length
}

// Ack message from proxy (8 bytes)
struct ChunkAck {
    chunk_index: u32,    // 4 bytes, big-endian
    total_received: u32, // 4 bytes, big-endian (cumulative bytes)
}
```

### Chunk Size

- Default chunk size: 64 KB (65,536 bytes)
- Binary transfer (no base64 overhead)
- Flow control via DataChannel bufferedAmount

### Progress Tracking

```rust
// Frontend progress tracking
struct UploadProgress {
    upload_id: String,
    file_name: String,
    total_bytes: u64,
    bytes_sent: u64,
    bytes_acked: u64,
    started_at: DateTime<Utc>,
}

impl UploadProgress {
    fn percent_complete(&self) -> f32 {
        (self.bytes_acked as f32 / self.total_bytes as f32) * 100.0
    }

    fn estimated_time_remaining(&self) -> Duration {
        let elapsed = Utc::now() - self.started_at;
        let bytes_per_sec = self.bytes_acked as f64 / elapsed.num_seconds() as f64;
        let remaining_bytes = self.total_bytes - self.bytes_acked;
        Duration::seconds((remaining_bytes as f64 / bytes_per_sec) as i64)
    }
}
```

## Proxy WebRTC Implementation

The proxy needs a WebRTC library. Recommended: **webrtc-rs** (pure Rust).

```rust
// proxy/src/webrtc.rs
use webrtc::api::API;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::data_channel::RTCDataChannel;

pub struct FileReceiver {
    peer_connection: RTCPeerConnection,
    upload_id: String,
    target_path: PathBuf,
    expected_size: u64,
    received_bytes: u64,
    file_handle: Option<File>,
}

impl FileReceiver {
    pub async fn new(
        upload_id: String,
        target_path: PathBuf,
        expected_size: u64,
    ) -> Result<Self, Error> {
        let api = API::new(Default::default());
        let config = RTCConfiguration {
            ice_servers: vec![
                RTCIceServer {
                    urls: vec!["stun:stun.l.google.com:19302".to_string()],
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let peer_connection = api.new_peer_connection(config).await?;

        Ok(Self {
            peer_connection,
            upload_id,
            target_path,
            expected_size,
            received_bytes: 0,
            file_handle: None,
        })
    }

    pub async fn handle_offer(&mut self, sdp: &str) -> Result<String, Error> {
        // Set up data channel handler
        let target_path = self.target_path.clone();
        let expected_size = self.expected_size;

        self.peer_connection.on_data_channel(Box::new(move |dc| {
            let path = target_path.clone();
            Box::pin(async move {
                dc.on_message(Box::new(move |msg| {
                    // Handle incoming chunks
                    let data = msg.data.to_vec();
                    // Parse header, write to file, send ack
                }));
            })
        }));

        // Set remote description from browser's offer
        let offer = RTCSessionDescription::offer(sdp.to_string())?;
        self.peer_connection.set_remote_description(offer).await?;

        // Create answer
        let answer = self.peer_connection.create_answer(None).await?;
        self.peer_connection.set_local_description(answer.clone()).await?;

        Ok(answer.sdp)
    }

    pub async fn handle_ice_candidate(&mut self, candidate: &str) -> Result<(), Error> {
        let candidate = RTCIceCandidateInit {
            candidate: candidate.to_string(),
            ..Default::default()
        };
        self.peer_connection.add_ice_candidate(candidate).await
    }
}
```

## NAT Traversal

For WebRTC to work across NATs:

1. **STUN Server**: For discovering public IP (can use public STUN servers)
2. **TURN Server**: Relay fallback when direct connection fails

```rust
// ICE server configuration
let ice_servers = vec![
    RTCIceServer {
        urls: vec!["stun:stun.l.google.com:19302".to_string()],
        ..Default::default()
    },
    // Optional TURN server for fallback when direct connection fails
    RTCIceServer {
        urls: vec!["turn:turn.example.com:3478".to_string()],
        username: "user".to_string(),
        credential: "pass".to_string(),
        ..Default::default()
    },
];
```

**Note**: Most connections will work with just STUN. TURN is only needed when both peers are behind symmetric NATs.

## Frontend Implementation

### 1. Drag and Drop Zone

```rust
// frontend/src/components/file_drop_zone.rs

use gloo::file::{callbacks::FileReader, File};
use web_sys::{DragEvent, FileList, RtcPeerConnection, RtcDataChannel};
use yew::prelude::*;

#[derive(Properties, PartialEq)]
pub struct FileDropZoneProps {
    pub session_id: String,
    pub children: Children,
}

#[function_component(FileDropZone)]
pub fn file_drop_zone(props: &FileDropZoneProps) -> Html {
    let dragging = use_state(|| false);
    let upload_progress = use_state(|| None::<UploadProgress>);

    let ondragover = {
        let dragging = dragging.clone();
        Callback::from(move |e: DragEvent| {
            e.prevent_default();
            dragging.set(true);
        })
    };

    let ondragleave = {
        let dragging = dragging.clone();
        Callback::from(move |e: DragEvent| {
            e.prevent_default();
            dragging.set(false);
        })
    };

    let ondrop = {
        let session_id = props.session_id.clone();
        let dragging = dragging.clone();
        let upload_progress = upload_progress.clone();

        Callback::from(move |e: DragEvent| {
            e.prevent_default();
            dragging.set(false);

            if let Some(files) = e.data_transfer().and_then(|dt| dt.files()) {
                for i in 0..files.length() {
                    if let Some(file) = files.get(i) {
                        // Start WebRTC upload for each file
                        start_webrtc_upload(session_id.clone(), file, upload_progress.clone());
                    }
                }
            }
        })
    };

    html! {
        <div class="file-drop-zone" {ondragover} {ondragleave} {ondrop}>
            { if *dragging {
                html! { <div class="drop-overlay">{"Drop files here"}</div> }
            } else {
                html! {}
            }}
            { if let Some(progress) = &*upload_progress {
                html! { <UploadProgressBar progress={progress.clone()} /> }
            } else {
                html! {}
            }}
            { props.children.clone() }
        </div>
    }
}
```

### 2. WebRTC Upload Logic

```rust
// frontend/src/upload.rs

async fn start_webrtc_upload(
    session_id: String,
    file: web_sys::File,
    progress: UseStateHandle<Option<UploadProgress>>,
) -> Result<(), JsValue> {
    let upload_id = uuid::Uuid::new_v4().to_string();
    let file_name = file.name();
    let file_size = file.size() as u64;

    // 1. Send upload request via WebSocket
    send_message(ProxyMessage::FileUploadRequest {
        upload_id: upload_id.clone(),
        path: file_name.clone(),
        total_size: file_size,
        mime_type: Some(file.type_()),
    }).await?;

    // 2. Wait for acknowledgment
    let ack = wait_for_message::<FileUploadRequestAck>(&upload_id).await?;
    if !ack.accepted {
        return Err(JsValue::from_str(&ack.error.unwrap_or_default()));
    }

    // 3. Create RTCPeerConnection
    let config = RtcConfiguration::new();
    config.set_ice_servers(&js_sys::Array::of1(
        &JsValue::from_serde(&json!({
            "urls": "stun:stun.l.google.com:19302"
        })).unwrap()
    ));

    let pc = RtcPeerConnection::new_with_configuration(&config)?;

    // 4. Create DataChannel
    let dc = pc.create_data_channel("file-upload");
    dc.set_binary_type(RtcDataChannelType::Arraybuffer);

    // 5. Create and send offer
    let offer = JsFuture::from(pc.create_offer()).await?;
    JsFuture::from(pc.set_local_description(&offer.into())).await?;

    send_message(ProxyMessage::WebRTCOffer {
        upload_id: upload_id.clone(),
        sdp: pc.local_description().unwrap().sdp(),
    }).await?;

    // 6. Handle answer and ICE candidates (via WebSocket)
    // ...

    // 7. Once DataChannel is open, send file chunks
    let chunk_size = 64 * 1024; // 64 KB
    let file_bytes = read_file_as_bytes(&file).await?;

    for (i, chunk) in file_bytes.chunks(chunk_size).enumerate() {
        // Wait for buffer to drain if needed
        while dc.buffered_amount() > 1024 * 1024 {
            gloo_timers::future::TimeoutFuture::new(10).await;
        }

        // Send chunk with header
        let mut msg = Vec::with_capacity(8 + chunk.len());
        msg.extend_from_slice(&(i as u32).to_be_bytes());
        msg.extend_from_slice(&(chunk.len() as u32).to_be_bytes());
        msg.extend_from_slice(chunk);

        dc.send_with_u8_array(&msg)?;

        // Update progress
        progress.set(Some(UploadProgress {
            upload_id: upload_id.clone(),
            file_name: file_name.clone(),
            total_bytes: file_size,
            bytes_sent: ((i + 1) * chunk_size).min(file_size as usize) as u64,
            bytes_acked: 0, // Updated when we receive acks
            started_at: Utc::now(),
        }));
    }

    Ok(())
}
```

### 3. Path Handling

For drag-and-drop, files don't have path context. Options:
- Drop into "uploads" subdirectory
- Show a path input dialog before upload
- Use the file's original name in working directory root

## Backend Implementation

### Message Routing

The backend only handles signaling - no file data passes through:

```rust
// In websocket handler
match message {
    ProxyMessage::FileUploadRequest { .. } |
    ProxyMessage::WebRTCOffer { .. } |
    ProxyMessage::WebRTCIceCandidate { .. } => {
        // Forward signaling to proxy
        if let Some(proxy_tx) = session_manager.get_proxy_sender(&session_id) {
            proxy_tx.send(message).await?;
        }
    }

    ProxyMessage::FileUploadRequestAck { .. } |
    ProxyMessage::WebRTCAnswer { .. } |
    ProxyMessage::FileUploadResult { .. } => {
        // Forward signaling to browser
        if let Some(browser_tx) = session_manager.get_browser_sender(&session_id) {
            browser_tx.send(message).await?;
        }
    }

    // ... other message types
}
```

## Security Considerations

### Path Traversal Prevention

**Critical**: Validate that the upload path cannot escape the working directory:

```rust
fn is_safe_path(working_dir: &Path, relative_path: &str) -> bool {
    // Reject absolute paths
    if Path::new(relative_path).is_absolute() {
        return false;
    }

    // Reject paths with ..
    if relative_path.contains("..") {
        return false;
    }

    // Normalize and verify it stays within working dir
    let full_path = working_dir.join(relative_path).canonicalize();
    match full_path {
        Ok(p) => p.starts_with(working_dir),
        Err(_) => false, // Path doesn't exist yet, need different check
    }
}
```

### File Type Restrictions

Consider restricting uploadable file types:
- Allow: text files, images, common code files
- Block: executables, scripts (unless explicitly allowed)

### Size Limits

- Individual file: 100 MB default (larger is fine with WebRTC)
- Total upload per session: 1 GB
- Configurable via environment variables

### Rate Limiting

Prevent abuse with upload rate limits:
- Max 10 files per minute
- Max 500 MB per minute

## UI/UX Design

### Visual Feedback

1. **Drag overlay**: Semi-transparent overlay when dragging files over the session
2. **Upload progress**: Show progress bar with percentage and ETA
3. **Success/error toasts**: Notify user of upload results

### CSS Classes

```css
.file-drop-zone {
    position: relative;
    min-height: 100%;
}

.drop-overlay {
    position: absolute;
    inset: 0;
    background: rgba(122, 162, 247, 0.2);
    border: 2px dashed var(--accent);
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 1.5rem;
    color: var(--accent);
    z-index: 100;
    pointer-events: none;
}

.upload-progress {
    position: fixed;
    bottom: 1rem;
    right: 1rem;
    background: var(--bg-secondary);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 1rem;
    min-width: 300px;
}

.upload-progress .progress-bar {
    height: 8px;
    background: var(--bg-tertiary);
    border-radius: 4px;
    overflow: hidden;
    margin-top: 0.5rem;
}

.upload-progress .progress-fill {
    height: 100%;
    background: var(--accent);
    transition: width 0.1s ease;
}
```

## Implementation Order

1. **Protocol**: Add signaling messages to `shared/src/lib.rs`
2. **Proxy**: Add webrtc-rs dependency, implement FileReceiver
3. **Backend**: Add signaling message routing
4. **Frontend**: Implement drag-and-drop zone component
5. **Frontend**: Implement WebRTC upload logic
6. **Frontend**: Add upload progress UI
7. **Testing**: Test NAT traversal, large files, error cases

## Future Enhancements

- **Directory upload**: Support dragging folders (via `webkitdirectory`)
- **Clipboard paste**: Support pasting images/files from clipboard
- **Download files**: Allow downloading files from the session (reverse direction via WebRTC)
- **File browser**: Show working directory contents, allow browsing
- **Conflict handling**: Ask user before overwriting existing files
- **Resume**: Support resuming interrupted uploads

## Testing

### Manual Testing

1. Drag single file onto session view
2. Drag multiple files
3. Drag file with spaces in name
4. Drag file with unicode characters in name
5. Attempt path traversal (should fail)
6. Upload file larger than limit (should fail with clear error)
7. Upload to disconnected session (should show error)
8. Test with peers on different networks (NAT traversal)

### Automated Testing

- Unit tests for path validation
- Unit tests for chunk message parsing
- Integration tests for WebRTC signaling flow
- E2E tests for complete upload flow
