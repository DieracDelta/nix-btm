use std::fs::File;

use tracing_subscriber::{
    EnvFilter, layer::SubscriberExt, util::SubscriberInitExt,
};

pub fn init_tracing() {
    // TODO make this a cli option
    let file = File::create("/tmp/nixbtm.log").unwrap();

    // Tokio console layer (spawns a background task; must be called inside a
    // Tokio runtime)
    let console_layer = console_subscriber::ConsoleLayer::builder()
        .with_default_env() // honors TOKIO_CONSOLE_* env vars
        .spawn();
    let env_filter = EnvFilter::from_default_env();

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
