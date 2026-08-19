//! superx-mod-website — the public SuperX website as a module
//! (epic #213). The site source lives in `site/` (pure static, no
//! build toolchain), deploys to GitHub Pages by CI, and the OS serves
//! it locally for preview — a second server module proving same-kind
//! modules coexist (D18): ui on its port, website on this one.

use async_trait::async_trait;
use superx_kernel::types::Value;
use superx_kernel::{
    Kernel, KernelModule, KernelModuleDescriptor, NodeKind, Result, KERNEL_MODULES,
};

/// The module's name — CLI namespace, log target.
pub const MODULE_NAME: &str = "website";
/// Local preview port parameter on the module's registry entity.
pub const PORT_PARAM: &str = "attr_website_port";
const DEFAULT_PORT: u16 = 5151; // skill-allow: §9-const — bootstrap fallback, param-overridable (attr_website_port)
/// Where CI publishes the same files.
pub const PUBLIC_URL: &str = "https://k8nstantin.github.io/superx/";

/// The site, embedded (debug builds read from disk, release embeds).
#[derive(rust_embed::RustEmbed)]
#[folder = "site/"]
struct Site;

/// The website module. A ZST — the site is the state.
pub struct WebsiteModule;

/// Resolve the preview port: substrate parameter, else the marked
/// fallback (the ui module's resolved_port pattern).
pub async fn resolved_port(kernel: &Kernel) -> u16 {
    let Ok(Some(entity)) = kernel
        .find_module_by_name(NodeKind::KernelModule, MODULE_NAME)
        .await
    else {
        return DEFAULT_PORT;
    };
    match kernel.get_parameter(entity, PORT_PARAM).await {
        Ok(Some(Value::Number(n))) => n
            .to_int()
            .and_then(|i| u16::try_from(i).ok())
            .filter(|&p| p > 0)
            .unwrap_or(DEFAULT_PORT),
        _ => DEFAULT_PORT,
    }
}

async fn serve(port: u16) -> Result<()> {
    use axum::routing::get;
    let app = axum::Router::new()
        .route("/", get(|| async { asset("index.html") }))
        .fallback(get(|uri: axum::http::Uri| async move {
            asset(uri.path().trim_start_matches('/'))
        }));
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .map_err(|e| {
            superx_kernel::KernelError::Config(format!("website preview bind :{port} failed: {e}"))
        })?;
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            tracing::warn!(target: "website", "preview server exited: {e}");
        }
    });
    Ok(())
}

fn asset(path: &str) -> axum::response::Response {
    use axum::response::IntoResponse as _;
    let path = if path.is_empty() { "index.html" } else { path };
    match Site::get(path) {
        Some(file) => {
            let mime = mime_of(path);
            ([(axum::http::header::CONTENT_TYPE, mime)], file.data).into_response()
        }
        None => (axum::http::StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

fn mime_of(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "css" => "text/css",
        "js" => "application/javascript",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        _ => "application/octet-stream",
    }
}

#[async_trait]
impl KernelModule for WebsiteModule {
    fn descriptor(&self) -> KernelModuleDescriptor {
        KernelModuleDescriptor {
            name: MODULE_NAME,
            version: env!("CARGO_PKG_VERSION"),
            kind: NodeKind::KernelModule,
            depends_on: &[],
            required_metamodel: &[],
        }
    }

    async fn startup(&self, kernel: &Kernel) -> Result<()> {
        let port = resolved_port(kernel).await;
        serve(port).await?;
        tracing::info!(target: "website", port, "site preview up");
        let payload = {
            let mut o = superx_kernel::types::Object::new();
            o.insert("port".to_string(), Value::Number(i64::from(port).into()));
            Value::Object(o)
        };
        if let Err(e) = kernel.log_telemetry("website_started", payload, None).await {
            tracing::warn!(target: "website", "telemetry write failed: {e}");
        }
        Ok(())
    }

    // No schema_ddl, no dir: the first data-less module — the minimal
    // honest contract (the site itself is versioned in git).

    async fn cli(&self, kernel: &Kernel, args: &[String]) -> Result<String> {
        match args.first().map(String::as_str) {
            Some("url") => {
                let port = resolved_port(kernel).await;
                Ok(format!(
                    "local preview: http://127.0.0.1:{port}\npublic site:   {PUBLIC_URL}\n"
                ))
            }
            _ => Err(superx_kernel::KernelError::Module(
                "usage: superx website url".to_string(),
            )),
        }
    }
}

#[linkme::distributed_slice(KERNEL_MODULES)]
static WEBSITE_REGISTRATION: &'static (dyn KernelModule + Sync) = &WebsiteModule;
