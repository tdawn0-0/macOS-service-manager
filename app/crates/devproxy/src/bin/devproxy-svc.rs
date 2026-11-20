//! Hidden process modes for the two launchd jobs.
//!
//! ```text
//! devproxy-svc serve <root>    # HTTPS proxy listener
//! devproxy-svc tunnel <root>   # Cloudflare Worker supervisor
//! ```
//!
//! The `--dev` prefix is accepted so the Go and Rust builds can share one
//! launchd job definition during cutover.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use devproxy::proxy::server;
use devproxy::tunnel::client;
use tokio::sync::watch;

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("devproxy: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let mut mode = args.next();
    if mode.as_deref() == Some("--dev") {
        mode = args.next();
    }
    let mode = mode.ok_or_else(usage)?;
    let root = args.next().ok_or_else(usage)?;
    if args.next().is_some() {
        return Err(usage());
    }
    let root = PathBuf::from(root);
    if !root.is_absolute() {
        return Err("the development root must be an absolute path".into());
    }

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            let _ = shutdown_tx.send(true);
        }
    });

    match mode.as_str() {
        "serve" => server::serve(root, shutdown_rx)
            .await
            .map_err(|error| error.to_string()),
        "tunnel" => {
            let event: Arc<dyn Fn(&str) + Send + Sync> = Arc::new(|message: &str| {
                eprintln!("{message}");
            });
            client::serve(root, event, shutdown_rx)
                .await
                .map_err(|error| error.to_string())
        }
        other => Err(format!("unknown mode {other:?}\n{}", usage())),
    }
}

fn usage() -> String {
    "usage: devproxy-svc [--dev] <serve|tunnel> <root>".into()
}
