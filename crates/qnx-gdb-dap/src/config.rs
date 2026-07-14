use std::path::PathBuf;

use serde::Deserialize;

/// Arguments of the DAP `launch` request.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LaunchArguments {
    /// Path to the QNX GDB executable.
    pub gdb: PathBuf,

    /// Local executable containing symbols.
    pub program: PathBuf,

    /// QNX remote target in `HOST:PORT` form.
    pub target: String,

    /// Optional host-side working directory for GDB.
    #[serde(default)]
    pub working_directory: Option<PathBuf>,

    /// Additional command-line arguments passed to GDB.
    #[serde(default)]
    pub gdb_arguments: Vec<String>,
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::json;

    use super::LaunchArguments;

    #[test]
    fn deserializes_launch_arguments() {
        let arguments: LaunchArguments = serde_json::from_value(json!({
            "gdb": "/opt/qnx650/host/linux/x86/usr/bin/ntoarm-gdb",
            "program": "/home/user/build/application",
            "target": "192.168.1.28:8080",
            "workingDirectory": "/home/user/project",
            "gdbArguments": [
                "--quiet"
            ]
        }))
        .expect("launch arguments should deserialize");

        assert_eq!(
            arguments,
            LaunchArguments {
                gdb: PathBuf::from("/opt/qnx650/host/linux/x86/usr/bin/ntoarm-gdb"),
                program: PathBuf::from("/home/user/build/application"),
                target: "192.168.1.28:8080".to_owned(),
                working_directory: Some(PathBuf::from("/home/user/project")),
                gdb_arguments: vec!["--quiet".to_owned()],
            }
        );
    }

    #[test]
    fn uses_defaults_for_optional_arguments() {
        let arguments: LaunchArguments = serde_json::from_value(json!({
            "gdb": "ntoarm-gdb",
            "program": "application",
            "target": "localhost:8000"
        }))
        .expect("launch arguments should deserialize");

        assert_eq!(arguments.working_directory, None);
        assert!(arguments.gdb_arguments.is_empty());
    }
}
