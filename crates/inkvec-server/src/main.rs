//! `inkvec-server`: the binary. Argument parsing and process wiring only; every endpoint and
//! all request handling lives in the library (`src/lib.rs`) so the integration tests build
//! the same router in-process.

use inkvec_server::{app, init_logging, AppState};
use std::net::SocketAddr;

fn main() -> std::process::ExitCode {
    // `--healthcheck`: Docker's HEALTHCHECK for the distroless image, which has no shell,
    // curl or wget. A short-lived process that makes one request to its own /healthz and
    // exits 0 or 1; it never starts the server.
    if std::env::args().skip(1).any(|a| a == "--healthcheck") {
        return std::process::ExitCode::from(inkvec_server::healthcheck() as u8);
    }

    init_logging();
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("inkvec-server: failed to start the async runtime: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };
    runtime.block_on(serve())
}

async fn serve() -> std::process::ExitCode {
    let state = AppState::from_env();
    let port = AppState::port();
    let addr = SocketAddr::from(([0, 0, 0, 0], port));

    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!(%addr, error = %e, "failed to bind");
            return std::process::ExitCode::FAILURE;
        }
    };
    tracing::info!(
        %addr,
        version = inkvec::version(),
        build_target = inkvec::build_target(),
        "inkvec-server listening"
    );

    let result = axum::serve(listener, app(state))
        .with_graceful_shutdown(shutdown_signal())
        .await;
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!(error = %e, "server error");
            std::process::ExitCode::FAILURE
        }
    }
}

/// Resolves on Ctrl-C or, on Unix, SIGTERM (the signal Docker sends on `docker stop`), so
/// `axum::serve` finishes in-flight requests before the process exits.
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(e) => tracing::warn!(error = %e, "failed to install SIGTERM handler"),
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => tracing::info!("received Ctrl-C, shutting down"),
        () = terminate => tracing::info!("received SIGTERM, shutting down"),
    }
}
