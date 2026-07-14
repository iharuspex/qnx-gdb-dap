use std::io::{self, BufReader, BufWriter};

use anyhow::Result;
use qnx_dap::{DapReader, DapWriter};
use qnx_gdb_dap::DebugAdapter;
use tracing::info;
use tracing_subscriber::EnvFilter;

fn main() -> Result<()> {
    initialize_logging();

    info!(
        adapter_version = qnx_dap::VERSION,
        gdb_version = qnx_gdb_mi::REFERENCE_GDB_VERSION,
        "starting QNX GDB debug adapter"
    );

    let stdin = io::stdin();
    let stdout = io::stdout();

    let mut reader = DapReader::new(BufReader::new(stdin.lock()));
    let mut writer = DapWriter::new(BufWriter::new(stdout.lock()));
    let mut adapter = DebugAdapter::new();

    adapter.run(&mut reader, &mut writer)
}

fn initialize_logging() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();
}
