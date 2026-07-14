//! GDB Machine Interface client.
//!
//! The implementation targets the MI dialect provided by:
//!
//! `GNU gdb 6.8 qnx-nto (rev. 506)`
//!
//! It must not assume that modern GDB/MI commands are available.

#![forbid(unsafe_code)]

/// GDB version against which the initial implementation is developed.
pub const REFERENCE_GDB_VERSION: &str = "GNU gdb 6.8 qnx-nto (rev. 506)";

#[cfg(test)]
mod tests {
    use super::REFERENCE_GDB_VERSION;

    #[test]
    fn reference_version_mentions_qnx() {
        assert!(REFERENCE_GDB_VERSION.contains("qnx-nto"));
    }
}
