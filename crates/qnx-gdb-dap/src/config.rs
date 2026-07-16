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

    pub deployment: DeploymentArguments,

    /// Optional host-side working directory for GDB.
    #[serde(default)]
    pub working_directory: Option<PathBuf>,

    /// Additional command-line arguments passed to GDB.
    #[serde(default)]
    pub gdb_arguments: Vec<String>,
}

/// Arguments of the DAP `disconnect` request.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct DisconnectArguments {
    /// Whether the debug adapter should terminate the inferior.
    ///
    /// Inferior termination is not implemented yet, but the field is accepted
    /// for DAP compatibility.
    #[serde(default)]
    pub terminate_debuggee: bool,

    /// Whether the debug adapter should suspend the inferior.
    ///
    /// This is not supported by the first adapter version.
    #[serde(default)]
    pub suspend_debuggee: bool,
}

/// Target deployment configuration.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(tag = "mode", rename_all = "camelCase")]
pub enum DeploymentArguments {
    Upload {
        #[serde(rename = "remoteProgram")]
        remote_program: String,
    },
    Existing {
        #[serde(rename = "remoteProgram")]
        remote_program: String,
    },
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::json;

    use super::{DeploymentArguments, DisconnectArguments, LaunchArguments};

    #[test]
    fn deserializes_launch_arguments() {
        let arguments: LaunchArguments = serde_json::from_value(json!({
            "gdb": "/opt/qnx650/host/linux/x86/usr/bin/ntoarm-gdb",
            "program": "/home/user/build/application",
            "target": "192.168.1.28:8080",
            "deployment": {
                "mode": "upload",
                "remoteProgram": "/dev/shmem/application"
            },
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
                deployment: DeploymentArguments::Upload {
                    remote_program: ("/dev/shmem/application".to_owned())
                },
                working_directory: Some(PathBuf::from("/home/user/project")),
                gdb_arguments: vec!["--quiet".to_owned()],
            }
        );
    }

    #[test]
    fn deserializes_upload_deployment() {
        let arguments: LaunchArguments = serde_json::from_value(json!({
            "gdb": "ntoarm-gdb",
            "program": "/local/application",
            "target": "192.168.1.28:8080",
            "deployment": {
                "mode": "upload",
                "remoteProgram": "/dev/shmem/application"
            }
        }))
        .expect("launch arguments should deserialize");

        assert_eq!(
            arguments.deployment,
            DeploymentArguments::Upload {
                remote_program: "/dev/shmem/application".to_owned(),
            }
        );
    }

    #[test]
    fn uses_defaults_for_optional_arguments() {
        let arguments: LaunchArguments = serde_json::from_value(json!({
            "gdb": "ntoarm-gdb",
            "program": "application",
            "target": "localhost:8000",
            "deployment": {
                "mode": "upload",
                "remoteProgram": "/dev/shmem/application"
            }
        }))
        .expect("launch arguments should deserialize");

        assert_eq!(arguments.working_directory, None);
        assert!(arguments.gdb_arguments.is_empty());
    }

    #[test]
    fn deserializes_disconnect_arguments() {
        let arguments: DisconnectArguments = serde_json::from_value(json!({
            "terminateDebuggee": true,
            "suspendDebuggee": false
        }))
        .expect("disconnect arguments should deserialize");

        assert_eq!(
            arguments,
            DisconnectArguments {
                terminate_debuggee: true,
                suspend_debuggee: false,
            }
        );
    }

    #[test]
    fn uses_disconnect_argument_defaults() {
        let arguments: DisconnectArguments = serde_json::from_value(json!({}))
            .expect("empty disconnect arguments should deserialize");

        assert_eq!(arguments, DisconnectArguments::default());
    }
}
