use std::path::{Path, PathBuf};

use thiserror::Error;
use tracing::{debug, info};

use crate::{
    GdbCommandResult, GdbEvent, GdbProcess, GdbProcessConfig, GdbProcessError, MiRecord,
    MiResultRecord, commands,
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
    ) -> Self {
        Self {
            gdb_executable: gdb_executable.into(),
            program: program.into(),
            target: target.into(),
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
}

/// A configured remote QNX GDB session.
#[derive(Debug)]
pub struct GdbSession {
    process: GdbProcess,
    config: GdbSessionConfig,
    state: GdbSessionState,
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

        let output = GdbSessionOutput {
            startup_records,
            version_events: version_result.events,
            symbol_events: symbol_result.events,
            target_events: target_result.events,
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
}

#[cfg(test)]
mod tests {
    use std::{
        fs::{self, File},
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    use crate::{MiResult, MiResultRecord, MiValue};

    use super::{GdbSessionConfig, GdbSessionError, require_result_class, validate_config};

    #[test]
    fn rejects_empty_target() {
        let temporary = TemporaryFiles::new();

        let config = GdbSessionConfig::new(&temporary.gdb, &temporary.program, "");

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
