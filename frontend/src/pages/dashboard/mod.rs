//! Dashboard page components
//!
//! This module contains the main dashboard page and its sub-components:
//! - `DashboardPage`: Main orchestrating component
//! - `SessionRail`: Horizontal carousel of session pills
//! - `SessionView`: Terminal view for a single session

mod page;
mod session_rail;
mod session_view;
mod types;

pub use page::DashboardPage;
