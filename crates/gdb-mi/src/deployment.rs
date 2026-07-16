use std::path::PathBuf;

/// Method used to make an executable available on the QNX target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GdbDeployment {
    /// Upload the local executable through `pdebug`.
    Upload {
        /// Destination path on the QNX target.
        remote_program: String,
    },

    /// Use an executable that already exists on the target.
    Existing {
        /// Existing executable path on the QNX target.
        remote_program: String,
    },
}

impl GdbDeployment {
    /// Creates upload deployment.
    #[must_use]
    pub fn upload(remote_program: impl Into<String>) -> Self {
        Self::Upload {
            remote_program: remote_program.into(),
        }
    }

    /// Creates deployment using an existing remote executable.
    #[must_use]
    pub fn existing(remote_program: impl Into<String>) -> Self {
        Self::Existing {
            remote_program: remote_program.into(),
        }
    }

    /// Returns the target executable path.
    #[must_use]
    pub fn remote_program(&self) -> &str {
        match self {
            Self::Upload { remote_program } | Self::Existing { remote_program } => remote_program,
        }
    }
}

/// Result of preparing the executable on the target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GdbDeploymentResult {
    /// Local ELF used for symbols.
    pub local_program: PathBuf,

    /// Executable selected on the QNX target.
    pub remote_program: String,

    /// Whether the executable was uploaded by this session.
    pub uploaded: bool,
}
