use std::fmt::{self, Write as _};

/// A GDB/MI command with an optional numeric token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MiCommand {
    token: Option<u64>,
    operation: String,
    arguments: Vec<MiArgument>,
}

impl MiCommand {
    /// Creates a command without a token.
    ///
    /// The operation name must not include the leading `-`.
    #[must_use]
    pub fn new(operation: impl Into<String>) -> Self {
        Self {
            token: None,
            operation: operation.into(),
            arguments: Vec::new(),
        }
    }

    /// Assigns a numeric token to the command.
    #[must_use]
    pub const fn with_token(mut self, token: u64) -> Self {
        self.token = Some(token);
        self
    }

    /// Appends a raw argument.
    ///
    /// Raw arguments are written without quoting or escaping. Use this only
    /// for arguments whose syntax is controlled by the adapter, such as
    /// `qnx`, `--thread`, or a numeric identifier.
    #[must_use]
    pub fn raw_argument(mut self, argument: impl Into<String>) -> Self {
        self.arguments.push(MiArgument::Raw(argument.into()));
        self
    }

    /// Appends a quoted and escaped string argument.
    #[must_use]
    pub fn string_argument(mut self, argument: impl Into<String>) -> Self {
        self.arguments.push(MiArgument::String(argument.into()));
        self
    }

    /// Appends an integer argument.
    #[must_use]
    pub fn integer_argument(mut self, argument: i64) -> Self {
        self.arguments.push(MiArgument::Integer(argument));
        self
    }

    /// Returns the command token.
    #[must_use]
    pub const fn token(&self) -> Option<u64> {
        self.token
    }

    /// Returns the MI operation name without the leading `-`.
    #[must_use]
    pub fn operation(&self) -> &str {
        &self.operation
    }

    /// Serializes the command without a terminating newline.
    ///
    /// # Errors
    ///
    /// Returns an error if the internal formatting operation fails.
    pub fn encode(&self) -> Result<String, fmt::Error> {
        let mut output = String::new();

        if let Some(token) = self.token {
            write!(output, "{token}")?;
        }

        write!(output, "-{}", self.operation)?;

        for argument in &self.arguments {
            output.push(' ');
            argument.write_to(&mut output)?;
        }

        Ok(output)
    }
}

/// One argument of a GDB/MI command.
#[derive(Debug, Clone, PartialEq, Eq)]
enum MiArgument {
    Raw(String),
    String(String),
    Integer(i64),
}

impl MiArgument {
    fn write_to(&self, output: &mut String) -> fmt::Result {
        match self {
            Self::Raw(value) => output.write_str(value),
            Self::String(value) => write_quoted_string(output, value),
            Self::Integer(value) => write!(output, "{value}"),
        }
    }
}

fn write_quoted_string(output: &mut String, value: &str) -> fmt::Result {
    output.push('"');

    for character in value.chars() {
        match character {
            '\\' => output.write_str("\\\\")?,
            '"' => output.write_str("\\\"")?,
            '\n' => output.write_str("\\n")?,
            '\r' => output.write_str("\\r")?,
            '\t' => output.write_str("\\t")?,
            '\u{0008}' => output.write_str("\\b")?,
            '\u{000c}' => output.write_str("\\f")?,
            character if character.is_control() => {
                for byte in character.to_string().as_bytes() {
                    write!(output, "\\{byte:03o}")?;
                }
            }
            character => output.push(character),
        }
    }

    output.push('"');

    Ok(())
}

/// Generates monotonically increasing GDB/MI command tokens.
#[derive(Debug)]
pub struct MiTokenGenerator {
    next: u64,
}

impl MiTokenGenerator {
    /// Creates a token generator whose first token is `1`.
    #[must_use]
    pub const fn new() -> Self {
        Self { next: 1 }
    }

    /// Returns the next command token.
    ///
    /// # Panics
    ///
    /// Panics if the `u64` token space is exhausted.
    pub fn next_token(&mut self) -> u64 {
        let current = self.next;
        self.next = self
            .next
            .checked_add(1)
            .expect("GDB/MI token number overflow");

        current
    }
}

impl Default for MiTokenGenerator {
    fn default() -> Self {
        Self::new()
    }
}

/// Common GDB/MI commands used by the debug adapter.
pub mod commands {
    use super::MiCommand;

    /// Creates `-gdb-version`.
    #[must_use]
    pub fn gdb_version(token: u64) -> MiCommand {
        MiCommand::new("gdb-version").with_token(token)
    }

    /// Creates `-gdb-exit`.
    #[must_use]
    pub fn gdb_exit(token: u64) -> MiCommand {
        MiCommand::new("gdb-exit").with_token(token)
    }

    /// Creates `-file-exec-and-symbols`.
    #[must_use]
    pub fn file_exec_and_symbols(token: u64, executable: &str) -> MiCommand {
        MiCommand::new("file-exec-and-symbols")
            .with_token(token)
            .string_argument(executable)
    }

    /// Creates `-target-select qnx HOST`.
    #[must_use]
    pub fn target_select_qnx(token: u64, target: &str) -> MiCommand {
        MiCommand::new("target-select")
            .with_token(token)
            .raw_argument("qnx")
            .raw_argument(target)
    }

    /// Creates `-break-insert LOCATION`.
    #[must_use]
    pub fn break_insert(token: u64, location: &str) -> MiCommand {
        MiCommand::new("break-insert")
            .with_token(token)
            .string_argument(location)
    }

    /// Creates `-break-delete NUMBER...`.
    #[must_use]
    pub fn break_delete(token: u64, breakpoint_numbers: &[u64]) -> MiCommand {
        let mut command = MiCommand::new("break-delete").with_token(token);

        for number in breakpoint_numbers {
            command = command.raw_argument(number.to_string());
        }

        command
    }

    /// Creates a QNX GDB command that uploads a local executable to the target.
    #[must_use]
    pub fn qnx_upload(token: u64, local_program: &str, remote_program: &str) -> MiCommand {
        let command = format!(
            "upload {} {}",
            quote_console_argument(local_program),
            quote_console_argument(remote_program),
        );

        interpreter_exec_console(token, &command)
    }

    /// Creates a QNX GDB command that selects an existing target executable.
    #[must_use]
    pub fn qnx_set_executable(token: u64, remote_program: &str) -> MiCommand {
        let command = format!(
            "set nto-executable {}",
            quote_console_argument(remote_program),
        );

        interpreter_exec_console(token, &command)
    }

    fn quote_console_argument(value: &str) -> String {
        let mut output = String::with_capacity(value.len() + 2);
        output.push('"');

        for character in value.chars() {
            match character {
                '\\' => output.push_str("\\\\"),
                '"' => output.push_str("\\\""),
                '\n' => output.push_str("\\n"),
                '\r' => output.push_str("\\r"),
                '\t' => output.push_str("\\t"),
                other => output.push(other),
            }
        }

        output.push('"');
        output
    }

    /// Creates `-exec-run`.
    #[must_use]
    pub fn exec_run(token: u64) -> MiCommand {
        MiCommand::new("exec-run").with_token(token)
    }

    /// Creates `-exec-continue`.
    #[must_use]
    pub fn exec_continue(token: u64) -> MiCommand {
        MiCommand::new("exec-continue").with_token(token)
    }

    /// Creates `-exec-next`.
    #[must_use]
    pub fn exec_next(token: u64) -> MiCommand {
        MiCommand::new("exec-next").with_token(token)
    }

    /// Creates `-exec-step`.
    #[must_use]
    pub fn exec_step(token: u64) -> MiCommand {
        MiCommand::new("exec-step").with_token(token)
    }

    /// Creates `-exec-finish`.
    #[must_use]
    pub fn exec_finish(token: u64) -> MiCommand {
        MiCommand::new("exec-finish").with_token(token)
    }

    /// Creates `-exec-interrupt`.
    #[must_use]
    pub fn exec_interrupt(token: u64) -> MiCommand {
        MiCommand::new("exec-interrupt").with_token(token)
    }

    /// Creates `-stack-list-frames`.
    #[must_use]
    pub fn stack_list_frames(token: u64) -> MiCommand {
        MiCommand::new("stack-list-frames").with_token(token)
    }

    /// Creates `-stack-list-locals PRINT_VALUES`.
    #[must_use]
    pub fn stack_list_locals(token: u64, print_values: i64) -> MiCommand {
        MiCommand::new("stack-list-locals")
            .with_token(token)
            .integer_argument(print_values)
    }

    /// Creates `-stack-list-arguments PRINT_VALUES`.
    #[must_use]
    pub fn stack_list_arguments(token: u64, print_values: i64) -> MiCommand {
        MiCommand::new("stack-list-arguments")
            .with_token(token)
            .integer_argument(print_values)
    }

    /// Creates `-data-evaluate-expression EXPRESSION`.
    #[must_use]
    pub fn data_evaluate_expression(token: u64, expression: &str) -> MiCommand {
        MiCommand::new("data-evaluate-expression")
            .with_token(token)
            .string_argument(expression)
    }

    /// Creates `-var-create NAME FRAME EXPRESSION`.
    #[must_use]
    pub fn var_create(token: u64, name: &str, frame: &str, expression: &str) -> MiCommand {
        MiCommand::new("var-create")
            .with_token(token)
            .string_argument(name)
            .raw_argument(frame)
            .string_argument(expression)
    }

    /// Creates `-var-list-children PRINT_VALUES NAME`.
    #[must_use]
    pub fn var_list_children(token: u64, print_values: i64, name: &str) -> MiCommand {
        MiCommand::new("var-list-children")
            .with_token(token)
            .integer_argument(print_values)
            .string_argument(name)
    }

    /// Creates `-interpreter-exec console COMMAND`.
    #[must_use]
    pub fn interpreter_exec_console(token: u64, console_command: &str) -> MiCommand {
        MiCommand::new("interpreter-exec")
            .with_token(token)
            .raw_argument("console")
            .string_argument(console_command)
    }
}

#[cfg(test)]
mod tests {
    use super::{MiCommand, MiTokenGenerator, commands};

    #[test]
    fn encodes_command_without_arguments() {
        let command = MiCommand::new("gdb-version").with_token(12);

        assert_eq!(
            command.encode().expect("command should encode"),
            "12-gdb-version"
        );
    }

    #[test]
    fn encodes_raw_and_string_arguments() {
        let command = MiCommand::new("target-select")
            .with_token(3)
            .raw_argument("qnx")
            .string_argument("192.168.1.20:8000");

        assert_eq!(
            command.encode().expect("command should encode"),
            r#"3-target-select qnx "192.168.1.20:8000""#
        );
    }

    #[test]
    fn escapes_string_argument() {
        let command = MiCommand::new("data-evaluate-expression")
            .with_token(7)
            .string_argument("object.field[\"name\"]\n");

        assert_eq!(
            command.encode().expect("command should encode"),
            r#"7-data-evaluate-expression "object.field[\"name\"]\n""#
        );
    }

    #[test]
    fn escapes_windows_style_path() {
        let command = commands::file_exec_and_symbols(1, r"C:\project\build\application");

        assert_eq!(
            command.encode().expect("command should encode"),
            r#"1-file-exec-and-symbols "C:\\project\\build\\application""#
        );
    }

    #[test]
    fn generates_monotonic_tokens() {
        let mut generator = MiTokenGenerator::new();

        assert_eq!(generator.next_token(), 1);
        assert_eq!(generator.next_token(), 2);
        assert_eq!(generator.next_token(), 3);
    }

    #[test]
    fn creates_file_exec_and_symbols_command() {
        let command = commands::file_exec_and_symbols(1, "/home/user/project/build/app");

        assert_eq!(
            command.encode().expect("command should encode"),
            r#"1-file-exec-and-symbols "/home/user/project/build/app""#
        );
    }

    #[test]
    fn creates_target_select_qnx_command() {
        let command = commands::target_select_qnx(2, "192.168.1.20:8000");

        assert_eq!(
            command.encode().expect("command should encode"),
            r"2-target-select qnx 192.168.1.20:8000"
        );
    }

    #[test]
    fn creates_break_insert_command() {
        let command = commands::break_insert(3, "/project/src/main.cpp:42");

        assert_eq!(
            command.encode().expect("command should encode"),
            r#"3-break-insert "/project/src/main.cpp:42""#
        );
    }

    #[test]
    fn creates_break_delete_command() {
        let command = commands::break_delete(4, &[1, 7, 12]);

        assert_eq!(
            command.encode().expect("command should encode"),
            "4-break-delete 1 7 12"
        );
    }

    #[test]
    fn creates_execution_commands() {
        assert_eq!(
            commands::exec_run(1)
                .encode()
                .expect("command should encode"),
            "1-exec-run"
        );

        assert_eq!(
            commands::exec_continue(2)
                .encode()
                .expect("command should encode"),
            "2-exec-continue"
        );

        assert_eq!(
            commands::exec_next(3)
                .encode()
                .expect("command should encode"),
            "3-exec-next"
        );

        assert_eq!(
            commands::exec_step(4)
                .encode()
                .expect("command should encode"),
            "4-exec-step"
        );

        assert_eq!(
            commands::exec_finish(5)
                .encode()
                .expect("command should encode"),
            "5-exec-finish"
        );

        assert_eq!(
            commands::exec_interrupt(6)
                .encode()
                .expect("command should encode"),
            "6-exec-interrupt"
        );
    }

    #[test]
    fn creates_stack_commands() {
        assert_eq!(
            commands::stack_list_frames(1)
                .encode()
                .expect("command should encode"),
            "1-stack-list-frames"
        );

        assert_eq!(
            commands::stack_list_locals(2, 1)
                .encode()
                .expect("command should encode"),
            "2-stack-list-locals 1"
        );

        assert_eq!(
            commands::stack_list_arguments(3, 1)
                .encode()
                .expect("command should encode"),
            "3-stack-list-arguments 1"
        );
    }

    #[test]
    fn creates_qnx_upload_command() {
        let command =
            commands::qnx_upload(4, "/home/user/build/application", "/dev/shmem/application");

        assert_eq!(
            command.encode().expect("command should encode"),
            r#"4-interpreter-exec console "upload \"/home/user/build/application\" \"/dev/shmem/application\"""#
        );
    }

    #[test]
    fn creates_qnx_set_executable_command() {
        let command = commands::qnx_set_executable(5, "/opt/application/bin/app");

        assert_eq!(
            command.encode().expect("command should encode"),
            r#"5-interpreter-exec console "set nto-executable \"/opt/application/bin/app\"""#
        );
    }

    #[test]
    fn quotes_console_paths_containing_spaces() {
        let command = commands::qnx_upload(1, "/home/user/my project/app", "/dev/shmem/my app");

        assert_eq!(
            command.encode().expect("command should encode"),
            r#"1-interpreter-exec console "upload \"/home/user/my project/app\" \"/dev/shmem/my app\"""#
        );
    }

    #[test]
    fn creates_data_evaluate_expression_command() {
        let command = commands::data_evaluate_expression(9, "counter + 1");

        assert_eq!(
            command.encode().expect("command should encode"),
            r#"9-data-evaluate-expression "counter + 1""#
        );
    }

    #[test]
    fn creates_var_create_command() {
        let command = commands::var_create(10, "watch_1", "*", "object.member");

        assert_eq!(
            command.encode().expect("command should encode"),
            r#"10-var-create "watch_1" * "object.member""#
        );
    }

    #[test]
    fn creates_var_list_children_command() {
        let command = commands::var_list_children(11, 1, "watch_1");

        assert_eq!(
            command.encode().expect("command should encode"),
            r#"11-var-list-children 1 "watch_1""#
        );
    }

    #[test]
    fn creates_console_command() {
        let command = commands::interpreter_exec_console(12, "info threads");

        assert_eq!(
            command.encode().expect("command should encode"),
            r#"12-interpreter-exec console "info threads""#
        );
    }
}
