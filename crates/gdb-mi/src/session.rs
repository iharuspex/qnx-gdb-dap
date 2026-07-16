use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use thiserror::Error;
use tracing::{debug, info};

use crate::{
    GdbBreakpoint, GdbDeployment, GdbDeploymentResult, GdbEvent, GdbProcess, GdbProcessConfig,
    GdbProcessError, MiRecord, MiResultRecord, SourceBreakpoint, commands, find_result,
};

/// Configuration of a remote QNX debugging session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GdbSessionConfig {
    /// Path to `ntoarm-gdb`.
    pub gdb_executable: PathBuf,

    /// Local executable containing debug symbols.
    pub program: PathBuf,

    /// QNX remote target in `HOST:PORT` form.
    pub target: String,

    /// Method used to prepare the target executable.
    pub deployment: GdbDeployment,

    /// Optional working directory for the GDB host process.
    pub working_directory: Option<PathBuf>,

    /// Additional command-line arguments passed to GDB.
    pub gdb_arguments: Vec<String>,
}

impl GdbSessionConfig {
    /// Creates a QNX GDB session configuration.
    #[must_use]
    pub fn new(
        gdb_executable: impl Into<PathBuf>,
        program: impl Into<PathBuf>,
        target: impl Into<String>,
        deployment: GdbDeployment,
    ) -> Self {
        Self {
            gdb_executable: gdb_executable.into(),
            program: program.into(),
            target: target.into(),
            deployment,
            working_directory: None,
            gdb_arguments: Vec::new(),
        }
    }

    /// Sets the working directory of the host GDB process.
    #[must_use]
    pub fn working_directory(mut self, working_directory: impl Into<PathBuf>) -> Self {
        self.working_directory = Some(working_directory.into());
        self
    }

    /// Appends an argument passed directly to GDB.
    #[must_use]
    pub fn gdb_argument(mut self, argument: impl Into<String>) -> Self {
        self.gdb_arguments.push(argument.into());
        self
    }
}

/// Current state of a GDB session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GdbSessionState {
    /// GDB has not been started.
    Created,

    /// GDB has started and reached its first prompt.
    Ready,

    /// The local executable and symbols have been loaded.
    SymbolsLoaded,

    /// GDB is connected to the QNX remote target.
    Connected,

    /// The remote executable has been uploaded or selected.
    Deployed,

    /// GDB has terminated.
    Terminated,
}

/// Output produced during GDB startup and session initialization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GdbSessionOutput {
    /// Records emitted before the initial GDB prompt.
    pub startup_records: Vec<MiRecord>,

    /// Records emitted by `-gdb-version`.
    pub version_events: Vec<GdbEvent>,

    /// Records emitted while loading the executable.
    pub symbol_events: Vec<GdbEvent>,

    /// Records emitted while connecting to the remote target.
    pub target_events: Vec<GdbEvent>,

    /// Records emitted while preparing the target executable.
    pub deployment_events: Vec<GdbEvent>,

    /// Description of the prepared executable.
    pub deployment: GdbDeploymentResult,
}

/// A configured remote QNX GDB session.
#[derive(Debug)]
pub struct GdbSession {
    process: GdbProcess,
    config: GdbSessionConfig,
    state: GdbSessionState,

    /// GDB breakpoint numbers grouped by requested source file.
    source_breakpoints: HashMap<PathBuf, Vec<u64>>,
}

impl GdbSession {
    /// Starts GDB, loads symbols and connects to the QNX target.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// - the configured paths or target are invalid;
    /// - GDB cannot be started;
    /// - startup synchronization fails;
    /// - an initialization command returns an unexpected result;
    /// - GDB reports an error.
    pub fn connect(config: GdbSessionConfig) -> Result<(Self, GdbSessionOutput), GdbSessionError> {
        validate_config(&config)?;

        let mut process_config = GdbProcessConfig::new(&config.gdb_executable);

        for argument in &config.gdb_arguments {
            process_config = process_config.argument(argument);
        }

        if let Some(working_directory) = &config.working_directory {
            process_config = process_config.working_directory(working_directory);
        }

        let mut process = GdbProcess::spawn(&process_config)?;
        let startup_records = process.synchronize()?;

        let mut session = Self {
            process,
            config,
            state: GdbSessionState::Ready,
            source_breakpoints: HashMap::new(),
        };

        debug!("checking GDB version");

        let version_result = session.process.execute(commands::gdb_version)?;

        require_result_class("gdb-version", &version_result.result, &["done"])?;

        debug!(
            program = %session.config.program.display(),
            "loading executable and symbols"
        );

        let program = path_to_utf8(&session.config.program, "program executable")?;

        let symbol_result = session
            .process
            .execute(|token| commands::file_exec_and_symbols(token, program))?;

        require_result_class("file-exec-and-symbols", &symbol_result.result, &["done"])?;

        session.state = GdbSessionState::SymbolsLoaded;

        debug!(
            target = %session.config.target,
            "connecting to QNX remote target"
        );

        let target_result = session
            .process
            .execute(|token| commands::target_select_qnx(token, &session.config.target))?;

        // QNX GDB 6.8 was observed returning `^connected` for
        // `-target-select` even when invoked without all arguments.
        // A successful real connection may return either `connected`
        // or `done`, depending on the QNX GDB build.
        require_result_class(
            "target-select",
            &target_result.result,
            &["connected", "done"],
        )?;

        session.state = GdbSessionState::Connected;

        info!(
            target = %session.config.target,
            program = %session.config.program.display(),
            "QNX GDB session connected"
        );

        let deployment_result = session.prepare_deployment()?;

        session.state = GdbSessionState::Deployed;

        let output = GdbSessionOutput {
            startup_records,
            version_events: version_result.events,
            symbol_events: symbol_result.events,
            target_events: target_result.events,
            deployment_events: deployment_result.0,
            deployment: deployment_result.1,
        };

        Ok((session, output))
    }

    /// Returns the current session state.
    #[must_use]
    pub const fn state(&self) -> GdbSessionState {
        self.state
    }

    /// Returns the session configuration.
    #[must_use]
    pub const fn config(&self) -> &GdbSessionConfig {
        &self.config
    }

    /// Returns the operating-system process identifier of GDB.
    #[must_use]
    pub fn gdb_process_id(&self) -> u32 {
        self.process.id()
    }

    /// Replaces all source breakpoints for one source file.
    ///
    /// Existing breakpoints previously installed through this session for the
    /// specified source file are removed before new breakpoints are created.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// - the session is not connected;
    /// - the source path is not valid UTF-8;
    /// - old breakpoints cannot be removed;
    /// - communication with GDB fails.
    ///
    /// An individual `-break-insert` failure is returned as an unverified
    /// breakpoint and does not abort processing of the remaining lines.
    pub fn set_source_breakpoints(
        &mut self,
        source: &Path,
        breakpoints: &[SourceBreakpoint],
    ) -> Result<Vec<GdbBreakpoint>, GdbSessionError> {
        self.require_state("set source breakpoints", &[GdbSessionState::Deployed])?;

        let source_text = path_to_utf8(source, "breakpoint source file")?;

        self.remove_source_breakpoints(source)?;

        let mut results = Vec::with_capacity(breakpoints.len());
        let mut installed_numbers = Vec::new();

        for breakpoint in breakpoints {
            let location = format!("{source_text}:{}", breakpoint.line);

            debug!(
                source = %source.display(),
                line = breakpoint.line,
                %location,
                "inserting source breakpoint"
            );

            let command_result = self
                .process
                .execute(|token| commands::break_insert(token, &location))?;

            match parse_breakpoint_result(source, breakpoint.line, &command_result.result) {
                Ok(parsed) => {
                    if let Some(number) = parsed.number {
                        installed_numbers.push(number);
                    }

                    results.push(parsed);
                }

                Err(GdbSessionError::GdbCommand { message, .. }) => {
                    results.push(GdbBreakpoint::unverified(source, breakpoint.line, message));
                }

                Err(error) => return Err(error),
            }
        }

        if !installed_numbers.is_empty() {
            self.source_breakpoints
                .insert(source.to_path_buf(), installed_numbers);
        }

        Ok(results)
    }

    fn remove_source_breakpoints(&mut self, source: &Path) -> Result<(), GdbSessionError> {
        let Some(numbers) = self.source_breakpoints.get(source).cloned() else {
            return Ok(());
        };

        if numbers.is_empty() {
            self.source_breakpoints.remove(source);
            return Ok(());
        }

        debug!(
            source = %source.display(),
            breakpoint_numbers = ?numbers,
            "removing previous source breakpoints"
        );

        let result = self
            .process
            .execute(|token| commands::break_delete(token, &numbers))?;

        require_result_class("break-delete", &result.result, &["done"])?;

        self.source_breakpoints.remove(source);

        Ok(())
    }

    fn require_state(
        &self,
        operation: &'static str,
        accepted_states: &[GdbSessionState],
    ) -> Result<(), GdbSessionError> {
        if accepted_states.contains(&self.state) {
            return Ok(());
        }

        Err(GdbSessionError::InvalidSessionState {
            operation,
            actual: self.state,
            expected: accepted_states.to_vec(),
        })
    }

    fn prepare_deployment(
        &mut self,
    ) -> Result<(Vec<GdbEvent>, GdbDeploymentResult), GdbSessionError> {
        self.require_state("prepare deployment", &[GdbSessionState::Connected])?;

        let local_program = path_to_utf8(&self.config.program, "program executable")?;

        let deployment = self.config.deployment.clone();

        let remote_program = deployment.remote_program();

        validate_remote_program(remote_program)?;

        let (result, uploaded) = match &self.config.deployment {
            GdbDeployment::Upload { remote_program } => {
                info!(
                    local_program,
                    remote_program, "uploading executable to QNX target"
                );

                (
                    self.process.execute(|token| {
                        commands::qnx_upload(token, local_program, remote_program)
                    })?,
                    true,
                )
            }

            GdbDeployment::Existing { remote_program } => {
                info!(remote_program, "selecting existing QNX target executable");

                (
                    self.process
                        .execute(|token| commands::qnx_set_executable(token, remote_program))?,
                    false,
                )
            }
        };

        let operation = if uploaded {
            "upload"
        } else {
            "set nto-executable"
        };

        require_result_class(operation, &result.result, &["done"])?;

        Ok((
            result.events,
            GdbDeploymentResult {
                local_program: self.config.program.clone(),
                remote_program: remote_program.to_owned(),
                uploaded,
            },
        ))
    }

    /// Returns mutable access to the low-level GDB process.
    ///
    /// This is temporarily exposed while higher-level session commands are
    /// implemented. Callers must preserve the session state invariants.
    pub fn process_mut(&mut self) -> &mut GdbProcess {
        &mut self.process
    }

    /// Cleanly terminates GDB.
    ///
    /// # Errors
    ///
    /// Returns an error if GDB cannot be shut down cleanly.
    pub fn shutdown(&mut self) -> Result<(), GdbSessionError> {
        if self.state == GdbSessionState::Terminated {
            return Ok(());
        }

        let status = self.process.shutdown()?;

        debug!(%status, "GDB process terminated");

        self.state = GdbSessionState::Terminated;

        Ok(())
    }
}

fn validate_config(config: &GdbSessionConfig) -> Result<(), GdbSessionError> {
    validate_file(&config.gdb_executable, "GDB executable")?;
    validate_file(&config.program, "program executable")?;

    if config.target.trim().is_empty() {
        return Err(GdbSessionError::EmptyTarget);
    }

    if config.target.chars().any(char::is_whitespace) {
        return Err(GdbSessionError::InvalidTarget {
            target: config.target.clone(),
        });
    }

    Ok(())
}

fn validate_file(path: &Path, description: &'static str) -> Result<(), GdbSessionError> {
    if !path.exists() {
        return Err(GdbSessionError::PathDoesNotExist {
            description,
            path: path.to_path_buf(),
        });
    }

    if !path.is_file() {
        return Err(GdbSessionError::PathIsNotFile {
            description,
            path: path.to_path_buf(),
        });
    }

    Ok(())
}

fn path_to_utf8<'a>(path: &'a Path, description: &'static str) -> Result<&'a str, GdbSessionError> {
    path.to_str().ok_or_else(|| GdbSessionError::NonUtf8Path {
        description,
        path: path.to_path_buf(),
    })
}

fn validate_remote_program(remote_program: &str) -> Result<(), GdbSessionError> {
    if remote_program.trim().is_empty() {
        return Err(GdbSessionError::EmptyRemoteProgram);
    }

    if remote_program.contains('\0') {
        return Err(GdbSessionError::InvalidRemoteProgram {
            remote_program: remote_program.to_owned(),
        });
    }

    Ok(())
}

fn require_result_class(
    operation: &'static str,
    result: &MiResultRecord,
    accepted_classes: &[&str],
) -> Result<(), GdbSessionError> {
    if accepted_classes.iter().any(|class| result.class == *class) {
        return Ok(());
    }

    if result.class == "error" {
        let message = result
            .results
            .iter()
            .find(|result| result.variable == "msg")
            .and_then(|result| result.value.as_const())
            .unwrap_or("GDB returned an unspecified error")
            .to_owned();

        return Err(GdbSessionError::GdbCommand { operation, message });
    }

    Err(GdbSessionError::UnexpectedResultClass {
        operation,
        actual: result.class.clone(),
        expected: accepted_classes
            .iter()
            .map(|class| (*class).to_owned())
            .collect(),
    })
}

fn parse_breakpoint_result(
    requested_source: &Path,
    requested_line: u64,
    result: &MiResultRecord,
) -> Result<GdbBreakpoint, GdbSessionError> {
    require_result_class("break-insert", result, &["done"])?;

    let breakpoint =
        find_result(&result.results, "bkpt").ok_or(GdbSessionError::MissingBreakpointData)?;

    let tuple = breakpoint
        .as_tuple()
        .ok_or(GdbSessionError::InvalidBreakpointData)?;

    let number = find_tuple_const(tuple, "number").and_then(|value| value.parse::<u64>().ok());

    let resolved_line = find_tuple_const(tuple, "line")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(requested_line);

    let resolved_file = find_tuple_const(tuple, "fullname")
        .or_else(|| find_tuple_const(tuple, "file"))
        .map(PathBuf::from);

    let function = find_tuple_const(tuple, "func").map(ToOwned::to_owned);

    let address = find_tuple_const(tuple, "addr").map(ToOwned::to_owned);

    let pending = find_tuple_const(tuple, "pending");

    let verified = number.is_some() && pending.is_none();

    let message = if verified {
        None
    } else if let Some(pending_location) = pending {
        Some(format!("breakpoint is pending at {pending_location}"))
    } else {
        Some("GDB did not return a breakpoint number".to_owned())
    };

    Ok(GdbBreakpoint {
        number,
        source: requested_source.to_path_buf(),
        line: resolved_line,
        verified,
        function,
        resolved_file,
        address,
        message,
    })
}

fn find_tuple_const<'a>(results: &'a [crate::MiResult], name: &str) -> Option<&'a str> {
    find_result(results, name)?.as_const()
}

/// Error produced while configuring or managing a GDB session.
#[derive(Debug, Error)]
pub enum GdbSessionError {
    #[error("{description} does not exist: {path}")]
    PathDoesNotExist {
        description: &'static str,
        path: PathBuf,
    },

    #[error("{description} is not a regular file: {path}")]
    PathIsNotFile {
        description: &'static str,
        path: PathBuf,
    },

    #[error("{description} path is not valid UTF-8: {path:?}")]
    NonUtf8Path {
        description: &'static str,
        path: PathBuf,
    },

    #[error("QNX remote target must not be empty")]
    EmptyTarget,

    #[error("invalid QNX remote target {target:?}")]
    InvalidTarget { target: String },

    #[error("GDB command {operation:?} failed: {message}")]
    GdbCommand {
        operation: &'static str,
        message: String,
    },

    #[error(
        "GDB command {operation:?} returned result class {actual:?}; expected one of {expected:?}"
    )]
    UnexpectedResultClass {
        operation: &'static str,
        actual: String,
        expected: Vec<String>,
    },

    #[error("GDB process error")]
    Process(#[from] GdbProcessError),

    #[error("GDB did not return breakpoint data")]
    MissingBreakpointData,

    #[error("GDB returned malformed breakpoint data")]
    InvalidBreakpointData,

    #[error(
        "operation {operation:?} is not valid while GDB session is in state {actual:?}; expected one of {expected:?}"
    )]
    InvalidSessionState {
        operation: &'static str,
        actual: GdbSessionState,
        expected: Vec<GdbSessionState>,
    },

    #[error("QNX remote program path must not be empty")]
    EmptyRemoteProgram,

    #[error("invalid QNX remote program path {remote_program:?}")]
    InvalidRemoteProgram { remote_program: String },
}

#[cfg(test)]
mod tests {
    use std::{
        fs::{self, File},
        path::{Path, PathBuf},
        time::{SystemTime, UNIX_EPOCH},
    };

    use crate::{GdbDeployment, MiResult, MiResultRecord, MiValue};

    use super::{
        GdbSessionConfig, GdbSessionError, parse_breakpoint_result, require_result_class,
        validate_config, validate_remote_program,
    };

    #[test]
    fn rejects_empty_target() {
        let temporary = TemporaryFiles::new();

        let config = GdbSessionConfig::new(
            &temporary.gdb,
            &temporary.program,
            "",
            GdbDeployment::Existing {
                remote_program: ("".to_owned()),
            },
        );

        let error = validate_config(&config).expect_err("empty target should fail");

        assert!(matches!(error, GdbSessionError::EmptyTarget));
    }

    #[test]
    fn rejects_target_with_whitespace() {
        let temporary = TemporaryFiles::new();

        let config = GdbSessionConfig::new(
            &temporary.gdb,
            &temporary.program,
            "192.168.1.20:8000 invalid",
            GdbDeployment::Existing {
                remote_program: ("".to_owned()),
            },
        );

        let error = validate_config(&config).expect_err("target containing whitespace should fail");

        assert!(matches!(error, GdbSessionError::InvalidTarget { .. }));
    }

    #[test]
    fn accepts_done_result() {
        let result = MiResultRecord {
            token: Some(1),
            class: "done".to_owned(),
            results: Vec::new(),
        };

        require_result_class("test", &result, &["done"]).expect("done should be accepted");
    }

    #[test]
    fn accepts_connected_result() {
        let result = MiResultRecord {
            token: Some(1),
            class: "connected".to_owned(),
            results: Vec::new(),
        };

        require_result_class("target-select", &result, &["connected", "done"])
            .expect("connected should be accepted");
    }

    #[test]
    fn extracts_gdb_error_message() {
        let result = MiResultRecord {
            token: Some(1),
            class: "error".to_owned(),
            results: vec![MiResult::new(
                "msg",
                MiValue::Const("Connection refused.".to_owned()),
            )],
        };

        let error = require_result_class("target-select", &result, &["connected", "done"])
            .expect_err("error result should fail");

        assert!(matches!(
            error,
            GdbSessionError::GdbCommand {
                operation: "target-select",
                message,
            } if message == "Connection refused."
        ));
    }

    #[test]
    fn parses_inserted_breakpoint() {
        let result = MiResultRecord {
            token: Some(4),
            class: "done".to_owned(),
            results: vec![MiResult::new(
                "bkpt",
                MiValue::Tuple(vec![
                    MiResult::new("number", MiValue::Const("3".to_owned())),
                    MiResult::new("type", MiValue::Const("breakpoint".to_owned())),
                    MiResult::new("disp", MiValue::Const("keep".to_owned())),
                    MiResult::new("enabled", MiValue::Const("y".to_owned())),
                    MiResult::new("addr", MiValue::Const("0x00012340".to_owned())),
                    MiResult::new("func", MiValue::Const("main".to_owned())),
                    MiResult::new("file", MiValue::Const("main.cpp".to_owned())),
                    MiResult::new(
                        "fullname",
                        MiValue::Const("/home/user/project/main.cpp".to_owned()),
                    ),
                    MiResult::new("line", MiValue::Const("42".to_owned())),
                ]),
            )],
        };

        let breakpoint =
            parse_breakpoint_result(Path::new("/home/user/project/main.cpp"), 42, &result)
                .expect("breakpoint should parse");

        assert_eq!(breakpoint.number, Some(3));
        assert_eq!(breakpoint.line, 42);
        assert!(breakpoint.verified);
        assert_eq!(breakpoint.function.as_deref(), Some("main"));
        assert_eq!(
            breakpoint.resolved_file,
            Some(PathBuf::from("/home/user/project/main.cpp"))
        );
        assert_eq!(breakpoint.address.as_deref(), Some("0x00012340"));
        assert_eq!(breakpoint.message, None);
    }

    #[test]
    fn parses_pending_breakpoint() {
        let result = MiResultRecord {
            token: Some(4),
            class: "done".to_owned(),
            results: vec![MiResult::new(
                "bkpt",
                MiValue::Tuple(vec![
                    MiResult::new("number", MiValue::Const("5".to_owned())),
                    MiResult::new(
                        "pending",
                        MiValue::Const("/home/user/project/main.cpp:100".to_owned()),
                    ),
                ]),
            )],
        };

        let breakpoint =
            parse_breakpoint_result(Path::new("/home/user/project/main.cpp"), 100, &result)
                .expect("pending breakpoint should parse");

        assert_eq!(breakpoint.number, Some(5));
        assert!(!breakpoint.verified);
        assert_eq!(
            breakpoint.message.as_deref(),
            Some(
                "breakpoint is pending at \
                 /home/user/project/main.cpp:100"
            )
        );
    }

    #[test]
    fn reports_breakpoint_insert_error() {
        let result = MiResultRecord {
            token: Some(4),
            class: "error".to_owned(),
            results: vec![MiResult::new(
                "msg",
                MiValue::Const("No source file named missing.cpp.".to_owned()),
            )],
        };

        let error = parse_breakpoint_result(Path::new("missing.cpp"), 42, &result)
            .expect_err("GDB error should be returned");

        assert!(matches!(
            error,
            GdbSessionError::GdbCommand {
                operation: "break-insert",
                message,
            } if message == "No source file named missing.cpp."
        ));
    }

    #[test]
    fn rejects_breakpoint_result_without_bkpt() {
        let result = MiResultRecord {
            token: Some(4),
            class: "done".to_owned(),
            results: Vec::new(),
        };

        let error = parse_breakpoint_result(Path::new("main.cpp"), 42, &result)
            .expect_err("missing bkpt data should fail");

        assert!(matches!(error, GdbSessionError::MissingBreakpointData));
    }

    #[test]
    fn rejects_empty_remote_program() {
        let error = validate_remote_program("").expect_err("empty remote path should fail");

        assert!(matches!(error, GdbSessionError::EmptyRemoteProgram));
    }

    #[test]
    fn accepts_remote_program_with_spaces() {
        validate_remote_program("/dev/shmem/my application").expect("spaces should be accepted");
    }

    struct TemporaryFiles {
        directory: PathBuf,
        gdb: PathBuf,
        program: PathBuf,
    }

    impl TemporaryFiles {
        fn new() -> Self {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock should be valid")
                .as_nanos();

            let directory = std::env::temp_dir().join(format!("qnx-gdb-session-test-{unique}"));

            fs::create_dir(&directory).expect("temporary directory should be created");

            let gdb = directory.join("ntoarm-gdb");
            let program = directory.join("application");

            File::create(&gdb).expect("temporary GDB file should be created");

            File::create(&program).expect("temporary program file should be created");

            Self {
                directory,
                gdb,
                program,
            }
        }
    }

    impl Drop for TemporaryFiles {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.directory).expect("temporary directory should be removed");
        }
    }
}
