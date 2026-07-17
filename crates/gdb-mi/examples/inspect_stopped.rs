use std::{env, path::Path, process::ExitCode};

use anyhow::{Context, Result, bail};
use qnx_gdb_mi::{
    GdbDeployment, GdbDisconnectMode, GdbSession, GdbSessionConfig, GdbSessionEvent, GdbStopReason,
    SourceBreakpoint,
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

    let stopped_thread_id = loop {
        let event = session.next_execution_event()?;
        print_event(&event);

        match event {
            GdbSessionEvent::Stopped {
                reason: GdbStopReason::Breakpoint { .. },
                thread_id,
                ..
            } => {
                let thread_id = thread_id.unwrap_or(1);

                println!("breakpoint reached in thread {thread_id}");

                break Some(thread_id);
            }

            GdbSessionEvent::Stopped {
                reason: GdbStopReason::Exited { .. } | GdbStopReason::ExitedSignalled { .. },
                ..
            }
            | GdbSessionEvent::EndOfFile => {
                break None;
            }

            _ => {}
        }
    };

    if let Some(thread_id) = stopped_thread_id {
        let threads = session.threads()?;

        println!("threads:");

        for thread in &threads {
            println!(
                "  id={} name={} current={}",
                thread.id, thread.name, thread.current
            );
        }

        let frames = session.stack_frames(thread_id, 0, 20)?;

        println!("stack:");

        for frame in &frames {
            let source = frame
                .fullname
                .as_ref()
                .or(frame.file.as_ref())
                .map_or("<unknown>", |path| {
                    path.to_str().unwrap_or("<non-utf8 path>")
                });

            let function = frame.function.as_deref().unwrap_or("<unknown>");

            let line = frame.line.unwrap_or(0);

            let address = frame
                .address
                .map(|address| format!("0x{address:x}"))
                .unwrap_or_else(|| "<unknown>".to_owned());

            println!(
                "  #{} {} at {}:{} address={}",
                frame.level, function, source, line, address
            );
        }

        let shutdown = session.disconnect(GdbDisconnectMode::Detach)?;

        info!(
            status = %shutdown.status,
            shutdown_event_count = shutdown.events.len(),
            "GDB session disconnected"
        );

        for event in shutdown.events {
            print_event(&event);
        }
    } else {
        session.shutdown()?;
    }

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
