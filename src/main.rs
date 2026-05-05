use std::fs::{OpenOptions, create_dir_all};
use std::io;
use std::path::Path;

use nvdaremoteaudio_server::config::Config;
use nvdaremoteaudio_server::server;
use tracing::{Level, error};
use tracing_appender::non_blocking::WorkerGuard;

#[tokio::main]
async fn main() -> io::Result<()> {
    let config = match Config::from_args() {
        Ok(config) => config,
        Err(err) if err.kind() == io::ErrorKind::Other => {
            eprintln!("{err}");
            return Ok(());
        }
        Err(err) => return Err(err),
    };

    let _log_guard = init_logging(config.log_path.as_deref())?;

    tokio::select! {
        result = server::run(config) => result,
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("shutdown signal received");
            Ok(())
        }
    }
}

fn init_logging(log_path: Option<&Path>) -> io::Result<WorkerGuard> {
    let (writer, guard) = match log_path {
        Some(path) => {
            if let Some(parent) = path.parent()
                && !parent.as_os_str().is_empty()
            {
                create_dir_all(parent)?;
            }

            let file = OpenOptions::new().create(true).append(true).open(path)?;
            tracing_appender::non_blocking(file)
        }
        None => tracing_appender::non_blocking(std::io::stdout()),
    };

    tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .with_target(false)
        .with_thread_ids(true)
        .with_ansi(false)
        .with_writer(writer)
        .compact()
        .init();

    std::panic::set_hook(Box::new(|panic_info| {
        error!(details = %panic_info, "panic");
    }));

    Ok(guard)
}
