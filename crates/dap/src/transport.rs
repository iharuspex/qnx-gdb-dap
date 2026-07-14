use std::io::{BufRead, Write};

use serde::{Serialize, de::DeserializeOwned};
use thiserror::Error;

pub const DEFAULT_MAX_CONTENT_LENGTH: usize = 16 * 1024 * 1024;

const CONTENT_LENGTH_HEADER: &str = "content-length";

#[derive(Debug)]
pub struct DapReader<R> {
    inner: R,
    max_content_length: usize,
}

impl<R: BufRead> DapReader<R> {
    pub const fn new(inner: R) -> Self {
        Self {
            inner,
            max_content_length: DEFAULT_MAX_CONTENT_LENGTH,
        }
    }

    pub const fn with_max_content_length(inner: R, max_content_length: usize) -> Self {
        Self {
            inner,
            max_content_length,
        }
    }

    /// Reads and deserializes the next DAP message.
    ///
    /// Returns `Ok(None)` only when the input reaches EOF before the beginning
    /// of another message.
    ///
    /// # Errors
    ///
    /// Returns an error when:
    ///
    /// - the headers are malformed;
    /// - `Content-Length` is missing or invalid;
    /// - the body is incomplete;
    /// - the body exceeds the configured limit;
    /// - the body does not contain valid JSON.
    pub fn read_message<T>(&mut self) -> Result<Option<T>, DapReadError>
    where
        T: DeserializeOwned,
    {
        let Some(content_length) = self.read_headers()? else {
            return Ok(None);
        };

        if content_length > self.max_content_length {
            return Err(DapReadError::MessageTooLarge {
                content_length,
                maximum: self.max_content_length,
            });
        }

        let mut body = vec![0_u8; content_length];
        self.inner.read_exact(&mut body)?;

        let message = serde_json::from_slice(&body)?;

        Ok(Some(message))
    }

    /// Returns ownership of the underlying stread.
    pub fn into_inner(self) -> R {
        self.inner
    }

    fn read_headers(&mut self) -> Result<Option<usize>, DapReadError> {
        let mut content_length = None;
        let mut read_any_data = false;

        loop {
            let mut line = String::new();
            let bytes_read = self.inner.read_line(&mut line)?;

            if bytes_read == 0 {
                if read_any_data {
                    return Err(DapReadError::UnexpectedEndOfHeaders);
                }

                return Ok(None);
            }

            read_any_data = true;

            let line = line.trim_end_matches(['\r', '\n']);

            if line.is_empty() {
                break;
            }

            let (name, value) = line
                .split_once(':')
                .ok_or_else(|| DapReadError::MalformedHeader(line.to_owned()))?;

            if name.trim().eq_ignore_ascii_case(CONTENT_LENGTH_HEADER) {
                if content_length.is_some() {
                    return Err(DapReadError::DuplicateContentLength);
                }

                let value = value.trim();

                let parsed = value.parse::<usize>().map_err(|source| {
                    DapReadError::InvalidContentLength {
                        value: value.to_owned(),
                        source,
                    }
                })?;

                content_length = Some(parsed);
            }
        }

        content_length
            .ok_or(DapReadError::MissingContentLength)
            .map(Some)
    }
}

/// Writes DAP messages to an output stream
#[derive(Debug)]
pub struct DapWriter<W> {
    inner: W,
}

impl<W: Write> DapWriter<W> {
    pub const fn new(inner: W) -> Self {
        Self { inner }
    }

    /// Serializes and writes one DAP message.
    ///
    /// # Errors
    ///
    /// Returns an error when the message cannot be serialized or written to the
    /// underlying output stream.
    pub fn write_message<T>(&mut self, message: &T) -> Result<(), DapWriteError>
    where
        T: Serialize,
    {
        let body = serde_json::to_vec(message)?;

        write!(self.inner, "Content-Length: {}\r\n\r\n", body.len())?;
        self.inner.write_all(&body)?;
        self.inner.flush()?;

        Ok(())
    }

    /// Returns ownership of the underlying output stream.
    pub fn into_inner(self) -> W {
        self.inner
    }
}

/// Error produced while reading a DAP message
#[derive(Debug, Error)]
pub enum DapReadError {
    #[error("I/O error while reading a DAP message")]
    Io(#[from] std::io::Error),

    #[error("unexpected end of input while reading DAP headers")]
    UnexpectedEndOfHeaders,

    #[error("malformed DAP header: {0}")]
    MalformedHeader(String),

    #[error("DAP message does not contain a Content-Length header")]
    MissingContentLength,

    #[error("DAP message contains mode than one Content-Length header")]
    DuplicateContentLength,

    #[error("invalid Content-Length value {value:?}")]
    InvalidContentLength {
        value: String,

        #[source]
        source: std::num::ParseIntError,
    },

    #[error(
        "DAP message is too large: declared {content_length} bytes, maximum is {maximum} bytes"
    )]
    MessageTooLarge {
        content_length: usize,
        maximum: usize,
    },

    #[error("invalid JSON in DAP message")]
    InvalidJson(#[from] serde_json::Error),
}

#[derive(Debug, Error)]
pub enum DapWriteError {
    #[error("failed to serialize DAP message")]
    Serialization(#[from] serde_json::Error),

    #[error("I/O error while writing a DAP message")]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use std::io::{BufReader, Cursor};

    use serde::{Deserialize, Serialize};
    use serde_json::{Value, json};

    use super::{DapReadError, DapReader, DapWriter};

    #[derive(Debug, Deserialize, PartialEq, Eq, Serialize)]
    struct TestMessage {
        seq: u64,
        #[serde(rename = "type")]
        message_type: String,
        command: String,
    }

    #[test]
    fn reads_one_message() {
        let body = br#"{"seq":1,"type":"request","command":"initialize"}"#;

        let input = framed_message(body);
        let cursor = Cursor::new(input);
        let mut reader = DapReader::new(BufReader::new(cursor));

        let message = reader
            .read_message::<TestMessage>()
            .expect("message should be valid")
            .expect("message should be present");

        assert_eq!(
            message,
            TestMessage {
                seq: 1,
                message_type: "request".to_owned(),
                command: "initialize".to_owned(),
            }
        );
    }

    #[test]
    fn reads_multiple_messages() {
        let first = br#"{"seq":1,"type":"request","command":"initialize"}"#;
        let second = br#"{"seq":2,"type":"request","command":"launch"}"#;

        let mut input = framed_message(first);
        input.extend(framed_message(second));

        let cursor = Cursor::new(input);
        let mut reader = DapReader::new(BufReader::new(cursor));

        let first_message = reader
            .read_message::<TestMessage>()
            .expect("first message should be valid")
            .expect("first message should be present");

        let second_message = reader
            .read_message::<TestMessage>()
            .expect("second message should be valid")
            .expect("second message should be present");

        let end = reader
            .read_message::<TestMessage>()
            .expect("EOF should not be an error");

        assert_eq!(first_message.seq, 1);
        assert_eq!(second_message.seq, 2);
        assert_eq!(end, None);
    }

    #[test]
    fn accepts_case_insensitive_content_length_header() {
        let body = br#"{"seq":1}"#;
        let input = format!("content-length: {}\r\n\r\n", body.len())
            .into_bytes()
            .into_iter()
            .chain(body.iter().copied())
            .collect::<Vec<_>>();

        let cursor = Cursor::new(input);
        let mut reader = DapReader::new(BufReader::new(cursor));

        let message = reader
            .read_message::<Value>()
            .expect("message should be valid")
            .expect("message should be present");

        assert_eq!(message, json!({ "seq": 1 }));
    }

    #[test]
    fn ignores_unknown_headers() {
        let body = br#"{"seq":1}"#;

        let mut input = format!(
            "Content-Type: application/vscode-jsonrpc; charset=utf-8\r\n\
             Content-Length: {}\r\n\
             X-Debug-Header: test\r\n\
             \r\n",
            body.len()
        )
        .into_bytes();

        input.extend_from_slice(body);

        let cursor = Cursor::new(input);
        let mut reader = DapReader::new(BufReader::new(cursor));

        let message = reader
            .read_message::<Value>()
            .expect("message should be valid")
            .expect("message should be present");

        assert_eq!(message, json!({ "seq": 1 }));
    }

    #[test]
    fn rejects_missing_content_length() {
        let input = b"Content-Type: application/json\r\n\r\n{}";
        let cursor = Cursor::new(input);
        let mut reader = DapReader::new(BufReader::new(cursor));

        let error = reader
            .read_message::<Value>()
            .expect_err("missing Content-Length should fail");

        assert!(matches!(error, DapReadError::MissingContentLength));
    }

    #[test]
    fn rejects_duplicate_content_length() {
        let input = b"Content-Length: 2\r\nContent-Length: 2\r\n\r\n{}";
        let cursor = Cursor::new(input);
        let mut reader = DapReader::new(BufReader::new(cursor));

        let error = reader
            .read_message::<Value>()
            .expect_err("duplicate Content-Length should fail");

        assert!(matches!(error, DapReadError::DuplicateContentLength));
    }

    #[test]
    fn rejects_invalid_content_length() {
        let input = b"Content-Length: invalid\r\n\r\n{}";
        let cursor = Cursor::new(input);
        let mut reader = DapReader::new(BufReader::new(cursor));

        let error = reader
            .read_message::<Value>()
            .expect_err("invalid Content-Length should fail");

        assert!(matches!(error, DapReadError::InvalidContentLength { .. }));
    }

    #[test]
    fn rejects_message_larger_than_configured_limit() {
        let input = b"Content-Length: 11\r\n\r\n{\"seq\":123}";
        let cursor = Cursor::new(input);
        let mut reader = DapReader::with_max_content_length(BufReader::new(cursor), 10);

        let error = reader
            .read_message::<Value>()
            .expect_err("oversized message should fail");

        assert!(matches!(
            error,
            DapReadError::MessageTooLarge {
                content_length: 11,
                maximum: 10,
            }
        ));
    }

    #[test]
    fn rejects_truncated_body() {
        let input = b"Content-Length: 20\r\n\r\n{}";
        let cursor = Cursor::new(input);
        let mut reader = DapReader::new(BufReader::new(cursor));

        let error = reader
            .read_message::<Value>()
            .expect_err("truncated body should fail");

        assert!(matches!(error, DapReadError::Io(_)));
    }

    #[test]
    fn rejects_truncated_headers() {
        let input = b"Content-Length: 10\r\n";
        let cursor = Cursor::new(input);
        let mut reader = DapReader::new(BufReader::new(cursor));

        let error = reader
            .read_message::<Value>()
            .expect_err("truncated headers should fail");

        assert!(matches!(error, DapReadError::UnexpectedEndOfHeaders));
    }

    #[test]
    fn writes_framed_message() {
        let message = TestMessage {
            seq: 1,
            message_type: "request".to_owned(),
            command: "initialize".to_owned(),
        };

        let mut writer = DapWriter::new(Vec::new());

        writer
            .write_message(&message)
            .expect("message should be written");

        let output = writer.into_inner();
        let expected_body = serde_json::to_vec(&message).expect("message should serialize");

        let mut expected = format!("Content-Length: {}\r\n\r\n", expected_body.len()).into_bytes();
        expected.extend(expected_body);

        assert_eq!(output, expected);
    }

    #[test]
    fn writer_output_can_be_read_back() {
        let original = TestMessage {
            seq: 42,
            message_type: "request".to_owned(),
            command: "continue".to_owned(),
        };

        let mut writer = DapWriter::new(Vec::new());
        writer
            .write_message(&original)
            .expect("message should be written");

        let output = writer.into_inner();
        let cursor = Cursor::new(output);
        let mut reader = DapReader::new(BufReader::new(cursor));

        let decoded = reader
            .read_message::<TestMessage>()
            .expect("message should be valid")
            .expect("message should be present");

        assert_eq!(decoded, original);
    }

    fn framed_message(body: &[u8]) -> Vec<u8> {
        let mut output = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
        output.extend_from_slice(body);
        output
    }
}
