use yew::prelude::*;

#[function_component(AccessDeniedPage)]
pub fn access_denied_page() -> Html {
    html! {
        <div class="banned-container">
            <div class="banned-content">
                <div class="banned-icon">{ "🔒" }</div>
                <h1>{ "Access Denied" }</h1>
                <p class="banned-message">
                    { "Your email address is not authorized to access this service." }
                </p>
                <div class="banned-reason">
                    <h3>{ "What happened?" }</h3>
                    <p>{ "This portal restricts access to specific email addresses or domains. Your email is not on the allowlist." }</p>
                </div>
                <p class="banned-contact">
                    { "If you believe this is an error, please contact the portal administrator." }
                </p>
            </div>
        </div>
    }
}
