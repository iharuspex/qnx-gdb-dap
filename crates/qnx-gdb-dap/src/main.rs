// use anyhow::Result;
use tracing::info;
use tracing_subscriber::EnvFilter;

fn main() {
    initialize_logging();

    info!(
        adapter_version = qnx_dap::VERSION,
        gdb_version = qnx_gdb_mi::REFERENCE_GDB_VERSION,
        "starting QNX GDB debug adapter"
    );

    // Ok(())
}

fn initialize_logging() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();
}
