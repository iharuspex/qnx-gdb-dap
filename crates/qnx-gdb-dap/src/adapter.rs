use std::io::{BufRead, Write};

use crate::LaunchArguments;
use anyhow::Result;
use qnx_dap::{DapReader, DapWriter, Event, OutgoingMessage, Request, Response};
use qnx_gdb_mi::{GdbEvent, GdbSession, GdbSessionConfig, GdbSessionOutput, MiRecord};
use serde_json::json;
use tracing::{debug, info, warn};

/// Current lifecycle state of the debug adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdapterState {
    /// The adapter has started but has not received `initialize`.
    Created,

    /// The DAP client has successfully initialized the adapter.
    Initialized,

    /// A launch request is currently being processed.
    Launching,

    /// GDB is connected to the remote QNX target.
    Connected,

    /// The DAP input stream has been closed.
    Disconnected,
}

/// Stateful DAP request handler.
#[derive(Debug)]
pub struct DebugAdapter {
    sequence: SequenceGenerator,
    state: AdapterState,
    session: Option<GdbSession>,
}

impl DebugAdapter {
    /// Creates a new debug adapter.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            sequence: SequenceGenerator::new(),
            state: AdapterState::Created,
            session: None,
        }
    }

    /// Returns the current adapter state.
    #[must_use]
    pub const fn state(&self) -> AdapterState {
        self.state
    }

    /// Processes DAP requests until the input stream reaches EOF.
    ///
    /// # Errors
    ///
    /// Returns an error if a DAP message cannot be read, deserialized,
    /// serialized, or written.
    pub fn run<R, W>(&mut self, reader: &mut DapReader<R>, writer: &mut DapWriter<W>) -> Result<()>
    where
        R: BufRead,
        W: Write,
    {
        while let Some(request) = reader.read_message::<Request>()? {
            debug!(
                request_seq = request.seq,
                command = %request.command,
                state = ?self.state,
                "received DAP request"
            );

            self.handle_request(&request, writer)?;
        }

        if let Some(session) = self.session.as_mut()
            && let Err(error) = session.shutdown()
        {
            warn!(%error, "failed to shut down GDB session");
        }

        self.session = None;
        self.state = AdapterState::Disconnected;
        info!("DAP input stream closed");

        Ok(())
    }

    fn handle_request<W>(&mut self, request: &Request, writer: &mut DapWriter<W>) -> Result<()>
    where
        W: Write,
    {
        match request.command.as_str() {
            "initialize" => self.handle_initialize(request, writer),
            "launch" => self.handle_launch(request, writer),
            command => {
                warn!(
                    command = %command,
                    state = ?self.state,
                    "unsupported DAP request"
                );

                self.send_error_response(
                    request,
                    writer,
                    format!("DAP command {command:?} is not implemented"),
                )
            }
        }
    }

    fn handle_initialize<W>(&mut self, request: &Request, writer: &mut DapWriter<W>) -> Result<()>
    where
        W: Write,
    {
        if self.state != AdapterState::Created {
            return self.send_error_response(
                request,
                writer,
                "the debug adapter has already been initialized",
            );
        }

        let capabilities = json!({
            "supportsConfigurationDoneRequest": false,
            "supportsFunctionBreakpoints": false,
            "supportsConditionalBreakpoints": false,
            "supportsHitConditionalBreakpoints": false,
            "supportsEvaluateForHovers": false,
            "supportsTerminateRequest": false,
            "supportsRestartRequest": false,
            "supportsStepBack": false,
            "supportsSetVariable": false,
            "supportsReadMemoryRequest": false,
            "supportsDisassembleRequest": false
        });

        self.send_success_response(request, writer, Some(capabilities))?;

        self.state = AdapterState::Initialized;

        let initialized = Event::new(self.sequence.next(), "initialized");
        writer.write_message(&OutgoingMessage::Event(initialized))?;

        Ok(())
    }

    fn handle_launch<W>(&mut self, request: &Request, writer: &mut DapWriter<W>) -> Result<()>
    where
        W: Write,
    {
        if self.state != AdapterState::Initialized {
            return self.send_error_response(
                request,
                writer,
                format!(
                    "launch is not valid while adapter is in state {:?}",
                    self.state
                ),
            );
        }

        let Some(arguments) = request.arguments.clone() else {
            return self.send_error_response(
                request,
                writer,
                "launch request does not contain arguments",
            );
        };

        let launch_arguments = match serde_json::from_value::<LaunchArguments>(arguments) {
            Ok(arguments) => arguments,
            Err(error) => {
                return self.send_error_response(
                    request,
                    writer,
                    format!("invalid launch arguments: {error}"),
                );
            }
        };

        self.state = AdapterState::Launching;

        match self.create_session(launch_arguments) {
            Ok((session, output)) => {
                self.log_session_output(&output);
                self.session = Some(session);
                self.state = AdapterState::Connected;

                self.send_success_response(request, writer, None)
            }

            Err(error) => {
                self.state = AdapterState::Initialized;

                self.send_error_response(
                    request,
                    writer,
                    format!("failed to launch QNX debug session: {error:#}"),
                )
            }
        }
    }

    fn create_session(&self, arguments: LaunchArguments) -> Result<(GdbSession, GdbSessionOutput)> {
        let mut config = GdbSessionConfig::new(arguments.gdb, arguments.program, arguments.target);

        if let Some(working_directory) = arguments.working_directory {
            config = config.working_directory(working_directory);
        }

        for argument in arguments.gdb_arguments {
            config = config.gdb_argument(argument);
        }

        let session = GdbSession::connect(config)?;

        Ok(session)
    }

    fn log_session_output(&self, output: &GdbSessionOutput) {
        for record in &output.startup_records {
            log_mi_record("startup", record);
        }

        for event in &output.version_events {
            log_gdb_event("version", event);
        }

        for event in &output.symbol_events {
            log_gdb_event("symbols", event);
        }

        for event in &output.target_events {
            log_gdb_event("target", event);
        }
    }

    fn send_success_response<W>(
        &mut self,
        request: &Request,
        writer: &mut DapWriter<W>,
        body: Option<serde_json::Value>,
    ) -> Result<()>
    where
        W: Write,
    {
        let response = Response::success(self.sequence.next(), request, body);
        writer.write_message(&OutgoingMessage::Response(response))?;

        Ok(())
    }

    fn send_error_response<W>(
        &mut self,
        request: &Request,
        writer: &mut DapWriter<W>,
        message: impl Into<String>,
    ) -> Result<()>
    where
        W: Write,
    {
        let response = Response::error(self.sequence.next(), request, message);

        writer.write_message(&OutgoingMessage::Response(response))?;

        Ok(())
    }
}

impl Default for DebugAdapter {
    fn default() -> Self {
        Self::new()
    }
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
        self.next = self
            .next
            .checked_add(1)
            .expect("DAP sequence number overflow");

        current
    }
}

fn log_gdb_event(context: &str, event: &GdbEvent) {
    match event {
        GdbEvent::Async(record) | GdbEvent::Stream(record) => {
            log_mi_record(context, record);
        }
    }
}

fn log_mi_record(context: &str, record: &MiRecord) {
    match record {
        MiRecord::ConsoleStream(text) => {
            debug!(
                %context,
                output = %text.trim_end(),
                "GDB console output"
            );
        }

        MiRecord::TargetStream(text) => {
            debug!(
                %context,
                output = %text.trim_end(),
                "QNX target output"
            );
        }

        MiRecord::LogStream(text) => {
            debug!(
                %context,
                output = %text.trim_end(),
                "GDB diagnostic output"
            );
        }

        other => {
            debug!(%context, record = ?other, "GDB MI record");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufReader, Cursor};

    use qnx_dap::{DapReader, DapWriter};
    use serde_json::{Value, json};

    use super::{AdapterState, DebugAdapter};

    #[test]
    fn starts_in_created_state() {
        let adapter = DebugAdapter::new();

        assert_eq!(adapter.state(), AdapterState::Created);
    }

    #[test]
    fn handles_initialize_request() {
        let input = encode_messages(&[json!({
            "seq": 10,
            "type": "request",
            "command": "initialize",
            "arguments": {
                "clientID": "test-client",
                "adapterID": "qnx-gdb"
            }
        })]);

        let (adapter, messages) = run_adapter(input);

        assert_eq!(adapter.state(), AdapterState::Disconnected);
        assert_eq!(messages.len(), 2);

        assert_eq!(
            messages[0],
            json!({
                "seq": 1,
                "type": "response",
                "request_seq": 10,
                "success": true,
                "command": "initialize",
                "body": {
                    "supportsConfigurationDoneRequest": false,
                    "supportsFunctionBreakpoints": false,
                    "supportsConditionalBreakpoints": false,
                    "supportsHitConditionalBreakpoints": false,
                    "supportsEvaluateForHovers": false,
                    "supportsTerminateRequest": false,
                    "supportsRestartRequest": false,
                    "supportsStepBack": false,
                    "supportsSetVariable": false,
                    "supportsReadMemoryRequest": false,
                    "supportsDisassembleRequest": false
                }
            })
        );

        assert_eq!(
            messages[1],
            json!({
                "seq": 2,
                "type": "event",
                "event": "initialized"
            })
        );
    }

    #[test]
    fn rejects_unknown_request() {
        let input = encode_messages(&[json!({
            "seq": 7,
            "type": "request",
            "command": "launch",
            "arguments": {}
        })]);

        let (_, messages) = run_adapter(input);

        assert_eq!(
            messages,
            vec![json!({
                "seq": 1,
                "type": "response",
                "request_seq": 7,
                "success": false,
                "command": "launch",
                "message": "DAP command \"launch\" is not implemented"
            })]
        );
    }

    #[test]
    fn rejects_second_initialize_request() {
        let input = encode_messages(&[
            json!({
                "seq": 1,
                "type": "request",
                "command": "initialize"
            }),
            json!({
                "seq": 2,
                "type": "request",
                "command": "initialize"
            }),
        ]);

        let (_, messages) = run_adapter(input);

        assert_eq!(messages.len(), 3);

        assert_eq!(
            messages[2],
            json!({
                "seq": 3,
                "type": "response",
                "request_seq": 2,
                "success": false,
                "command": "initialize",
                "message": "the debug adapter has already been initialized"
            })
        );
    }

    #[test]
    fn uses_monotonically_increasing_sequence_numbers() {
        let input = encode_messages(&[
            json!({
                "seq": 1,
                "type": "request",
                "command": "initialize"
            }),
            json!({
                "seq": 2,
                "type": "request",
                "command": "launch"
            }),
            json!({
                "seq": 3,
                "type": "request",
                "command": "threads"
            }),
        ]);

        let (_, messages) = run_adapter(input);

        let sequences = messages
            .iter()
            .map(|message| {
                message["seq"]
                    .as_u64()
                    .expect("message sequence should be an integer")
            })
            .collect::<Vec<_>>();

        assert_eq!(sequences, vec![1, 2, 3, 4]);
    }

    #[test]
    fn rejects_launch_before_initialize() {
        let input = encode_messages(&[json!({
            "seq": 1,
            "type": "request",
            "command": "launch",
            "arguments": {
                "gdb": "/does/not/matter",
                "program": "/does/not/matter",
                "target": "localhost:8000"
            }
        })]);

        let (_, messages) = run_adapter(input);

        assert_eq!(
            messages,
            vec![json!({
                "seq": 1,
                "type": "response",
                "request_seq": 1,
                "success": false,
                "command": "launch",
                "message": "launch is not valid while adapter is in state Created"
            })]
        );
    }

    #[test]
    fn rejects_launch_without_arguments() {
        let input = encode_messages(&[
            json!({
                "seq": 1,
                "type": "request",
                "command": "initialize"
            }),
            json!({
                "seq": 2,
                "type": "request",
                "command": "launch"
            }),
        ]);

        let (_, messages) = run_adapter(input);

        assert_eq!(messages.len(), 3);

        assert_eq!(
            messages[2],
            json!({
                "seq": 3,
                "type": "response",
                "request_seq": 2,
                "success": false,
                "command": "launch",
                "message": "launch request does not contain arguments"
            })
        );
    }

    #[test]
    fn rejects_invalid_launch_arguments() {
        let input = encode_messages(&[
            json!({
                "seq": 1,
                "type": "request",
                "command": "initialize"
            }),
            json!({
                "seq": 2,
                "type": "request",
                "command": "launch",
                "arguments": {
                    "gdb": "/usr/bin/ntoarm-gdb"
                }
            }),
        ]);

        let (_, messages) = run_adapter(input);

        assert_eq!(messages.len(), 3);

        assert_eq!(messages[2]["type"], "response");
        assert_eq!(messages[2]["request_seq"], 2);
        assert_eq!(messages[2]["success"], false);
        assert_eq!(messages[2]["command"], "launch");

        let message = messages[2]["message"]
            .as_str()
            .expect("error response should contain a message");

        assert!(message.starts_with("invalid launch arguments:"));
    }

    fn run_adapter(input: Vec<u8>) -> (DebugAdapter, Vec<Value>) {
        let cursor = Cursor::new(input);
        let mut reader = DapReader::new(BufReader::new(cursor));
        let mut writer = DapWriter::new(Vec::new());
        let mut adapter = DebugAdapter::new();

        adapter
            .run(&mut reader, &mut writer)
            .expect("adapter should process valid input");

        let output = writer.into_inner();
        let messages = decode_messages(output);

        (adapter, messages)
    }

    fn encode_messages(messages: &[Value]) -> Vec<u8> {
        let mut writer = DapWriter::new(Vec::new());

        for message in messages {
            writer
                .write_message(message)
                .expect("test message should be encoded");
        }

        writer.into_inner()
    }

    fn decode_messages(data: Vec<u8>) -> Vec<Value> {
        let cursor = Cursor::new(data);
        let mut reader = DapReader::new(BufReader::new(cursor));
        let mut messages = Vec::new();

        while let Some(message) = reader
            .read_message::<Value>()
            .expect("adapter output should contain valid DAP messages")
        {
            messages.push(message);
        }

        messages
    }
}
