//! Debug Adapter Protocol messages and transport.
//!
//! This crate is independent of GDB and QNX. It is responsible only for:
//!
//! - DAP message framing;
//! - JSON serialization and deserialization;
//! - request, response and event types.

#![forbid(unsafe_code)]

mod transport;

pub use transport::{
    DEFAULT_MAX_CONTENT_LENGTH, DapReadError, DapReader, DapWriteError, DapWriter,
};

/// Current internal version of the DAP implementation.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::VERSION;

    #[test]
    fn version_is_not_empty() {
        assert!(!VERSION.is_empty());
    }
}
