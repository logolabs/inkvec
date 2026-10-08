//! `inkvec-server`: the binary. Argument parsing and process wiring only; every endpoint and
//! all request handling lives in the library (`src/lib.rs`) so the integration tests build
//! the same router in-process.

use inkvec_server::{app, init_logging, AppState};
use std::net::SocketAddr;

fn main() -> std::process::ExitCode {
    run_main(std::env::args())
}

fn run_main<I: IntoIterator<Item = String>>(args: I) -> std::process::ExitCode {
    // `--healthcheck`: Docker's HEALTHCHECK for the distroless image, which has no shell,
    // curl or wget. A short-lived process that makes one request to its own /healthz and
    // exits 0 or 1; it never starts the server.
    if args.into_iter().skip(1).any(|a| a == "--healthcheck") {
        return match inkvec_server::healthcheck() {
            Ok(()) => std::process::ExitCode::SUCCESS,
            Err(reason) => {
                eprintln!("inkvec-server --healthcheck: {reason}");
                std::process::ExitCode::FAILURE
            }
        };
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

/// Bind `0.0.0.0:<INKVEC_PORT>`, serve the router until a shutdown signal, and turn a bind or
/// serve failure into a logged error and exit code 1.
async fn serve() -> std::process::ExitCode {
    let port = AppState::port();
    let addr = SocketAddr::from(([0, 0, 0, 0], port));

    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!(%addr, error = %e, "failed to bind");
            return std::process::ExitCode::FAILURE;
        }
    };
    serve_with(listener, shutdown_signal()).await
}

async fn serve_with<F>(listener: tokio::net::TcpListener, shutdown: F) -> std::process::ExitCode
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    let state = AppState::from_env();
    tracing::info!(
        addr = %listener.local_addr().unwrap_or_else(|_| SocketAddr::from(([0, 0, 0, 0], 0))),
        version = inkvec::version(),
        build_target = inkvec::build_target(),
        "inkvec-server listening"
    );

    let result = axum::serve(listener, app(state))
        .with_graceful_shutdown(shutdown)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_main_healthcheck_flag() {
        let code = run_main(vec![
            "inkvec-server".to_string(),
            "--healthcheck".to_string(),
        ]);
        assert_eq!(code, std::process::ExitCode::FAILURE);
    }

    #[test]
    fn test_main_healthcheck_success() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind a free port");
        let port = listener.local_addr().expect("bound").port();
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = listener.accept() {
                let mut buf = [0u8; 512];
                let _ = s.read(&mut buf);
                let _ = s.write_all(b"HTTP/1.1 200 OK\r\n\r\nok");
            }
        });
        std::env::set_var("INKVEC_PORT", port.to_string());
        let code = run_main(vec![
            "inkvec-server".to_string(),
            "--healthcheck".to_string(),
        ]);
        std::env::remove_var("INKVEC_PORT");
        assert_eq!(code, std::process::ExitCode::SUCCESS);
    }

    #[test]
    fn test_serve_with_shutdown() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let code = serve_with(listener, std::future::ready(())).await;
            assert_eq!(code, std::process::ExitCode::SUCCESS);
        });
    }
}
