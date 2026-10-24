//! The module's own URL (operator, 2026-10-24): `http://127.0.0.1:<port>/`,
//! on the port `attr_repos_port` names, answering what `superx-repos
//! status` says. It is this process's alone: the core UI on :5150 never
//! serves it, and links to it through the registry (`attr_module_ui_url`).

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use axum::Router;
use superx_kernel::Kernel;

/// The module's routes.
pub fn app(kernel: Kernel) -> Router {
    Router::new().route("/", get(status)).route("/status", get(status)).with_state(kernel)
}

/// The status, as text; when the substrate does not answer, that.
async fn status(State(kernel): State<Kernel>) -> (StatusCode, String) {
    match crate::status::render(&kernel).await {
        Ok(text) => (StatusCode::OK, text),
        Err(e) => (StatusCode::SERVICE_UNAVAILABLE, format!("superx-repos: the substrate did not answer: {e}\n")),
    }
}
