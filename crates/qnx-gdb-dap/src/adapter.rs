use std::{
    io::{BufRead, Write},
    sync::mpsc::{self, Receiver, RecvTimeoutError, Sender},
    thread,
    time::Duration,
};

use crate::{
    DapBreakpoint, DeploymentArguments, DisconnectArguments, LaunchArguments,
    SetBreakpointsArguments,
};
use anyhow::Result;
use qnx_dap::{DapReadError, DapReader, DapWriter, Event, OutgoingMessage, Request, Response};
use qnx_gdb_mi::{
    GdbDeployment, GdbEvent, GdbSession, GdbSessionConfig, GdbSessionEvent, GdbSessionEventPoll,
    GdbSessionOutput, GdbStopReason, MiRecord, SourceBreakpoint,
};
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

    /// The client has completed initial debug-session configuration.
    Configured,

    /// The inferior is currently running.
    Running,

    /// The inferior is stopped and can be inspected.
    Stopped,

    /// The debug session has been explicitly terminated.
    Terminated,

    /// The DAP input stream has been closed.
    Disconnected,
}

#[derive(Debug)]
enum DapInput {
    Request(Request),
    EndOfFile,
    Error(DapReadError),
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

    /// Processes DAP requests and asynchronous GDB events.
    ///
    /// The DAP input stream is read in a background thread. Responses and events
    /// are always written from the current thread.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// - the DAP reader thread cannot be started;
    /// - a DAP request cannot be decoded;
    /// - a response or event cannot be written;
    /// - handling a debugger request fails.
    pub fn run<R, W>(&mut self, reader: DapReader<R>, writer: &mut DapWriter<W>) -> Result<()>
    where
        R: BufRead + Send + 'static,
        W: Write,
    {
        let (sender, receiver) = mpsc::channel();

        let reader_thread = thread::Builder::new()
            .name("qnx-dap-reader".to_owned())
            .spawn(move || run_dap_reader(reader, sender))?;

        let result = self.run_event_loop(&receiver, writer);

        if reader_thread.join().is_err() {
            warn!("DAP reader thread panicked");
        }

        result
    }

    fn run_event_loop<W>(
        &mut self,
        receiver: &Receiver<DapInput>,
        writer: &mut DapWriter<W>,
    ) -> Result<()>
    where
        W: Write,
    {
        let mut input_closed = false;

        while !input_closed {
            match receiver.recv_timeout(Duration::from_millis(10)) {
                Ok(DapInput::Request(request)) => {
                    debug!(
                        request_seq = request.seq,
                        command = %request.command,
                        state = ?self.state,
                        "received DAP request"
                    );

                    self.handle_request(&request, writer)?;
                }

                Ok(DapInput::EndOfFile) => {
                    input_closed = true;
                }

                Ok(DapInput::Error(error)) => {
                    return Err(error.into());
                }

                Err(RecvTimeoutError::Timeout) => {}

                Err(RecvTimeoutError::Disconnected) => {
                    input_closed = true;
                }
            }

            self.drain_gdb_events(writer)?;
        }

        self.shutdown_session_after_eof();
        self.state = AdapterState::Disconnected;

        info!("DAP input stream closed");

        Ok(())
    }

    fn shutdown_session_after_eof(&mut self) {
        if let Some(session) = self.session.as_mut() {
            if let Err(error) = session.shutdown() {
                warn!(%error, "failed to shut down GDB session");
            }
        }

        self.session = None;
    }

    fn handle_request<W>(&mut self, request: &Request, writer: &mut DapWriter<W>) -> Result<()>
    where
        W: Write,
    {
        match request.command.as_str() {
            "initialize" => self.handle_initialize(request, writer),
            "launch" => self.handle_launch(request, writer),
            "setBreakpoints" => self.handle_set_breakpoints(request, writer),
            "configurationDone" => self.handle_configuration_done(request, writer),
            "disconnect" => self.handle_disconnect(request, writer),
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
            "supportsConfigurationDoneRequest": true,
            "supportsFunctionBreakpoints": false,
            "supportsConditionalBreakpoints": false,
            "supportsHitConditionalBreakpoints": false,
            "supportsEvaluateForHovers": false,
            "supportsTerminateRequest": false,
            "supportsRestartRequest": false,
            "supportsStepBack": false,
            "supportsSetVariable": false,
            "supportsReadMemoryRequest": false,
            "supportsDisassembleRequest": false,
            "supportTerminateDebuggee": false,
            "supportSuspendDebuggee": false
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

    fn handle_set_breakpoints<W>(
        &mut self,
        request: &Request,
        writer: &mut DapWriter<W>,
    ) -> Result<()>
    where
        W: Write,
    {
        if !matches!(
            self.state,
            AdapterState::Connected | AdapterState::Configured | AdapterState::Stopped
        ) {
            return self.send_error_response(
                request,
                writer,
                format!(
                    "setBreakpoints is not valid while adapter is in state {:?}",
                    self.state
                ),
            );
        }

        let Some(arguments) = request.arguments.clone() else {
            return self.send_error_response(
                request,
                writer,
                "setBreakpoints request does not contain arguments",
            );
        };

        let arguments = match serde_json::from_value::<SetBreakpointsArguments>(arguments) {
            Ok(arguments) => arguments,
            Err(error) => {
                return self.send_error_response(
                    request,
                    writer,
                    format!("invalid setBreakpoints arguments: {error}"),
                );
            }
        };

        if arguments.source_modified {
            debug!(
                "sourceModified was set, but source reloading is not \
                 required by the current GDB backend"
            );
        }

        let Some(source_path) = arguments.source.path else {
            return self.send_error_response(
                request,
                writer,
                "setBreakpoints source does not contain a path",
            );
        };

        for breakpoint in &arguments.breakpoints {
            if breakpoint.line == 0 {
                return self.send_error_response(
                    request,
                    writer,
                    "breakpoint line must be greater than zero",
                );
            }

            if breakpoint.condition.is_some() {
                return self.send_error_response(
                    request,
                    writer,
                    "conditional breakpoints are not supported",
                );
            }

            if breakpoint.hit_condition.is_some() {
                return self.send_error_response(
                    request,
                    writer,
                    "hit conditional breakpoints are not supported",
                );
            }

            if breakpoint.log_message.is_some() {
                return self.send_error_response(request, writer, "log points are not supported");
            }
        }

        let requested_breakpoints = arguments
            .breakpoints
            .iter()
            .map(|breakpoint| SourceBreakpoint::new(breakpoint.line))
            .collect::<Vec<_>>();

        let breakpoint_result = {
            let Some(session) = self.session.as_mut() else {
                return self.send_error_response(request, writer, "GDB session is not available");
            };

            session.set_source_breakpoints(&source_path, &requested_breakpoints)
        };

        // if arguments
        //     .breakpoints
        //     .iter()
        //     .any(|breakpoint| breakpoints.column.is_some())
        // {
        //     debug!(
        //         "breakpoint columns were provided but are ignored by \
        //         the current GDB backend"
        //     );
        // }

        match breakpoint_result {
            Ok(breakpoints) => {
                let breakpoints = breakpoints
                    .into_iter()
                    .map(DapBreakpoint::from)
                    .collect::<Vec<_>>();
                let body = serde_json::json!({
                    "breakpoints": breakpoints
                });

                self.send_success_response(request, writer, Some(body))
            }

            Err(error) => self.send_error_response(
                request,
                writer,
                format!("failed to set source breakpoints: {error}"),
            ),
        }

        // let Some(session) = self.session.as_mut() else {
        //     return self.send_error_response(request, writer, "GDB session is not available");
        // };

        // match session.set_source_breakpoints(&source_path, &requested_breakpoints) {
        //     Ok(breakpoints) => {
        //         let breakpoints = breakpoints
        //             .into_iter()
        //             .map(DapBreakpoint::from)
        //             .collect::<Vec<_>>();

        //         let body = serde_json::json!({
        //             "breakpoints": breakpoints
        //         });

        //         self.send_success_response(request, writer, Some(body))
        //     }

        //     Err(error) => self.send_error_response(
        //         request,
        //         writer,
        //         format!("failed to set source breakpoints: {error}"),
        //     ),
        // }
    }

    fn handle_configuration_done<W>(
        &mut self,
        request: &Request,
        writer: &mut DapWriter<W>,
    ) -> Result<()>
    where
        W: Write,
    {
        if self.state != AdapterState::Connected {
            return self.send_error_response(
                request,
                writer,
                format!(
                    "configurationDone is not valid while adapter is in state {:?}",
                    self.state
                ),
            );
        }

        if let Some(arguments) = &request.arguments {
            if !arguments.as_object().is_some_and(serde_json::Map::is_empty) {
                debug!(
                    arguments = ?arguments,
                    "ignoring configurationDone arguments"
                );
            }
        }

        self.state = AdapterState::Configured;

        let run_result = {
            let Some(session) = self.session.as_mut() else {
                self.state = AdapterState::Connected;

                return self.send_error_response(request, writer, "GDB session is not available");
            };

            session.run()
        };

        match run_result {
            Ok(started) => {
                self.state = AdapterState::Running;

                self.send_success_response(request, writer, None)?;

                for event in started.initial_events {
                    self.send_gdb_session_event(writer, event)?;
                }

                Ok(())
            }

            Err(error) => {
                self.state = AdapterState::Connected;

                self.send_error_response(
                    request,
                    writer,
                    format!("failed to start QNX inferior: {error}"),
                )
            }
        }
    }

    fn drain_gdb_events<W>(&mut self, writer: &mut DapWriter<W>) -> Result<()>
    where
        W: Write,
    {
        if !matches!(self.state, AdapterState::Running | AdapterState::Stopped) {
            return Ok(());
        }

        loop {
            let poll_result = {
                let Some(session) = self.session.as_mut() else {
                    return Ok(());
                };

                session.try_next_execution_event()
            };

            match poll_result {
                Ok(GdbSessionEventPoll::Pending) => {
                    return Ok(());
                }

                Ok(GdbSessionEventPoll::EndOfFile) => {
                    self.state = AdapterState::Terminated;

                    let event = Event::new(self.sequence.next(), "terminated");

                    writer.write_message(&OutgoingMessage::Event(event))?;

                    return Ok(());
                }

                Ok(GdbSessionEventPoll::Event(event)) => {
                    self.send_gdb_session_event(writer, event)?;
                }

                Err(error) => {
                    self.send_output_event(
                        writer,
                        "stderr",
                        format!("GDB event error: {error}\n"),
                    )?;

                    self.state = AdapterState::Terminated;

                    let event = Event::new(self.sequence.next(), "terminated");

                    writer.write_message(&OutgoingMessage::Event(event))?;

                    return Ok(());
                }
            }
        }
    }

    fn send_gdb_session_event<W>(
        &mut self,
        writer: &mut DapWriter<W>,
        event: GdbSessionEvent,
    ) -> Result<()>
    where
        W: Write,
    {
        match event {
            GdbSessionEvent::TargetOutput(output) => {
                self.send_output_event(writer, "stdout", output)
            }

            GdbSessionEvent::ConsoleOutput(output) => {
                self.send_output_event(writer, "console", output)
            }

            GdbSessionEvent::DiagnosticOutput(output) => {
                self.send_output_event(writer, "stderr", output)
            }

            GdbSessionEvent::Stopped {
                reason,
                thread_id,
                frame: _,
            } => self.send_stopped_event(writer, reason, thread_id),

            GdbSessionEvent::LateResult { class, message, .. } => {
                let output = match message {
                    Some(message) => {
                        format!("GDB execution result {class}: {message}\n")
                    }

                    None => {
                        format!("GDB execution result: {class}\n")
                    }
                };

                self.send_output_event(writer, "stderr", output)
            }

            GdbSessionEvent::AsyncRecord { class } => {
                debug!(
                    %class,
                    "unmapped GDB asynchronous record"
                );

                Ok(())
            }

            GdbSessionEvent::EndOfFile => {
                self.state = AdapterState::Terminated;

                let event = Event::new(self.sequence.next(), "terminated");

                writer.write_message(&OutgoingMessage::Event(event))?;

                Ok(())
            }
        }
    }

    fn send_output_event<W>(
        &mut self,
        writer: &mut DapWriter<W>,
        category: &str,
        output: String,
    ) -> Result<()>
    where
        W: Write,
    {
        let event = Event::with_body(
            self.sequence.next(),
            "output",
            serde_json::json!({
                "category": category,
                "output": output
            }),
        );

        writer.write_message(&OutgoingMessage::Event(event))?;

        Ok(())
    }

    fn send_stopped_event<W>(
        &mut self,
        writer: &mut DapWriter<W>,
        reason: GdbStopReason,
        thread_id: Option<u64>,
    ) -> Result<()>
    where
        W: Write,
    {
        let thread_id = thread_id.unwrap_or(1);

        let (reason_text, description, hit_ids) = match reason {
            GdbStopReason::Breakpoint { breakpoint_number } => (
                "breakpoint",
                None,
                breakpoint_number.map(|number| vec![number]),
            ),

            GdbStopReason::Step => ("step", None, None),

            GdbStopReason::Signal { name, meaning } => {
                let description = match (name, meaning) {
                    (Some(name), Some(meaning)) => Some(format!("{name}: {meaning}")),

                    (Some(name), None) => Some(name),
                    (None, Some(meaning)) => Some(meaning),
                    (None, None) => None,
                };

                ("exception", description, None)
            }

            GdbStopReason::Unknown { reason } => ("pause", reason, None),

            GdbStopReason::Exited { exit_code } => {
                return self.send_exit_events(writer, exit_code.unwrap_or(0));
            }

            GdbStopReason::ExitedSignalled { signal_name } => {
                if let Some(signal_name) = signal_name {
                    self.send_output_event(
                        writer,
                        "stderr",
                        format!(
                            "Inferior exited because of signal \
                             {signal_name}\n"
                        ),
                    )?;
                }

                return self.send_exit_events(writer, 1);
            }
        };

        self.state = AdapterState::Stopped;

        let mut body = serde_json::json!({
            "reason": reason_text,
            "threadId": thread_id,
            "allThreadsStopped": true
        });

        if let Some(description) = description {
            body["description"] = serde_json::Value::String(description);
        }

        if let Some(hit_ids) = hit_ids {
            body["hitBreakpointIds"] = serde_json::json!(hit_ids);
        }

        let event = Event::with_body(self.sequence.next(), "stopped", body);

        writer.write_message(&OutgoingMessage::Event(event))?;

        Ok(())
    }

    fn send_exit_events<W>(&mut self, writer: &mut DapWriter<W>, exit_code: i32) -> Result<()>
    where
        W: Write,
    {
        self.state = AdapterState::Terminated;

        let exited = Event::with_body(
            self.sequence.next(),
            "exited",
            serde_json::json!({
                "exitCode": exit_code
            }),
        );

        writer.write_message(&OutgoingMessage::Event(exited))?;

        let terminated = Event::new(self.sequence.next(), "terminated");

        writer.write_message(&OutgoingMessage::Event(terminated))?;

        Ok(())
    }

    fn handle_disconnect<W>(&mut self, request: &Request, writer: &mut DapWriter<W>) -> Result<()>
    where
        W: Write,
    {
        if matches!(
            self.state,
            AdapterState::Created
                | AdapterState::Launching
                | AdapterState::Terminated
                | AdapterState::Disconnected
        ) {
            return self.send_error_response(
                request,
                writer,
                format!(
                    "disconnect is not valid while adapter is in state {:?}",
                    self.state
                ),
            );
        }

        let arguments = match request.arguments.clone() {
            Some(arguments) => match serde_json::from_value::<DisconnectArguments>(arguments) {
                Ok(arguments) => arguments,
                Err(error) => {
                    return self.send_error_response(
                        request,
                        writer,
                        format!("invalid disconnect arguments: {error}"),
                    );
                }
            },
            None => DisconnectArguments::default(),
        };

        if arguments.suspend_debuggee {
            return self.send_error_response(request, writer, "suspendDebuggee is not supported");
        }

        if arguments.terminate_debuggee {
            debug!(
                "terminateDebuggee was requested, but inferior termination \
                 is not implemented yet"
            );
        }

        if let Some(session) = self.session.as_mut() {
            if let Err(error) = session.shutdown() {
                return self.send_error_response(
                    request,
                    writer,
                    format!("failed to shut down GDB session: {error}"),
                );
            }
        }

        self.session = None;
        self.state = AdapterState::Terminated;

        self.send_success_response(request, writer, None)?;

        let event = Event::new(self.sequence.next(), "terminated");
        writer.write_message(&OutgoingMessage::Event(event))?;

        Ok(())
    }

    fn create_session(&self, arguments: LaunchArguments) -> Result<(GdbSession, GdbSessionOutput)> {
        let deployment = match arguments.deployment {
            DeploymentArguments::Upload { remote_program } => GdbDeployment::upload(remote_program),

            DeploymentArguments::Existing { remote_program } => {
                GdbDeployment::Existing { remote_program }
            }
        };

        let mut config = GdbSessionConfig::new(
            arguments.gdb,
            arguments.program,
            arguments.target,
            deployment,
        );

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

        for event in &output.deployment_events {
            log_gdb_event("deployment", event);
        }

        debug!(
            local_program = %output.deployment.local_program.display(),
            remote_program = %output.deployment.remote_program,
            uploaded = output.deployment.uploaded,
            "QNX executable prepared"
        );
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

fn run_dap_reader<R>(mut reader: DapReader<R>, sender: Sender<DapInput>)
where
    R: BufRead,
{
    loop {
        match reader.read_message::<Request>() {
            Ok(Some(request)) => {
                if sender.send(DapInput::Request(request)).is_err() {
                    return;
                }
            }

            Ok(None) => {
                let _ = sender.send(DapInput::EndOfFile);
                return;
            }

            Err(error) => {
                let _ = sender.send(DapInput::Error(error));
                return;
            }
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
                    "supportsConfigurationDoneRequest": true,
                    "supportsFunctionBreakpoints": false,
                    "supportsConditionalBreakpoints": false,
                    "supportsHitConditionalBreakpoints": false,
                    "supportsEvaluateForHovers": false,
                    "supportsTerminateRequest": false,
                    "supportsRestartRequest": false,
                    "supportsStepBack": false,
                    "supportsSetVariable": false,
                    "supportsReadMemoryRequest": false,
                    "supportsDisassembleRequest": false,
                    "supportTerminateDebuggee": false,
                    "supportSuspendDebuggee": false
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

    // #[test]
    // fn rejects_unknown_request() {
    //     let input = encode_messages(&[json!({
    //         "seq": 7,
    //         "type": "request",
    //         "command": "launch",
    //         "arguments": {}
    //     })]);

    //     let (_, messages) = run_adapter(input);

    //     assert_eq!(
    //         messages,
    //         vec![json!({
    //             "seq": 1,
    //             "type": "response",
    //             "request_seq": 7,
    //             "success": false,
    //             "command": "launch",
    //             "message": "DAP command \"launch\" is not implemented"
    //         })]
    //     );
    // }

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

    #[test]
    fn rejects_set_breakpoints_before_launch() {
        let input = encode_messages(&[
            json!({
                "seq": 1,
                "type": "request",
                "command": "initialize"
            }),
            json!({
                "seq": 2,
                "type": "request",
                "command": "setBreakpoints",
                "arguments": {
                    "source": {
                        "path": "/project/main.c"
                    },
                    "breakpoints": [
                        {
                            "line": 7
                        }
                    ]
                }
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
                "command": "setBreakpoints",
                "message": "setBreakpoints is not valid while adapter is in state Initialized"
            })
        );
    }

    #[test]
    fn rejects_set_breakpoints_without_arguments() {
        let mut adapter = DebugAdapter::new();
        adapter.state = AdapterState::Connected;

        let input = encode_messages(&[json!({
            "seq": 1,
            "type": "request",
            "command": "setBreakpoints"
        })]);

        let cursor = Cursor::new(input);
        let reader = DapReader::new(BufReader::new(cursor));
        let mut writer = DapWriter::new(Vec::new());

        adapter
            .run(reader, &mut writer)
            .expect("adapter should process request");

        let messages = decode_messages(writer.into_inner());

        assert_eq!(
            messages[0],
            json!({
                "seq": 1,
                "type": "response",
                "request_seq": 1,
                "success": false,
                "command": "setBreakpoints",
                "message": "setBreakpoints request does not contain arguments"
            })
        );
    }

    #[test]
    fn rejects_set_breakpoints_without_source_path() {
        let mut adapter = DebugAdapter::new();
        adapter.state = AdapterState::Connected;

        let input = encode_messages(&[json!({
            "seq": 1,
            "type": "request",
            "command": "setBreakpoints",
            "arguments": {
                "source": {
                    "name": "main.c"
                },
                "breakpoints": [
                    {
                        "line": 7
                    }
                ]
            }
        })]);

        let cursor = Cursor::new(input);
        let reader = DapReader::new(BufReader::new(cursor));
        let mut writer = DapWriter::new(Vec::new());

        adapter
            .run(reader, &mut writer)
            .expect("adapter should process request");

        let messages = decode_messages(writer.into_inner());

        assert_eq!(
            messages[0]["message"],
            "setBreakpoints source does not contain a path"
        );
    }

    #[test]
    fn rejects_zero_breakpoint_line() {
        let mut adapter = DebugAdapter::new();
        adapter.state = AdapterState::Connected;

        let input = encode_messages(&[json!({
            "seq": 1,
            "type": "request",
            "command": "setBreakpoints",
            "arguments": {
                "source": {
                    "path": "/project/main.c"
                },
                "breakpoints": [
                    {
                        "line": 0
                    }
                ]
            }
        })]);

        let cursor = Cursor::new(input);
        let reader = DapReader::new(BufReader::new(cursor));
        let mut writer = DapWriter::new(Vec::new());

        adapter
            .run(reader, &mut writer)
            .expect("adapter should process request");

        let messages = decode_messages(writer.into_inner());

        assert_eq!(
            messages[0]["message"],
            "breakpoint line must be greater than zero"
        );
    }

    #[test]
    fn rejects_configuration_done_before_launch() {
        let input = encode_messages(&[
            json!({
                "seq": 1,
                "type": "request",
                "command": "initialize"
            }),
            json!({
                "seq": 2,
                "type": "request",
                "command": "configurationDone"
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
                "command": "configurationDone",
                "message": "configurationDone is not valid while adapter is in state Initialized"
            })
        );
    }

    #[test]
    fn rejects_repeated_configuration_done() {
        let mut adapter = DebugAdapter::new();
        adapter.state = AdapterState::Configured;

        let input = encode_messages(&[json!({
            "seq": 1,
            "type": "request",
            "command": "configurationDone"
        })]);

        let (_, messages) = run_adapter_instance(adapter, input);

        assert_eq!(
            messages[0],
            json!({
                "seq": 1,
                "type": "response",
                "request_seq": 1,
                "success": false,
                "command": "configurationDone",
                "message": "configurationDone is not valid while adapter is in state Configured"
            })
        );
    }

    #[test]
    fn disconnects_after_initialize() {
        let input = encode_messages(&[
            json!({
                "seq": 1,
                "type": "request",
                "command": "initialize"
            }),
            json!({
                "seq": 2,
                "type": "request",
                "command": "disconnect",
                "arguments": {}
            }),
        ]);

        let (_, messages) = run_adapter(input);

        assert_eq!(messages.len(), 4);

        assert_eq!(
            messages[2],
            json!({
                "seq": 3,
                "type": "response",
                "request_seq": 2,
                "success": true,
                "command": "disconnect"
            })
        );

        assert_eq!(
            messages[3],
            json!({
                "seq": 4,
                "type": "event",
                "event": "terminated"
            })
        );
    }

    #[test]
    fn accepts_disconnect_without_arguments() {
        let input = encode_messages(&[
            json!({
                "seq": 1,
                "type": "request",
                "command": "initialize"
            }),
            json!({
                "seq": 2,
                "type": "request",
                "command": "disconnect"
            }),
        ]);

        let (_, messages) = run_adapter(input);

        assert_eq!(messages[2]["success"], true);
        assert_eq!(messages[2]["command"], "disconnect");
        assert_eq!(messages[3]["event"], "terminated");
    }

    #[test]
    fn rejects_suspend_debuggee() {
        let input = encode_messages(&[
            json!({
                "seq": 1,
                "type": "request",
                "command": "initialize"
            }),
            json!({
                "seq": 2,
                "type": "request",
                "command": "disconnect",
                "arguments": {
                    "suspendDebuggee": true
                }
            }),
        ]);

        let (_, messages) = run_adapter(input);

        assert_eq!(
            messages[2],
            json!({
                "seq": 3,
                "type": "response",
                "request_seq": 2,
                "success": false,
                "command": "disconnect",
                "message": "suspendDebuggee is not supported"
            })
        );
    }

    #[test]
    fn rejects_disconnect_before_initialize() {
        let input = encode_messages(&[json!({
            "seq": 1,
            "type": "request",
            "command": "disconnect"
        })]);

        let (_, messages) = run_adapter(input);

        assert_eq!(
            messages,
            vec![json!({
                "seq": 1,
                "type": "response",
                "request_seq": 1,
                "success": false,
                "command": "disconnect",
                "message": "disconnect is not valid while adapter is in state Created"
            })]
        );
    }

    #[test]
    fn accepts_repeated_disconnect() {
        let input = encode_messages(&[
            json!({
                "seq": 1,
                "type": "request",
                "command": "initialize"
            }),
            json!({
                "seq": 2,
                "type": "request",
                "command": "disconnect"
            }),
            json!({
                "seq": 3,
                "type": "request",
                "command": "disconnect"
            }),
        ]);

        let (_, messages) = run_adapter(input);

        assert_eq!(messages.len(), 5);

        assert_eq!(
            messages[4],
            json!({
                "seq": 5,
                "type": "response",
                "request_seq": 3,
                "success": false,
                "command": "disconnect",
                "message": "disconnect is not valid while adapter is in state Terminated"
            })
        );
    }

    fn run_adapter(input: Vec<u8>) -> (DebugAdapter, Vec<Value>) {
        run_adapter_instance(DebugAdapter::new(), input)
    }

    fn run_adapter_instance(
        mut adapter: DebugAdapter,
        input: Vec<u8>,
    ) -> (DebugAdapter, Vec<Value>) {
        let cursor = Cursor::new(input);
        let reader = DapReader::new(BufReader::new(cursor));
        let mut writer = DapWriter::new(Vec::new());

        adapter
            .run(reader, &mut writer)
            .expect("adapter should process valid input");

        let messages = decode_messages(writer.into_inner());

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
