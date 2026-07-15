use std::path::PathBuf;

use qnx_gdb_mi::GdbBreakpoint;
use serde::{Deserialize, Serialize};

/// DAP source descriptor.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DapSource {
    /// User-facing source name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Absolute or workspace-relative source path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
}

/// One requested source breakpoint.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DapSourceBreakpoint {
    /// One-based source line.
    pub line: u64,

    /// Optional one-based source column.
    #[serde(default)]
    pub column: Option<u64>,

    /// Conditional expressions are not supported in version 0.1.
    #[serde(default)]
    pub condition: Option<String>,

    /// Hit conditions are not supported in version 0.1.
    #[serde(default)]
    pub hit_condition: Option<String>,

    /// Log points are not supported in version 0.1.
    #[serde(default)]
    pub log_message: Option<String>,
}

/// Arguments of the DAP `setBreakpoints` request.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SetBreakpointsArguments {
    /// Source file whose complete breakpoint list is being replaced.
    pub source: DapSource,

    /// Requested breakpoints.
    #[serde(default)]
    pub breakpoints: Vec<DapSourceBreakpoint>,

    /// Whether the source contents have changed since they were read.
    #[serde(default)]
    pub source_modified: bool,
}

/// DAP representation of a breakpoint.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DapBreakpoint {
    /// Adapter-specific breakpoint identifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,

    /// Whether GDB resolved the breakpoint.
    pub verified: bool,

    /// Diagnostic message for an unresolved breakpoint.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,

    /// Resolved source.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<DapSource>,

    /// Resolved one-based source line.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u64>,

    /// Instruction address reported by GDB.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instruction_reference: Option<String>,
}

impl From<GdbBreakpoint> for DapBreakpoint {
    fn from(breakpoint: GdbBreakpoint) -> Self {
        let resolved_path = breakpoint
            .resolved_file
            .clone()
            .unwrap_or_else(|| breakpoint.source.clone());

        let name = resolved_path
            .file_name()
            .and_then(|name| name.to_str())
            .map(ToOwned::to_owned);

        Self {
            id: breakpoint.number,
            verified: breakpoint.verified,
            message: breakpoint.message,
            source: Some(DapSource {
                name,
                path: Some(resolved_path),
            }),
            line: Some(breakpoint.line),
            instruction_reference: breakpoint.address,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use qnx_gdb_mi::GdbBreakpoint;
    use serde_json::json;

    use super::{DapBreakpoint, SetBreakpointsArguments};

    #[test]
    fn deserializes_set_breakpoints_arguments() {
        let arguments: SetBreakpointsArguments = serde_json::from_value(json!({
            "source": {
                "name": "main.c",
                "path": "/home/user/project/main.c"
            },
            "breakpoints": [
                {
                    "line": 7
                },
                {
                    "line": 10,
                    "column": 3
                }
            ],
            "sourceModified": false
        }))
        .expect("setBreakpoints arguments should deserialize");

        assert_eq!(
            arguments.source.path,
            Some(PathBuf::from("/home/user/project/main.c"))
        );

        assert_eq!(arguments.breakpoints.len(), 2);
        assert_eq!(arguments.breakpoints[0].line, 7);
        assert_eq!(arguments.breakpoints[1].line, 10);
        assert_eq!(arguments.breakpoints[1].column, Some(3));
        assert!(!arguments.source_modified);
    }

    #[test]
    fn converts_verified_gdb_breakpoint() {
        let breakpoint = GdbBreakpoint {
            number: Some(4),
            source: PathBuf::from("/requested/main.c"),
            line: 12,
            verified: true,
            function: Some("main".to_owned()),
            resolved_file: Some(PathBuf::from("/resolved/project/main.c")),
            address: Some("0x001007d8".to_owned()),
            message: None,
        };

        let dap = DapBreakpoint::from(breakpoint);

        assert_eq!(dap.id, Some(4));
        assert!(dap.verified);
        assert_eq!(dap.line, Some(12));
        assert_eq!(dap.instruction_reference.as_deref(), Some("0x001007d8"));

        let source = dap.source.expect("source should be present");

        assert_eq!(source.name.as_deref(), Some("main.c"));
        assert_eq!(source.path, Some(PathBuf::from("/resolved/project/main.c")));
    }

    #[test]
    fn converts_unverified_gdb_breakpoint() {
        let breakpoint =
            GdbBreakpoint::unverified("/project/missing.c", 100, "No source file named missing.c.");

        let dap = DapBreakpoint::from(breakpoint);

        assert_eq!(dap.id, None);
        assert!(!dap.verified);
        assert_eq!(dap.line, Some(100));
        assert_eq!(
            dap.message.as_deref(),
            Some("No source file named missing.c.")
        );
    }
}
