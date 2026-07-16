use std::{
    io::{BufRead, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command, ExitStatus, Stdio},
    sync::mpsc::{self, Receiver, RecvError, Sender, TryRecvError},
    thread::{self, JoinHandle},
};

use thiserror::Error;
use tracing::{debug, trace, warn};

use crate::{
    MiCommand, MiParseError, MiRecord, MiResultRecord, MiTokenGenerator, commands, find_result,
    parse_record,
};

/// Configuration used to start a GDB/MI process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GdbProcessConfig {
    /// Path to the QNX GDB executable.
    pub executable: PathBuf,

    /// Additional command-line arguments passed to GDB.
    pub arguments: Vec<String>,

    /// Optional working directory for the GDB process.
    pub working_directory: Option<PathBuf>,
}

impl GdbProcessConfig {
    /// Creates a configuration for the specified GDB executable.
    #[must_use]
    pub fn new(executable: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
            arguments: Vec::new(),
            working_directory: None,
        }
    }

    /// Appends a command-line argument.
    #[must_use]
    pub fn argument(mut self, argument: impl Into<String>) -> Self {
        self.arguments.push(argument.into());
        self
    }

    /// Sets the GDB process working directory.
    #[must_use]
    pub fn working_directory(mut self, working_directory: impl Into<PathBuf>) -> Self {
        self.working_directory = Some(working_directory.into());
        self
    }
}

/// A record observed while waiting for a command result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GdbEvent {
    /// An asynchronous MI record.
    Async(MiRecord),

    /// Console, target or log stream output.
    Stream(MiRecord),
}

/// A record received after an execution command was initially accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GdbExecutionEvent {
    /// GDB reported asynchronous execution state.
    Async(MiRecord),

    /// Console, target or diagnostic output.
    Stream(MiRecord),

    /// A later result record, including a possible late `^error`.
    Result(MiResultRecord),

    /// GDB prompt.
    Prompt,
}

/// Result of polling for a post-execution GDB event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GdbExecutionPoll {
    /// One execution event is available.
    Event(GdbExecutionEvent),

    /// No record is currently available.
    Pending,

    /// GDB closed its output.
    EndOfFile,
}

/// Result of one GDB/MI command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GdbCommandResult {
    /// Final result record associated with the command token.
    pub result: MiResultRecord,

    /// Records received before the final command result.
    pub events: Vec<GdbEvent>,
}

impl GdbCommandResult {
    /// Returns whether the command completed with the specified result class.
    #[must_use]
    pub fn is_class(&self, class: &str) -> bool {
        self.result.class == class
    }

    /// Returns the GDB error message when the result class is `error`.
    #[must_use]
    pub fn error_message(&self) -> Option<&str> {
        if self.result.class != "error" {
            return None;
        }

        find_result(&self.result.results, "msg")?.as_const()
    }
}

#[derive(Debug)]
enum GdbReaderMessage {
    Record(MiRecord),
    Error(GdbReaderError),
    EndOfFile,
}

/// Error produced by the GDB stdout reader thread
#[derive(Debug, Error)]
pub enum GdbReaderError {
    #[error("I/O error while reading GDB output")]
    Io(#[from] std::io::Error),

    #[error("invalid GDB/MI output")]
    Parse(#[from] MiParseError),
}

/// Initial result of an asynchronous execution command.
///
/// A result class of `running` means GDB accepted the command. Execution
/// events such as `*stopped` must then be read separately.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GdbExecutionStart {
    /// Token assigned to the execution command.
    pub token: u64,

    /// Initial result record, normally `^running` or `^error`.
    pub result: MiResultRecord,

    /// Records received before the initial result.
    pub events: Vec<GdbEvent>,
}

impl GdbExecutionStart {
    /// Returns whether GDB accepted the command for execution.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.result.class == "running"
    }

    /// Returns an error message from an initial `^error` result.
    #[must_use]
    pub fn error_message(&self) -> Option<&str> {
        if self.result.class != "error" {
            return None;
        }

        find_result(&self.result.results, "msg")?.as_const()
    }
}

fn run_reader_thread(stdout: ChildStdout, sender: &Sender<GdbReaderMessage>) {
    let mut reader = BufReader::new(stdout);

    loop {
        let mut line = String::new();

        match reader.read_line(&mut line) {
            Ok(0) => {
                let _ = sender.send(GdbReaderMessage::EndOfFile);
                return;
            }

            Ok(_) => {
                let line = line.trim_end_matches(['\r', '\n']);

                trace!(line = %line, "received GDB/MI line");

                match parse_record(line) {
                    Ok(record) => {
                        if sender.send(GdbReaderMessage::Record(record)).is_err() {
                            return;
                        }
                    }

                    Err(error) => {
                        let _ = sender.send(GdbReaderMessage::Error(GdbReaderError::Parse(error)));
                        return;
                    }
                }
            }

            Err(error) => {
                let _ = sender.send(GdbReaderMessage::Error(GdbReaderError::Io(error)));
                return;
            }
        }
    }
}

/// A long-running GDB process communicating through GDB/MI.
#[derive(Debug)]
pub struct GdbProcess {
    child: Child,
    stdin: Option<BufWriter<ChildStdin>>,
    // stdout: BufReader<ChildStdout>,
    records: Receiver<GdbReaderMessage>,
    reader_thread: Option<JoinHandle<()>>,
    tokens: MiTokenGenerator,
    synchronized: bool,
    terminated: bool,
}

impl GdbProcess {
    /// Starts GDB in MI2 mode.
    ///
    /// # Errors
    ///
    /// Returns an error if GDB cannot be started or its standard streams
    /// cannot be captured.
    pub fn spawn(config: &GdbProcessConfig) -> Result<Self, GdbProcessError> {
        let mut command = Command::new(&config.executable);

        command
            .arg("--interpreter=mi2")
            .args(&config.arguments)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());

        if let Some(working_directory) = &config.working_directory {
            command.current_dir(working_directory);
        }

        debug!(
            executable = %config.executable.display(),
            arguments = ?config.arguments,
            "starting GDB process"
        );

        let mut child = command.spawn().map_err(|source| GdbProcessError::Spawn {
            executable: config.executable.clone(),
            source,
        })?;

        let stdin = child.stdin.take().ok_or(GdbProcessError::MissingStdin)?;

        let stdout = child.stdout.take().ok_or(GdbProcessError::MissingStdout)?;

        let (record_sender, record_receiver) = mpsc::channel();

        let reader_thread = thread::Builder::new()
            .name("qnx-gdb-mi-reader".to_owned())
            .spawn(move || {
                run_reader_thread(stdout, &record_sender);
            })
            .map_err(GdbProcessError::ReaderThreadSpawn)?;

        Ok(Self {
            child,
            stdin: Some(BufWriter::new(stdin)),
            // stdout: BufReader::new(stdout),
            records: record_receiver,
            reader_thread: Some(reader_thread),
            tokens: MiTokenGenerator::new(),
            synchronized: false,
            terminated: false,
        })
    }

    /// Returns the operating-system process identifier of GDB.
    #[must_use]
    pub fn id(&self) -> u32 {
        self.child.id()
    }

    /// Reads the initial GDB output until the first prompt.
    ///
    /// GDB writes its version banner before accepting the first command. This
    /// method separates that startup output from records produced by subsequent
    /// commands.
    ///
    /// # Errors
    ///
    /// Returns an error if GDB output cannot be read, contains malformed MI, or
    /// closes before the initial prompt is received.
    pub fn synchronize(&mut self) -> Result<Vec<MiRecord>, GdbProcessError> {
        if self.synchronized {
            return Err(GdbProcessError::AlreadySynchronized);
        }

        let mut startup_records = Vec::new();

        loop {
            let Some(record) = self.next_record()? else {
                let status = self.child.try_wait()?;

                return Err(GdbProcessError::UnexpectedEndDuringSynchronization { status });
            };

            match record {
                MiRecord::Prompt => {
                    self.synchronized = true;

                    debug!(
                        startup_record_count = startup_records.len(),
                        "GDB startup synchronization completed"
                    );

                    return Ok(startup_records);
                }

                MiRecord::Empty => {}

                MiRecord::ConsoleStream(_)
                | MiRecord::TargetStream(_)
                | MiRecord::LogStream(_)
                | MiRecord::ExecAsync(_)
                | MiRecord::StatusAsync(_)
                | MiRecord::NotifyAsync(_) => {
                    startup_records.push(record);
                }

                MiRecord::Result(result) => {
                    return Err(GdbProcessError::UnexpectedResultDuringSynchronization {
                        token: result.token,
                        class: result.class,
                    });
                }
            }
        }
    }

    /// Sends a command and waits for its result record.
    ///
    /// Stream and asynchronous records encountered before the command result
    /// are returned in [`GdbCommandResult::events`].
    ///
    /// # Errors
    ///
    /// Returns an error if writing fails, GDB terminates, a line cannot be
    /// decoded, or malformed GDB/MI output is received.
    pub fn execute<F>(&mut self, build_command: F) -> Result<GdbCommandResult, GdbProcessError>
    where
        F: FnOnce(u64) -> MiCommand,
    {
        if !self.synchronized {
            return Err(GdbProcessError::NotSynchronized);
        }

        let token = self.tokens.next_token();
        let command = build_command(token);

        if command.token() != Some(token) {
            return Err(GdbProcessError::UnexpectedCommandToken {
                expected: token,
                actual: command.token(),
            });
        }

        self.send_command(&command)?;
        self.wait_for_result(token)
    }

    /// Sends an execution command and waits only for its initial result.
    ///
    /// Normally GDB returns `^running`, after which execution continues
    /// asynchronously. Later records such as `*stopped` must be consumed through
    /// [`GdbProcess::next_record`] or [`GdbProcess::try_next_record`].
    ///
    /// # Errors
    ///
    /// Returns an error if the command cannot be written, GDB terminates, or an
    /// unexpected result token is received.
    pub fn start_execution<F>(
        &mut self,
        build_command: F,
    ) -> Result<GdbExecutionStart, GdbProcessError>
    where
        F: FnOnce(u64) -> MiCommand,
    {
        let token = self.tokens.next_token();
        let command = build_command(token);

        if command.token() != Some(token) {
            return Err(GdbProcessError::UnexpectedCommandToken {
                expected: token,
                actual: command.token(),
            });
        }

        self.send_command(&command)?;

        let mut events = Vec::new();

        loop {
            let Some(record) = self.next_record()? else {
                let status = self.child.try_wait()?;

                return Err(GdbProcessError::UnexpectedEndOfOutput {
                    status,
                    expected_token: token,
                });
            };

            match record {
                MiRecord::Result(result) if result.token == Some(token) => {
                    return Ok(GdbExecutionStart {
                        token,
                        result,
                        events,
                    });
                }

                MiRecord::Result(result) => {
                    return Err(GdbProcessError::UnexpectedResultToken {
                        expected: token,
                        actual: result.token,
                        class: result.class,
                    });
                }

                MiRecord::ExecAsync(_) | MiRecord::StatusAsync(_) | MiRecord::NotifyAsync(_) => {
                    events.push(GdbEvent::Async(record));
                }

                MiRecord::ConsoleStream(_) | MiRecord::TargetStream(_) | MiRecord::LogStream(_) => {
                    events.push(GdbEvent::Stream(record));
                }

                MiRecord::Prompt | MiRecord::Empty => {}
            }
        }
    }

    /// Sends an already constructed command.
    ///
    /// This method does not wait for a result record.
    ///
    /// # Errors
    ///
    /// Returns an error when GDB stdin is closed or writing fails.
    pub fn send_command(&mut self, command: &MiCommand) -> Result<(), GdbProcessError> {
        if !self.synchronized {
            return Err(GdbProcessError::NotSynchronized);
        }

        let encoded = command.encode()?;

        trace!(command = %encoded, "sending GDB/MI command");

        let stdin = self.stdin.as_mut().ok_or(GdbProcessError::StdinClosed)?;

        writeln!(stdin, "{encoded}")?;
        stdin.flush()?;

        Ok(())
    }

    /// Waits for the next GDB/MI record.
    ///
    /// Returns `Ok(None)` when GDB closes its output.
    ///
    /// # Errors
    ///
    /// Returns an error if the reader thread reports an I/O or parsing failure,
    /// or if its communication channel closes unexpectedly.
    pub fn next_record(&mut self) -> Result<Option<MiRecord>, GdbProcessError> {
        match self.records.recv() {
            Ok(GdbReaderMessage::Record(record)) => Ok(Some(record)),
            Ok(GdbReaderMessage::EndOfFile) => Ok(None),
            Ok(GdbReaderMessage::Error(error)) => Err(GdbProcessError::Reader(error)),
            Err(error) => Err(GdbProcessError::ReaderChannelClosed(error)),
        }
    }

    /// Attempts to receive a GDB/MI record without blocking.
    ///
    /// # Errors
    ///
    /// Returns an error if the reader thread failed or its channel has closed.
    pub fn try_next_record(&mut self) -> Result<GdbRecordPoll, GdbProcessError> {
        match self.records.try_recv() {
            Ok(GdbReaderMessage::Record(record)) => Ok(GdbRecordPoll::Record(record)),
            Ok(GdbReaderMessage::EndOfFile) => Ok(GdbRecordPoll::EndOfFile),
            Ok(GdbReaderMessage::Error(error)) => Err(GdbProcessError::Reader(error)),
            Err(TryRecvError::Empty) => Ok(GdbRecordPoll::Pending),
            Err(TryRecvError::Disconnected) => Err(GdbProcessError::ReaderChannelDisconnected),
        }
    }

    /// Waits for one record produced after an execution command.
    ///
    /// # Errors
    ///
    /// Returns an error if GDB output cannot be read.
    pub fn next_execution_event(&mut self) -> Result<Option<GdbExecutionEvent>, GdbProcessError> {
        loop {
            let Some(record) = self.next_record()? else {
                return Ok(None);
            };

            let event = match record {
                MiRecord::Result(result) => GdbExecutionEvent::Result(result),

                MiRecord::ExecAsync(_) | MiRecord::StatusAsync(_) | MiRecord::NotifyAsync(_) => {
                    GdbExecutionEvent::Async(record)
                }

                MiRecord::ConsoleStream(_) | MiRecord::TargetStream(_) | MiRecord::LogStream(_) => {
                    GdbExecutionEvent::Stream(record)
                }

                MiRecord::Prompt => GdbExecutionEvent::Prompt,
                MiRecord::Empty => continue,
            };

            return Ok(Some(event));
        }
    }

    /// Attempts to receive one execution event without blocking.
    ///
    /// # Errors
    ///
    /// Returns an error if the GDB reader thread failed or its channel closed
    /// unexpectedly.
    pub fn try_next_execution_event(&mut self) -> Result<GdbExecutionPoll, GdbProcessError> {
        loop {
            let record = match self.try_next_record()? {
                GdbRecordPoll::Record(record) => record,

                GdbRecordPoll::Pending => {
                    return Ok(GdbExecutionPoll::Pending);
                }

                GdbRecordPoll::EndOfFile => {
                    return Ok(GdbExecutionPoll::EndOfFile);
                }
            };

            let event = match record {
                MiRecord::Result(result) => GdbExecutionEvent::Result(result),

                MiRecord::ExecAsync(_) | MiRecord::StatusAsync(_) | MiRecord::NotifyAsync(_) => {
                    GdbExecutionEvent::Async(record)
                }

                MiRecord::ConsoleStream(_) | MiRecord::TargetStream(_) | MiRecord::LogStream(_) => {
                    GdbExecutionEvent::Stream(record)
                }

                MiRecord::Prompt => GdbExecutionEvent::Prompt,

                MiRecord::Empty => continue,
            };

            return Ok(GdbExecutionPoll::Event(event));
        }
    }

    /// Requests a clean GDB shutdown and waits for process termination.
    ///
    /// # Errors
    ///
    /// Returns an error if `-gdb-exit` cannot be sent or the child process
    /// cannot be waited for.
    pub fn shutdown(&mut self) -> Result<ExitStatus, GdbProcessError> {
        if self.terminated {
            return self.child.wait().map_err(GdbProcessError::Wait);
        }

        let result = self.execute(commands::gdb_exit)?;

        if result.result.class != "exit" {
            warn!(
                result_class = %result.result.class,
                "GDB returned an unexpected result for gdb-exit"
            );
        }

        self.stdin.take();
        let status = self.child.wait().map_err(GdbProcessError::Wait)?;
        self.join_reader_thread();
        self.terminated = true;

        Ok(status)
    }

    fn join_reader_thread(&mut self) {
        let Some(handle) = self.reader_thread.take() else {
            return;
        };

        if handle.join().is_err() {
            warn!("GDB reader thread panicked");
        }
    }

    fn wait_for_result(
        &mut self,
        expected_token: u64,
    ) -> Result<GdbCommandResult, GdbProcessError> {
        let mut events = Vec::new();

        loop {
            let Some(record) = self.next_record()? else {
                let status = self.child.try_wait()?;

                return Err(GdbProcessError::UnexpectedEndOfOutput {
                    status,
                    expected_token,
                });
            };

            match record {
                MiRecord::Result(result) if result.token == Some(expected_token) => {
                    return Ok(GdbCommandResult { result, events });
                }

                MiRecord::Result(result) => {
                    return Err(GdbProcessError::UnexpectedResultToken {
                        expected: expected_token,
                        actual: result.token,
                        class: result.class,
                    });
                }

                MiRecord::ExecAsync(_) | MiRecord::StatusAsync(_) | MiRecord::NotifyAsync(_) => {
                    events.push(GdbEvent::Async(record));
                }

                MiRecord::ConsoleStream(_) | MiRecord::TargetStream(_) | MiRecord::LogStream(_) => {
                    events.push(GdbEvent::Stream(record));
                }

                MiRecord::Prompt | MiRecord::Empty => {}
            }
        }
    }
}

impl Drop for GdbProcess {
    fn drop(&mut self) {
        if self.terminated {
            return;
        }

        self.stdin.take();

        match self.child.try_wait() {
            Ok(Some(_)) => {
                self.join_reader_thread();
                self.terminated = true;
            }
            Ok(None) => {
                warn!(
                    pid = self.child.id(),
                    "GDB process is still running during drop; terminating it"
                );

                if let Err(error) = self.child.kill() {
                    warn!(
                        pid = self.child.id(),
                        %error,
                        "failed to terminate GDB process"
                    );
                }

                if let Err(error) = self.child.wait() {
                    warn!(
                        pid = self.child.id(),
                        %error,
                        "failed to reap GDB process"
                    );
                }

                self.join_reader_thread();
                self.terminated = true;
            }
            Err(error) => {
                warn!(
                    pid = self.child.id(),
                    %error,
                    "failed to query GDB process state during drop"
                );
            }
        }
    }
}

/// Result of polling the GDB record channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GdbRecordPoll {
    /// One record is available.
    Record(MiRecord),

    /// No record is currently available.
    Pending,

    /// GDB closed its stdout.
    EndOfFile,
}

/// Error produced while managing the GDB process.
#[derive(Debug, Error)]
pub enum GdbProcessError {
    #[error("failed to start GDB executable {executable}")]
    Spawn {
        executable: PathBuf,

        #[source]
        source: std::io::Error,
    },

    #[error("spawned GDB process does not have a writable stdin")]
    MissingStdin,

    #[error("spawned GDB process does not have a readable stdout")]
    MissingStdout,

    #[error("GDB stdin has already been closed")]
    StdinClosed,

    #[error("command builder returned token {actual:?}, but token {expected} was expected")]
    UnexpectedCommandToken { expected: u64, actual: Option<u64> },

    #[error(
        "received result token {actual:?} with class {class:?}, but token {expected} was expected"
    )]
    UnexpectedResultToken {
        expected: u64,
        actual: Option<u64>,
        class: String,
    },

    #[error(
        "GDB closed its output while waiting for command token {expected_token}; process status: {status:?}"
    )]
    UnexpectedEndOfOutput {
        status: Option<ExitStatus>,
        expected_token: u64,
    },

    #[error("failed to encode a GDB/MI command")]
    CommandFormatting(#[from] std::fmt::Error),

    #[error("I/O error while communicating with GDB")]
    Io(#[from] std::io::Error),

    #[error("invalid GDB/MI output")]
    Parse(#[from] MiParseError),

    #[error("failed to wait for GDB process")]
    Wait(std::io::Error),

    #[error("GDB process has not completed startup synchronization")]
    NotSynchronized,

    #[error("GDB process has already completed startup synchronization")]
    AlreadySynchronized,

    #[error("GDB closed its output before the initial prompt; process status: {status:?}")]
    UnexpectedEndDuringSynchronization { status: Option<ExitStatus> },

    #[error(
        "received unexpected result record during GDB startup synchronization: \
         token={token:?}, class={class:?}"
    )]
    UnexpectedResultDuringSynchronization { token: Option<u64>, class: String },

    #[error("failed to start GDB stdout reader thread")]
    ReaderThreadSpawn(#[source] std::io::Error),

    #[error("GDB stdout reader failed")]
    Reader(#[from] GdbReaderError),

    #[error("GDB stdout reader channel closed unexpectedly")]
    ReaderChannelClosed(#[source] RecvError),

    #[error("GDB stdout reader channel disconnected")]
    ReaderChannelDisconnected,
}

/// Returns whether a path appears to be executable.
///
/// This helper currently checks only that the path points to a regular file.
/// Actual execution permissions are validated by the operating system when
/// spawning GDB.
#[must_use]
pub fn is_gdb_file(path: &Path) -> bool {
    path.is_file()
}
