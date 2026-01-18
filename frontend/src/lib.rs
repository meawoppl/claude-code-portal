mod components;
mod pages;
pub mod utils;

/// Application version from Cargo.toml (set at compile time)
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

use pages::{
    admin::AdminPage, banned::BannedPage, dashboard::DashboardPage, settings::SettingsPage,
    splash::SplashPage,
};
use yew::prelude::*;
use yew_router::prelude::*;

#[derive(Clone, Routable, PartialEq)]
pub enum Route {
    #[at("/")]
    Home,
    #[at("/dashboard")]
    Dashboard,
    #[at("/settings")]
    Settings,
    #[at("/admin")]
    Admin,
    #[at("/banned")]
    Banned,
}

fn switch(routes: Route) -> Html {
    match routes {
        Route::Home => html! { <SplashPage /> },
        Route::Dashboard => html! { <DashboardPage /> },
        Route::Settings => html! { <SettingsPage /> },
        Route::Admin => html! { <AdminPage /> },
        Route::Banned => html! { <BannedPage /> },
    }
}

#[function_component(App)]
fn app() -> Html {
    html! {
        <BrowserRouter>
            <Switch<Route> render={switch} />
        </BrowserRouter>
    }
}

#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn run_app() {
    wasm_logger::init(wasm_logger::Config::default());
    yew::Renderer::<App>::new().render();
}
