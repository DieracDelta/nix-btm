use std::fs::File;

use rustix::process::getpid;
use tracing_subscriber::{
    EnvFilter, layer::SubscriberExt, util::SubscriberInitExt,
};

use crate::Args;

pub(crate) fn init_tracing(args: &Args) {
    let log_path: &str = match args {
        Args::Daemon {
            daemon_log_path, ..
        } => match daemon_log_path {
            Some(path) => path,
            None => {
                let pid = getpid();
                &format!("/tmp/nixbtm-daemon-{pid}.log")
            }
        },
        Args::Client {
            client_log_path, ..
        } => match client_log_path {
            Some(path) => path,
            None => {
                let pid = getpid();
                &format!("/tmp/nixbtm-client-{pid}.log")
            }
        },
        Args::Standalone {
            standalone_log_path,
            ..
        } => match standalone_log_path {
            Some(path) => path,
            None => {
                let pid = getpid();
                &format!("/tmp/nixbtm-standalone-{pid}.log")
            }
        },
    };

    let file = File::create(log_path).expect("Could not initialize log");

    let env_filter = EnvFilter::from_default_env();

    #[cfg(tokio_unstable)]
    {
        // Tokio console layer (spawns a background task; must be called inside
        // a Tokio runtime)
        let console_layer = console_subscriber::ConsoleLayer::builder()
            .with_default_env() // honors TOKIO_CONSOLE_* env vars
            .spawn();

        tracing_subscriber::registry()
            .with(env_filter)
            .with(console_layer)
            .with(
                tracing_subscriber::fmt::layer()
                    .with_writer(file)
                    .with_target(false),
            )
            .init();
    }

    #[cfg(not(tokio_unstable))]
    {
        tracing_subscriber::registry()
            .with(env_filter)
            .with(
                tracing_subscriber::fmt::layer()
                    .with_writer(file)
                    .with_target(false),
            )
            .init();
    }
}
