use std::{env, path::Path, process::ExitCode};

use anyhow::{Context, Result, bail};
use qnx_gdb_mi::{GdbDeployment, GdbSession, GdbSessionConfig, SourceBreakpoint};
use tracing::info;
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

    if arguments.len() < 6 {
        bail!(
            "usage: {} <ntoarm-gdb> <program> <host:port> \
            <remote-program> <source_file> <line> [line...]",
            arguments[0]
        );
    }

    let gdb_path = &arguments[1];
    let program = &arguments[2];
    let target = &arguments[3];
    let remote_program = &arguments[4];
    let source = Path::new(&arguments[5]);

    let breakpoints = arguments[6..]
        .iter()
        .map(|line| {
            line.parse::<u64>()
                .map(SourceBreakpoint::new)
                .with_context(|| format!("invalid line number {line:?}"))
        })
        .collect::<Result<Vec<_>>>()?;

    let config = GdbSessionConfig::new(
        gdb_path,
        program,
        target,
        GdbDeployment::upload(remote_program),
    );

    let (mut session, _) = GdbSession::connect(config)?;

    info!(
        source = %&source.display(),
        count = breakpoints.len(),
        "setting source breakpoints"
    );

    let results = session.set_source_breakpoints(source, &breakpoints)?;

    for breakpoint in results {
        println!(
            "line={} verified={} number={:?} function={:?} \
            address={:?} resolved_file={:?} message={:?}",
            breakpoint.line,
            breakpoint.verified,
            breakpoint.number,
            breakpoint.function,
            breakpoint.address,
            breakpoint.resolved_file,
            breakpoint.message,
        );
    }

    session.shutdown()?;

    Ok(())
}

fn initialize_logging() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_ansi(false)
        .init();
}
