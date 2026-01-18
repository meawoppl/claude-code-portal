# File Upload Feature Specification

This document outlines how to implement drag-and-drop file uploads from the web dashboard to the Claude Code client running on the user's machine.

## Overview

Users should be able to drag and drop files (or use a file picker) in the web dashboard to upload files to their local Claude Code session. Files are transmitted with progress tracking, and for large files, we can optionally use WebRTC for direct peer-to-peer transfer.

## Architecture Options

### Option A: WebSocket Relay (Simple, Works Everywhere)

```
Browser (drag/drop) → WebSocket → Backend → WebSocket → Proxy → Local filesystem
```

**Pros**: Simple, works behind NATs, no additional setup
**Cons**: All data flows through server, higher latency, server bandwidth costs

### Option B: WebRTC Direct Transfer (Optimal for Large Files)

```
Browser ←──WebRTC DataChannel──→ Proxy
         (signaling via WebSocket)
```

**Pros**: Direct peer-to-peer, no server bandwidth, lower latency
**Cons**: NAT traversal complexity, requires STUN/TURN, more implementation work

### Recommended: Hybrid Approach

- **Small files (<1MB)**: Use WebSocket relay (simpler, fast enough)
- **Large files (>1MB)**: Attempt WebRTC, fall back to WebSocket if connection fails

## Protocol Messages

### New ProxyMessage Variants

Add to `shared/src/lib.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ProxyMessage {
    // ... existing variants ...

    // ========== Chunked Upload (WebSocket) ==========

    /// Browser → Proxy: Initiate file upload
    FileUploadStart {
        /// Unique ID for this upload
        upload_id: String,
        /// Relative path within working directory
        path: String,
        /// Total file size in bytes
        total_size: u64,
        /// Number of chunks
        total_chunks: u32,
        /// MIME type if known
        mime_type: Option<String>,
    },

    /// Browser → Proxy: Send a chunk of file data
    FileUploadChunk {
        upload_id: String,
        chunk_index: u32,
        /// Base64-encoded chunk data
        data_base64: String,
    },

    /// Proxy → Browser: Acknowledge chunk received
    FileUploadChunkAck {
        upload_id: String,
        chunk_index: u32,
        /// Bytes received so far
        bytes_received: u64,
    },

    /// Proxy → Browser: Upload complete or failed
    FileUploadResult {
        upload_id: String,
        path: String,
        success: bool,
        error: Option<String>,
    },

    // ========== WebRTC Signaling ==========

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
}
```

## Chunked Upload Protocol

For progress tracking, files are split into chunks:

```
┌─────────────────────────────────────────────────────────────┐
│                     Upload Flow                              │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│  Browser                    Backend                  Proxy   │
│     │                          │                       │     │
│     │──FileUploadStart────────►│──────────────────────►│     │
│     │                          │                       │     │
│     │──FileUploadChunk[0]─────►│──────────────────────►│     │
│     │◄─FileUploadChunkAck[0]───│◄──────────────────────│     │
│     │                          │                       │     │
│     │──FileUploadChunk[1]─────►│──────────────────────►│     │
│     │◄─FileUploadChunkAck[1]───│◄──────────────────────│     │
│     │                          │                       │     │
│     │        ... more chunks ...                       │     │
│     │                          │                       │     │
│     │◄─FileUploadResult────────│◄──────────────────────│     │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

### Chunk Size

- Default chunk size: 64 KB (65,536 bytes)
- Base64 encoding overhead: ~33%, so wire size ~87 KB per chunk
- Configurable based on connection quality

### Progress Calculation

```rust
// Frontend progress tracking
struct UploadProgress {
    upload_id: String,
    file_name: String,
    total_bytes: u64,
    bytes_sent: u64,
    chunks_acked: u32,
    total_chunks: u32,
    started_at: DateTime<Utc>,
}

impl UploadProgress {
    fn percent_complete(&self) -> f32 {
        (self.bytes_sent as f32 / self.total_bytes as f32) * 100.0
    }

    fn estimated_time_remaining(&self) -> Duration {
        let elapsed = Utc::now() - self.started_at;
        let bytes_per_sec = self.bytes_sent as f64 / elapsed.num_seconds() as f64;
        let remaining_bytes = self.total_bytes - self.bytes_sent;
        Duration::seconds((remaining_bytes as f64 / bytes_per_sec) as i64)
    }
}
```

## WebRTC Direct Transfer

For large files, WebRTC DataChannels provide direct peer-to-peer transfer:

### Connection Flow

```
┌─────────────────────────────────────────────────────────────┐
│                  WebRTC Connection Setup                     │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│  Browser                    Backend                  Proxy   │
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
│     │──────── Binary Data ────────────────────────────►│     │
│     │◄─────── Progress Acks ──────────────────────────│     │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

### Proxy WebRTC Implementation

The proxy needs a WebRTC library. Options for Rust:

1. **webrtc-rs**: Pure Rust implementation
2. **libdatachannel**: C library with Rust bindings

```rust
// proxy/src/webrtc.rs
use webrtc::api::API;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::data_channel::RTCDataChannel;

pub struct FileReceiver {
    peer_connection: RTCPeerConnection,
    data_channel: Option<RTCDataChannel>,
    file_buffer: Vec<u8>,
    expected_size: u64,
}

impl FileReceiver {
    pub async fn handle_offer(&mut self, sdp: &str) -> Result<String, Error> {
        // Set remote description from browser's offer
        let offer = RTCSessionDescription::offer(sdp.to_string())?;
        self.peer_connection.set_remote_description(offer).await?;

        // Create answer
        let answer = self.peer_connection.create_answer(None).await?;
        self.peer_connection.set_local_description(answer.clone()).await?;

        Ok(answer.sdp)
    }

    pub async fn handle_ice_candidate(&mut self, candidate: &str) -> Result<(), Error> {
        let candidate = RTCIceCandidateInit { candidate: candidate.to_string(), ..Default::default() };
        self.peer_connection.add_ice_candidate(candidate).await
    }
}
```

### NAT Traversal

For WebRTC to work across NATs, we need:

1. **STUN Server**: For discovering public IP (can use public STUN servers)
2. **TURN Server**: Relay fallback when direct connection fails (optional, costly)

```rust
// ICE server configuration
let ice_servers = vec![
    RTCIceServer {
        urls: vec!["stun:stun.l.google.com:19302".to_string()],
        ..Default::default()
    },
    // Optional TURN server for fallback
    RTCIceServer {
        urls: vec!["turn:turn.example.com:3478".to_string()],
        username: "user".to_string(),
        credential: "pass".to_string(),
        ..Default::default()
    },
];
```

### Fallback Strategy

```rust
async fn upload_file(file: File, session: &Session) -> Result<(), Error> {
    let file_size = file.size();

    if file_size < 1_000_000 {
        // Small file: use WebSocket directly
        return upload_via_websocket(file, session).await;
    }

    // Large file: try WebRTC first
    match establish_webrtc_connection(session).await {
        Ok(data_channel) => {
            upload_via_webrtc(file, data_channel).await
        }
        Err(e) => {
            log::warn!("WebRTC failed, falling back to WebSocket: {}", e);
            upload_via_websocket(file, session).await
        }
    }
}
```

## Frontend Implementation

### 1. Drag and Drop Zone

Add a drop zone overlay to the session view that appears when dragging files:

```rust
// frontend/src/components/file_drop_zone.rs

use gloo::file::{callbacks::FileReader, File};
use web_sys::{DragEvent, FileList};
use yew::prelude::*;

#[derive(Properties, PartialEq)]
pub struct FileDropZoneProps {
    pub on_files_dropped: Callback<Vec<(String, Vec<u8>)>>,
    pub children: Children,
}

#[function_component(FileDropZone)]
pub fn file_drop_zone(props: &FileDropZoneProps) -> Html {
    let dragging = use_state(|| false);

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
        let on_files = props.on_files_dropped.clone();
        let dragging = dragging.clone();
        Callback::from(move |e: DragEvent| {
            e.prevent_default();
            dragging.set(false);

            if let Some(files) = e.data_transfer().and_then(|dt| dt.files()) {
                // Process files...
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
            { props.children.clone() }
        </div>
    }
}
```

### 2. File Reading

Read dropped files and encode as base64:

```rust
async fn read_file_as_base64(file: File) -> Result<String, JsValue> {
    let bytes = gloo::file::futures::read_as_bytes(&file).await?;
    Ok(base64::encode(&bytes))
}
```

### 3. Path Handling

For drag-and-drop, files don't have path context. Options:
- Drop into "uploads" subdirectory
- Show a path input dialog before upload
- Use the file's original name in working directory root

## Backend Implementation

### Message Routing

The backend simply forwards `FileUpload` messages from the browser WebSocket to the corresponding proxy WebSocket:

```rust
// In websocket handler
ProxyMessage::FileUpload { path, content_base64, mime_type } => {
    // Forward to proxy connection for this session
    if let Some(proxy_tx) = session_manager.get_proxy_sender(&session_id) {
        proxy_tx.send(message).await?;
    }
}
```

### Size Limits

Add configuration for maximum file size:

```rust
const MAX_FILE_UPLOAD_SIZE: usize = 10 * 1024 * 1024; // 10 MB
```

Validate before forwarding.

## Proxy Implementation

### File Writing

In `proxy/src/main.rs`, handle incoming file uploads:

```rust
ProxyMessage::FileUpload { path, content_base64, mime_type } => {
    let result = handle_file_upload(&working_directory, &path, &content_base64).await;

    // Send result back
    let response = ProxyMessage::FileUploadResult {
        path: path.clone(),
        success: result.is_ok(),
        error: result.err().map(|e| e.to_string()),
    };
    send_to_backend(response).await;
}

async fn handle_file_upload(
    working_dir: &Path,
    relative_path: &str,
    content_base64: &str,
) -> Result<(), Box<dyn Error>> {
    // Security: Validate path doesn't escape working directory
    let full_path = working_dir.join(relative_path);
    if !full_path.starts_with(working_dir) {
        return Err("Invalid path: attempted directory traversal".into());
    }

    // Decode base64
    let content = base64::decode(content_base64)?;

    // Create parent directories if needed
    if let Some(parent) = full_path.parent() {
        fs::create_dir_all(parent)?;
    }

    // Write file
    fs::write(&full_path, content)?;

    Ok(())
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

- Individual file: 10 MB default
- Total upload per session: 100 MB
- Configurable via environment variables

### Rate Limiting

Prevent abuse with upload rate limits:
- Max 10 files per minute
- Max 50 MB per minute

## UI/UX Design

### Visual Feedback

1. **Drag overlay**: Semi-transparent overlay when dragging files over the session
2. **Upload progress**: Show progress bar for large files
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
```

## Implementation Order

1. **Protocol**: Add `FileUpload` and `FileUploadResult` to `shared/src/lib.rs`
2. **Proxy**: Implement file writing with security checks
3. **Backend**: Add message routing for file uploads
4. **Frontend**: Implement drag-and-drop zone component
5. **Frontend**: Integrate drop zone into session view
6. **Frontend**: Add upload progress and result notifications
7. **Testing**: Test path traversal prevention, size limits, various file types

## Future Enhancements

- **Directory upload**: Support dragging folders (via `webkitdirectory`)
- **Clipboard paste**: Support pasting images/files from clipboard
- **Download files**: Allow downloading files from the session (reverse direction)
- **File browser**: Show working directory contents, allow browsing
- **Conflict handling**: Ask user before overwriting existing files

## Testing

### Manual Testing

1. Drag single file onto session view
2. Drag multiple files
3. Drag file with spaces in name
4. Drag file with unicode characters in name
5. Attempt path traversal (should fail)
6. Upload file larger than limit (should fail with clear error)
7. Upload to disconnected session (should show error)

### Automated Testing

- Unit tests for path validation
- Unit tests for base64 encoding/decoding
- Integration tests for WebSocket message flow
