# File Upload Feature Specification

This document outlines how to implement drag-and-drop file uploads from the web dashboard to the Claude Code client running on the user's machine.

## Overview

Users should be able to drag and drop files (or use a file picker) in the web dashboard to upload files to their local Claude Code session. The files will be transmitted through the existing WebSocket connection and written to the user's working directory on their machine.

## Architecture

```
Browser (drag/drop) → WebSocket → Backend → WebSocket → Proxy → Local filesystem
```

### Components

1. **Frontend (Dashboard)**: Handle drag/drop events, read file contents, send via WebSocket
2. **Backend (Server)**: Route file upload messages between browser and proxy
3. **Proxy (CLI)**: Receive file data, write to local filesystem

## Protocol Messages

### New ProxyMessage Variants

Add to `shared/src/lib.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ProxyMessage {
    // ... existing variants ...

    /// Browser → Backend → Proxy: Request to upload a file
    FileUpload {
        /// Relative path within working directory (e.g., "src/main.rs")
        path: String,
        /// File contents as base64-encoded string
        content_base64: String,
        /// MIME type if known
        mime_type: Option<String>,
    },

    /// Proxy → Backend → Browser: File upload result
    FileUploadResult {
        path: String,
        success: bool,
        error: Option<String>,
    },
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
