use gloo::utils::window;
use yew::prelude::*;

#[function_component(BannedPage)]
pub fn banned_page() -> Html {
    // Extract reason from URL query params
    let reason = window()
        .location()
        .search()
        .ok()
        .and_then(|search| {
            let params = web_sys::UrlSearchParams::new_with_str(&search).ok()?;
            params.get("reason")
        })
        .map(|r| {
            // URL decode the reason
            js_sys::decode_uri_component(&r)
                .ok()
                .map(|s| s.as_string().unwrap_or_default())
                .unwrap_or(r)
        })
        .unwrap_or_else(|| "No reason provided".to_string());

    html! {
        <div class="banned-container">
            <div class="banned-content">
                <div class="banned-icon">{ "🚫" }</div>
                <h1>{ "Account Suspended" }</h1>
                <p class="banned-message">
                    { "Your account has been suspended and you are unable to access this service." }
                </p>
                <div class="banned-reason">
                    <h3>{ "Reason:" }</h3>
                    <p>{ reason }</p>
                </div>
                <p class="banned-contact">
                    { "If you believe this is an error, please contact " }
                    <a href="mailto:support@txcl.io">{ "support@txcl.io" }</a>
                </p>
            </div>
        </div>
    }
}
