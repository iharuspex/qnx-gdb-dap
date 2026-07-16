use std::{env, path::Path, process::ExitCode};

use anyhow::{Context, Result, bail};
use qnx_gdb_mi::{
    GdbDeployment, GdbSession, GdbSessionConfig, GdbSessionEvent, GdbStopReason, SourceBreakpoint,
};
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

    if arguments.len() != 7 {
        bail!(
            "usage: {} <ntoarm-gdb> <local-program> <host:port> \
             <remote-program> <source-file> <line>",
            arguments[0]
        );
    }

    let gdb_path = &arguments[1];
    let local_program = &arguments[2];
    let target = &arguments[3];
    let remote_program = &arguments[4];
    let source = Path::new(&arguments[5]);

    let line = arguments[6]
        .parse::<u64>()
        .context("invalid breakpoint line")?;

    let config = GdbSessionConfig::new(
        gdb_path,
        local_program,
        target,
        GdbDeployment::upload(remote_program),
    );

    let (mut session, _) = GdbSession::connect(config)?;

    let breakpoints = session.set_source_breakpoints(source, &[SourceBreakpoint::new(line)])?;

    let breakpoint = breakpoints
        .first()
        .context("breakpoint result is missing")?;

    if !breakpoint.verified {
        bail!("breakpoint was not verified: {:?}", breakpoint.message);
    }

    let started = session.run()?;

    info!(
        token = started.token,
        initial_event_count = started.initial_events.len(),
        "inferior execution started"
    );

    for event in started.initial_events {
        print_event(&event);
    }

    loop {
        let event = session.next_execution_event()?;
        print_event(&event);

        match event {
            GdbSessionEvent::Stopped {
                reason: GdbStopReason::Breakpoint { .. },
                ..
            } => {
                println!("breakpoint reached");
                break;
            }

            GdbSessionEvent::Stopped {
                reason: GdbStopReason::Exited { .. } | GdbStopReason::ExitedSignalled { .. },
                ..
            }
            | GdbSessionEvent::EndOfFile => {
                break;
            }

            _ => {}
        }
    }

    session.shutdown()?;

    Ok(())
}

fn print_event(event: &GdbSessionEvent) {
    match event {
        GdbSessionEvent::TargetOutput(output) => {
            print!("{output}");
        }

        GdbSessionEvent::ConsoleOutput(output) => {
            eprint!("[gdb] {output}");
        }

        GdbSessionEvent::DiagnosticOutput(output) => {
            eprint!("[gdb diagnostic] {output}");
        }

        other => {
            println!("event: {other:#?}");
        }
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
