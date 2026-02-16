use web_sys::window;

/// Get the base HTTP URL (e.g., "http://localhost:3000" or "https://myapp.com")
pub fn get_base_url() -> String {
    let window = window().expect("no global window");
    let location = window.location();

    let protocol = location.protocol().unwrap_or_else(|_| "http:".to_string());
    let host = location
        .host()
        .unwrap_or_else(|_| "localhost:3000".to_string());

    format!("{}//{}", protocol, host)
}

/// Get the WebSocket URL (e.g., "ws://localhost:3000" or "wss://myapp.com")
pub fn get_ws_url() -> String {
    let window = window().expect("no global window");
    let location = window.location();

    let protocol = location.protocol().unwrap_or_else(|_| "http:".to_string());
    let ws_protocol = if protocol == "https:" { "wss:" } else { "ws:" };
    let host = location
        .host()
        .unwrap_or_else(|_| "localhost:3000".to_string());

    format!("{}//{}", ws_protocol, host)
}

/// Build a full API URL from a path (e.g., "/api/sessions" -> "http://localhost:3000/api/sessions")
pub fn api_url(path: &str) -> String {
    format!("{}{}", get_base_url(), path)
}

/// Build a full WebSocket URL from a path (e.g., "/ws/client" -> "ws://localhost:3000/ws/client")
pub fn ws_url(path: &str) -> String {
    format!("{}{}", get_ws_url(), path)
}

/// Extract hostname from session_name (format: "hostname-YYYYMMDD-HHMMSS")
pub fn extract_hostname(session_name: &str) -> &str {
    let mut dash_count = 0;
    for (i, c) in session_name.bytes().enumerate().rev() {
        if c == b'-' {
            dash_count += 1;
            if dash_count == 2 {
                return &session_name[..i];
            }
        }
    }
    session_name
}

/// Extract folder name from path (last path component)
pub fn extract_folder(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    trimmed
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or(trimmed)
}
