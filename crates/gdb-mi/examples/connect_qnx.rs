use std::{env, process::ExitCode};

use anyhow::{Context, Result};
use qnx_gdb_mi::{GdbDeployment, GdbEvent, GdbSession, GdbSessionConfig, MiRecord};
use tracing::{debug, info};
use tracing_subscriber::EnvFilter;

fn main() -> ExitCode {
    initialize_logging();

    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let arguments = env::args().collect::<Vec<_>>();

    if arguments.len() != 4 {
        anyhow::bail!(
            "usage: {} <ntoarm-gdb> <program> <host:port> <remote-program>",
            arguments[0]
        );
    }

    let gdb_path = &arguments[1];
    let program = &arguments[2];
    let target = &arguments[3];
    let remote_program = &arguments[4];

    let config = GdbSessionConfig::new(
        gdb_path,
        program,
        target,
        GdbDeployment::existing(remote_program),
    );

    let (mut session, output) = GdbSession::connect(config)
        .with_context(|| format!("failed to connect GDB to target {target}"))?;

    info!(
        pid = session.gdb_process_id(),
        ?target,
        state = ?session.state(),
        "debug session established"
    );

    print_records("startup", output.startup_records);

    print_events("version", output.version_events);
    print_events("symbols", output.symbol_events);
    print_events("target", output.target_events);

    session.shutdown()?;

    Ok(())
}

fn print_records(label: &str, records: Vec<MiRecord>) {
    for record in records {
        debug!(%label, ?record, "GDB record");
    }
}

fn print_events(label: &str, events: Vec<GdbEvent>) {
    for event in events {
        debug!(%label, ?event, "GDB event");
    }
}

fn initialize_logging() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_ansi(false)
        .init();
}
