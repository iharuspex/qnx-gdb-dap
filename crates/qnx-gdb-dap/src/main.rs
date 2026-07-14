use std::io::{self, BufReader, BufWriter};

use anyhow::Result;
use qnx_dap::{DapReader, DapWriter, Event, OutgoingMessage, Request, Response};
use serde_json::json;
use tracing::{debug, info, warn};
use tracing_subscriber::EnvFilter;

fn main() -> Result<()> {
    initialize_logging();

    info!(
        adapter_version = qnx_dap::VERSION,
        gdb_version = qnx_gdb_mi::REFERENCE_GDB_VERSION,
        "starting QNX GDB debug adapter"
    );

    run_server()
}

fn run_server() -> Result<()> {
    let stdin = io::stdin();
    let stdout = io::stdout();

    let mut reader = DapReader::new(BufReader::new(stdin.lock()));
    let mut writer = DapWriter::new(BufWriter::new(stdout.lock()));
    let mut sequence = SequenceGenerator::new();

    while let Some(request) = reader.read_message::<Request>()? {
        debug!(
            request_seq = request.seq,
            command = %request.command,
            "received DAP request"
        );

        handle_request(&request, &mut writer, &mut sequence)?;
    }

    info!("DAP input stream closed");

    Ok(())
}

fn handle_request<W>(
    request: &Request,
    writer: &mut DapWriter<W>,
    sequence: &mut SequenceGenerator,
) -> Result<()>
where
    W: io::Write,
{
    match request.command.as_str() {
        "initialize" => handle_initialize(request, writer, sequence),
        command => {
            warn!(command = %command, "unsupported DAP request");

            let response = Response::error(
                sequence.next(),
                request,
                format!("DAP command {command:?} is not implemented"),
            );

            writer.write_message(&OutgoingMessage::Response(response))?;

            Ok(())
        }
    }
}

fn handle_initialize<W>(
    request: &Request,
    writer: &mut DapWriter<W>,
    sequence: &mut SequenceGenerator,
) -> Result<()>
where
    W: io::Write,
{
    let capabilities = json!({
        "supportsConfigurationDoneRequest": false,
        "supportsFunctionBreakpoints": false,
        "supportsConditionalBreakpoints": false,
        "supportsEvaluateForHovers": false,
        "supportsTerminateRequest": false,
        "supportsRestartRequest": false
    });

    let response = Response::success(sequence.next(), request, Some(capabilities));

    writer.write_message(&OutgoingMessage::Response(response))?;

    let initialized = Event::new(sequence.next(), "initialized");
    writer.write_message(&OutgoingMessage::Event(initialized))?;

    Ok(())
}

fn initialize_logging() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();
}

#[derive(Debug)]
struct SequenceGenerator {
    next: u64,
}

impl SequenceGenerator {
    const fn new() -> Self {
        Self { next: 1 }
    }

    fn next(&mut self) -> u64 {
        let current = self.next;
        self.next += 1;
        current
    }
}
