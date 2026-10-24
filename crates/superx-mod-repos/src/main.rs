//! `superx-repos`: the repos module's own executable (operator,
//! 2026-10-24). Its own process, database and URL, beside the `superx`
//! daemon and independent of it.
//!
//! - `superx-repos --initialize`: provision the module's own database
//!   (`superx/repos`), asking for the root password once. The §13
//!   carve-out, as `superx --initialize` uses it: the password is used for
//!   that one session and never stored.
//! - `superx-repos start | stop | status`
//! - `superx-repos run`: the process itself, in the foreground; `start`
//!   runs it in the background.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::process::ExitCode;
use std::time::Instant;

use clap::{Parser, Subcommand};
use superx_kernel::Kernel;
use superx_mod_repos::instance::{Instance, PASSWORD_ENV};
use superx_mod_repos::{daemon, params, registry, server, status, MODULE_NAME, SCHEMA_DDL};
use tokio::net::TcpListener;
use tokio::signal::unix::{signal, Signal, SignalKind};

#[derive(Parser)]
#[command(
    name = "superx-repos",
    version,
    about = "The repositories the agents work in, read with git, read-only, and kept in the repos module's own database"
)]
struct Cli {
    /// The SuperX instance's home, as `superx --home` takes it.
    #[arg(long, global = true, env = "SUPERX_HOME", default_value = ".")] // skill-allow: §9-default — the instance anchor itself
    home: PathBuf,
    /// Provision the module's own database, asking for the root password once.
    #[arg(long)]
    initialize: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Start the module in the background.
    Start,
    /// Stop the running module.
    Stop,
    /// Whether it runs, its URL, what its database holds and its parameters.
    Status,
    /// Run the module in the foreground: what `start` runs.
    Run,
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let instance = match Instance::read(&cli.home) {
        Ok(i) => i,
        Err(e) => return fail(&e),
    };
    let result = if cli.initialize {
        initialize(&instance).await
    } else {
        match cli.command {
            Some(Command::Start) => {
                std::env::current_exe().map_err(|e| e.to_string()).and_then(|exe| daemon::start(&instance, &exe))
            }
            Some(Command::Stop) => daemon::stop(&instance),
            Some(Command::Status) | None => report(&instance).await,
            Some(Command::Run) => run(&instance).await.map(|()| String::new()),
        }
    };
    match result {
        Ok(text) => {
            print!("{text}");
            ExitCode::SUCCESS
        }
        Err(e) => fail(&e),
    }
}

fn fail(e: &str) -> ExitCode {
    eprintln!("superx-repos: {e}");
    ExitCode::FAILURE
}

/// Connect to the instance's substrate as its service account.
async fn connect(instance: &Instance) -> Result<Kernel, String> {
    let Some(password) = instance.password() else {
        return Err(format!("no credentials: export {PASSWORD_ENV} or run `superx --initialize` first"));
    };
    Kernel::connect_service_with_password(&instance.endpoint, &instance.namespace, &instance.database, &password)
        .await
        .map(|kernel| kernel.with_home(instance.home.clone()))
        .map_err(|e| format!("cannot reach the substrate at {}: {e}", instance.endpoint))
}

/// Provision the module's own database. Not registered here: a module
/// provisioned and not yet started is not down.
async fn initialize(instance: &Instance) -> Result<String, String> {
    let Some(password) = instance.password() else {
        return Err("this instance has no credentials: run `superx --initialize` first".to_string());
    };
    let root = prompt_root()?;
    let applied = superx_kernel::provision::provision_module_schema(
        &instance.endpoint,
        MODULE_NAME,
        &root,
        &password,
        SCHEMA_DDL,
    )
    .await;
    match applied {
        Ok(()) => Ok(format!(
            "module '{MODULE_NAME}' provisioned: its own database superx/{MODULE_NAME} and service account \
             superx_mod_{MODULE_NAME}\nstart it with `superx-repos start`\n"
        )),
        Err(e) if superx_kernel::provision::is_already_exists(&e.to_string()) => {
            Ok(format!("module '{MODULE_NAME}' is already provisioned: start it with `superx-repos start`\n"))
        }
        Err(e) => Err(e.to_string()),
    }
}

/// The root password, asked once: hidden at a terminal, a line otherwise.
fn prompt_root() -> Result<String, String> {
    use std::io::IsTerminal as _;
    let password = if std::io::stdin().is_terminal() {
        rpassword::prompt_password("Root password for this SuperX instance: ")
            .map_err(|e| format!("password prompt: {e}"))?
    } else {
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).map_err(|e| format!("password read: {e}"))?;
        line.trim_end_matches(['\r', '\n']).to_string()
    };
    if password.is_empty() {
        return Err("no root password given".to_string());
    }
    Ok(password)
}

/// `status`: whether it runs, said first and even when the substrate does
/// not answer; then its URL and what the substrate says.
async fn report(instance: &Instance) -> Result<String, String> {
    match daemon::live_pid(instance) {
        Some(pid) => println!("running     pid {pid}"),
        None => println!("running     no: start it with `superx-repos start`"),
    }
    let kernel = connect(instance).await?;
    let resolved = params::resolve(&kernel).await.map_err(|e| e.to_string())?;
    let mut out = format!("url         http://127.0.0.1:{}\n", resolved.params.port);
    out.push_str(&status::render(&kernel).await.map_err(|e| e.to_string())?);
    Ok(out)
}

/// The process itself, in the order the kernel's boot starts a module:
/// registered and starting; listening on its URL and active; serving until
/// asked to stop; then stopped. A stop that comes while it starts, such as
/// while the substrate is slow to answer, ends it there.
async fn run(instance: &Instance) -> Result<(), String> {
    if let Some(pid) = daemon::live_pid(instance) {
        return Err(format!("already running (pid {pid}): `superx-repos stop` first"));
    }
    let _log = superx_kernel::logging::init_with_filter(&instance.log_dir(), &instance.log_filter)
        .map_err(|e| e.to_string())?;
    // Listening for the stop signals before anything says it runs. Once a
    // listener exists, a signal no longer ends the process by itself, so
    // every step of the start is raced against it.
    let interrupt = signal(SignalKind::interrupt()).map_err(|e| format!("SIGINT cannot be watched: {e}"))?;
    let terminate = signal(SignalKind::terminate()).map_err(|e| format!("SIGTERM cannot be watched: {e}"))?;
    let mut stop = Box::pin(stop_signal(interrupt, terminate));
    let Some(kernel) = unless_stopped(&mut stop, connect(instance)).await else {
        tracing::info!(target: "repos", "stopped before it reached the substrate");
        return Ok(());
    };
    let kernel = kernel?;
    let started = Instant::now();
    let up = unless_stopped(&mut stop, async {
        let entity = registry::starting(&kernel).await.map_err(|e| e.to_string())?;
        let (listener, url) = listen(&kernel).await?;
        let startup_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        registry::active(&kernel, entity, &url, startup_ms).await.map_err(|e| e.to_string())?;
        let pid = daemon::PidFile::write(instance)?;
        Ok::<_, String>((listener, url, pid))
    })
    .await;
    let (listener, url, _pid) = match up {
        Some(Ok(up)) => up,
        Some(Err(why)) => {
            tracing::error!(target: "repos", %why, "not started");
            told(registry::failed(&kernel, &why).await, "failed");
            return Err(why);
        }
        None => {
            tracing::info!(target: "repos", "stopped while starting");
            told(registry::stopped(&kernel).await, "stopped");
            return Ok(());
        }
    };
    tracing::info!(target: "repos", %url, "running");
    let served = axum::serve(listener, server::app(kernel.clone())).with_graceful_shutdown(stop).await;
    match &served {
        Ok(()) => told(registry::stopped(&kernel).await, "stopped"),
        Err(e) => told(registry::failed(&kernel, &format!("serving {url}: {e}")).await, "failed"),
    }
    tracing::info!(target: "repos", "stopped");
    served.map_err(|e| e.to_string())
}

/// A registry write that did not go through is logged; the process goes on.
fn told(result: superx_kernel::Result<()>, what: &str) {
    if let Err(e) = result {
        tracing::warn!(target: "repos", error = %e, "the registry was not told it {what}");
    }
}

/// `step`, unless a stop signal comes first: then `None`.
async fn unless_stopped<T, S: Future<Output = ()>>(stop: &mut Pin<Box<S>>, step: impl Future<Output = T>) -> Option<T> {
    tokio::select! {
        out = step => Some(out),
        () = stop.as_mut() => None,
    }
}

/// Its port from its parameters, and the listener on it.
async fn listen(kernel: &Kernel) -> Result<(TcpListener, String), String> {
    let port = params::resolve(kernel).await.map_err(|e| e.to_string())?.params.port;
    let listener =
        TcpListener::bind(("127.0.0.1", port)).await.map_err(|e| format!("cannot listen on 127.0.0.1:{port}: {e}"))?;
    Ok((listener, format!("http://127.0.0.1:{port}")))
}

/// Resolves on SIGINT (`superx-repos stop`, ctrl-c) or SIGTERM.
async fn stop_signal(mut interrupt: Signal, mut terminate: Signal) {
    tokio::select! {
        _ = interrupt.recv() => {}
        _ = terminate.recv() => {}
    }
}
