//! The module's own URL (operator, 2026-10-24): `http://127.0.0.1:<port>/`,
//! on the port `attr_recon_port` names, answering what
//! `superx-recon status` says. It is this process's alone: the core
//! UI on :5150 never serves it.
//!
//! It answers only requests addressed to it by name: `127.0.0.1` or
//! `localhost` at its own port (#466's rules, review finding X1). It will
//! serve the operator's prompts and recaps, and a page from anywhere else
//! could otherwise read them by pointing a name of its own at 127.0.0.1
//! (DNS rebinding); the browser sends that name as the request's Host.

use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use superx_kernel::Kernel;

use crate::instance::Instance;

/// The port HTTP leaves out of a request's Host.
const HTTP_DEFAULT_PORT: u16 = 80; // skill-allow: §9-const — HTTP's own default port (RFC 9110), not a policy tunable

/// What the module's routes answer from: the kernel, and the instance
/// whose module folder status shows.
#[derive(Clone)]
struct Page {
    kernel: Kernel,
    instance: Instance,
}

/// The module's routes, for the server listening on `port`.
pub fn app(kernel: Kernel, instance: Instance, port: u16) -> Router {
    Router::new()
        .route("/", get(status))
        .route("/status", get(status))
        .with_state(Page { kernel, instance })
        .layer(middleware::from_fn_with_state(port, only_its_own_name))
}

/// The status, as text; when the substrate does not answer, that.
async fn status(State(page): State<Page>) -> (StatusCode, String) {
    match crate::status::render(&page.kernel, &page.instance).await {
        Ok(text) => (StatusCode::OK, text),
        Err(e) => (StatusCode::SERVICE_UNAVAILABLE, format!("superx-recon: the substrate did not answer: {e}\n")),
    }
}

/// Every request, before any route: answered only when it names this
/// server, refused otherwise.
async fn only_its_own_name(State(port): State<u16>, request: Request, next: Next) -> Response {
    let named = request
        .headers()
        .get(header::HOST)
        .and_then(|host| host.to_str().ok())
        .or_else(|| request.uri().authority().map(axum::http::uri::Authority::as_str));
    if named.is_some_and(|host| is_its_own(host, port)) {
        next.run(request).await
    } else {
        (StatusCode::FORBIDDEN, format!("superx-recon answers only at http://127.0.0.1:{port}\n")).into_response()
    }
}

/// Whether a request's Host names this server: `127.0.0.1` or `localhost`,
/// at `port`, which HTTP leaves out when it is 80.
#[must_use]
pub fn is_its_own(host: &str, port: u16) -> bool {
    let (name, named_port) = match host.rsplit_once(':') {
        Some((name, p)) => match p.parse::<u16>() {
            Ok(p) => (name, p),
            Err(_) => return false,
        },
        None => (host, HTTP_DEFAULT_PORT),
    };
    named_port == port && (name == "127.0.0.1" || name.eq_ignore_ascii_case("localhost"))
}

#[cfg(test)]
mod tests {
    use super::is_its_own;

    #[test]
    fn its_own_name_is_the_loopback_address_or_localhost_at_its_port() {
        for host in ["127.0.0.1:5153", "localhost:5153", "LOCALHOST:5153"] {
            assert!(is_its_own(host, 5153), "{host}");
        }
        for host in [
            "evil.example:5153",
            "127.0.0.1.evil.example:5153",
            "localhost.:5153",
            "127.0.0.1:5154",
            "127.0.0.1",
            "localhost",
            "[::1]:5153",
            "0.0.0.0:5153",
            "127.0.0.1:",
            "127.0.0.1:x",
            "",
        ] {
            assert!(!is_its_own(host, 5153), "{host}");
        }
        assert!(is_its_own("127.0.0.1", 80) && is_its_own("localhost:80", 80), "HTTP leaves out port 80");
    }
}
