use thiserror::Error;

use crate::{MiAsyncRecord, MiListItem, MiRecord, MiResult, MiResultRecord, MiValue};

/// Parses one complete line of GDB/MI output.
///
/// The terminating newline must not be included.
///
/// # Errors
///
/// Returns an error when the line contains malformed or unsupported MI syntax.
pub fn parse_record(line: &str) -> Result<MiRecord, MiParseError> {
    if line.is_empty() {
        return Ok(MiRecord::Empty);
    }

    if line == "(gdb)" || line == "(gdb) " {
        return Ok(MiRecord::Prompt);
    }

    let mut parser = Parser::new(line);
    let token = parser.parse_optional_token()?;

    let Some(prefix) = parser.next_char() else {
        return Err(MiParseError::MissingRecordPrefix);
    };

    let record = match prefix {
        '^' => MiRecord::Result(parser.parse_result_record(token)?),
        '*' => MiRecord::ExecAsync(parser.parse_async_record(token)?),
        '+' => MiRecord::StatusAsync(parser.parse_async_record(token)?),
        '=' => MiRecord::NotifyAsync(parser.parse_async_record(token)?),
        '~' => MiRecord::ConsoleStream(parser.parse_stream_record()?),
        '@' => MiRecord::TargetStream(parser.parse_stream_record()?),
        '&' => MiRecord::LogStream(parser.parse_stream_record()?),
        other => {
            return Err(MiParseError::UnknownRecordPrefix(other));
        }
    };

    parser.expect_end()?;

    Ok(record)
}

#[derive(Debug)]
struct Parser<'a> {
    input: &'a str,
    position: usize,
}

impl<'a> Parser<'a> {
    const fn new(input: &'a str) -> Self {
        Self { input, position: 0 }
    }

    fn parse_optional_token(&mut self) -> Result<Option<u64>, MiParseError> {
        let start = self.position;

        while self
            .peek_char()
            .is_some_and(|character| character.is_ascii_digit())
        {
            self.next_char();
        }

        if self.position == start {
            return Ok(None);
        }

        let token_text = &self.input[start..self.position];
        let token = token_text
            .parse::<u64>()
            .map_err(|source| MiParseError::InvalidToken {
                value: token_text.to_owned(),
                source,
            })?;

        Ok(Some(token))
    }

    fn parse_result_record(&mut self, token: Option<u64>) -> Result<MiResultRecord, MiParseError> {
        let class = self.parse_identifier()?;
        let results = self.parse_optional_results()?;

        Ok(MiResultRecord {
            token,
            class,
            results,
        })
    }

    fn parse_async_record(&mut self, token: Option<u64>) -> Result<MiAsyncRecord, MiParseError> {
        let class = self.parse_identifier()?;
        let results = self.parse_optional_results()?;

        Ok(MiAsyncRecord {
            token,
            class,
            results,
        })
    }

    fn parse_stream_record(&mut self) -> Result<String, MiParseError> {
        self.parse_c_string()
    }

    fn parse_optional_results(&mut self) -> Result<Vec<MiResult>, MiParseError> {
        let mut results = Vec::new();

        while self.consume_char(',') {
            results.push(self.parse_result()?);
        }

        Ok(results)
    }

    fn parse_result(&mut self) -> Result<MiResult, MiParseError> {
        let variable = self.parse_identifier()?;

        self.expect_char('=')?;

        let value = self.parse_value()?;

        Ok(MiResult::new(variable, value))
    }

    fn parse_value(&mut self) -> Result<MiValue, MiParseError> {
        match self.peek_char() {
            Some('"') => self.parse_c_string().map(MiValue::Const),
            Some('{') => self.parse_tuple(),
            Some('[') => self.parse_list(),
            Some(_) => self.parse_bare_const(),
            None => Err(MiParseError::UnexpectedEndOfInput),
        }
    }

    fn parse_tuple(&mut self) -> Result<MiValue, MiParseError> {
        self.expect_char('{')?;

        let mut results = Vec::new();

        if self.consume_char('}') {
            return Ok(MiValue::Tuple(results));
        }

        loop {
            results.push(self.parse_result()?);

            if self.consume_char('}') {
                break;
            }

            self.expect_char(',')?;
        }

        Ok(MiValue::Tuple(results))
    }

    fn parse_list(&mut self) -> Result<MiValue, MiParseError> {
        self.expect_char('[')?;

        let mut items = Vec::new();

        if self.consume_char(']') {
            return Ok(MiValue::List(items));
        }

        loop {
            items.push(self.parse_list_item()?);

            if self.consume_char(']') {
                break;
            }

            self.expect_char(',')?;
        }

        Ok(MiValue::List(items))
    }

    fn parse_list_item(&mut self) -> Result<MiListItem, MiParseError> {
        if self.looks_like_result() {
            return self.parse_result().map(MiListItem::Result);
        }

        self.parse_value().map(MiListItem::Value)
    }

    fn looks_like_result(&self) -> bool {
        let remaining = &self.input[self.position..];
        let mut characters = remaining.char_indices();

        let Some((_, first)) = characters.next() else {
            return false;
        };

        if !is_identifier_character(first) {
            return false;
        }

        for (_, character) in characters {
            if character == '=' {
                return true;
            }

            if !is_identifier_character(character) {
                return false;
            }
        }

        false
    }

    fn parse_identifier(&mut self) -> Result<String, MiParseError> {
        let start = self.position;

        while self.peek_char().is_some_and(is_identifier_character) {
            self.next_char();
        }

        if self.position == start {
            return Err(MiParseError::ExpectedIdentifier {
                position: self.position,
            });
        }

        Ok(self.input[start..self.position].to_owned())
    }

    fn parse_bare_const(&mut self) -> Result<MiValue, MiParseError> {
        let start = self.position;

        while let Some(character) = self.peek_char() {
            if matches!(character, ',' | '}' | ']') {
                break;
            }

            self.next_char();
        }

        if self.position == start {
            return Err(MiParseError::ExpectedValue {
                position: self.position,
            });
        }

        Ok(MiValue::Const(self.input[start..self.position].to_owned()))
    }

    fn parse_c_string(&mut self) -> Result<String, MiParseError> {
        self.expect_char('"')?;

        let mut output = String::new();

        loop {
            let Some(character) = self.next_char() else {
                return Err(MiParseError::UnterminatedString);
            };

            match character {
                '"' => break,
                '\\' => {
                    let Some(escaped) = self.next_char() else {
                        return Err(MiParseError::UnterminatedEscape);
                    };

                    self.push_escape(&mut output, escaped)?;
                }
                other => output.push(other),
            }
        }

        Ok(output)
    }

    fn push_escape(&mut self, output: &mut String, escaped: char) -> Result<(), MiParseError> {
        match escaped {
            'a' => output.push('\u{0007}'),
            'b' => output.push('\u{0008}'),
            't' => output.push('\t'),
            'n' => output.push('\n'),
            'v' => output.push('\u{000b}'),
            'f' => output.push('\u{000c}'),
            'r' => output.push('\r'),
            '"' => output.push('"'),
            '\\' => output.push('\\'),
            '0'..='7' => {
                output.push(self.parse_octal_escape(escaped)?);
            }
            other => output.push(other),
        }

        Ok(())
    }

    fn parse_octal_escape(&mut self, first: char) -> Result<char, MiParseError> {
        let mut digits = String::from(first);

        for _ in 0..2 {
            let Some(character) = self.peek_char() else {
                break;
            };

            if !matches!(character, '0'..='7') {
                break;
            }

            digits.push(character);
            self.next_char();
        }

        let value =
            u8::from_str_radix(&digits, 8).map_err(|source| MiParseError::InvalidOctalEscape {
                value: digits.clone(),
                source,
            })?;

        Ok(char::from(value))
    }

    fn expect_char(&mut self, expected: char) -> Result<(), MiParseError> {
        match self.next_char() {
            Some(actual) if actual == expected => Ok(()),
            Some(actual) => Err(MiParseError::UnexpectedCharacter {
                expected,
                actual,
                position: self.position.saturating_sub(actual.len_utf8()),
            }),
            None => Err(MiParseError::UnexpectedEndOfInput),
        }
    }

    fn consume_char(&mut self, expected: char) -> bool {
        if self.peek_char() != Some(expected) {
            return false;
        }

        self.next_char();
        true
    }

    fn peek_char(&self) -> Option<char> {
        self.input[self.position..].chars().next()
    }

    fn next_char(&mut self) -> Option<char> {
        let character = self.peek_char()?;
        self.position += character.len_utf8();
        Some(character)
    }

    fn expect_end(&self) -> Result<(), MiParseError> {
        if self.position == self.input.len() {
            return Ok(());
        }

        Err(MiParseError::TrailingInput(
            self.input[self.position..].to_owned(),
        ))
    }
}

const fn is_identifier_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
}

/// Error produced while parsing GDB/MI output.
#[derive(Debug, Error)]
pub enum MiParseError {
    #[error("GDB/MI record does not contain a record prefix")]
    MissingRecordPrefix,

    #[error("unknown GDB/MI record prefix {0:?}")]
    UnknownRecordPrefix(char),

    #[error("unexpected end of GDB/MI input")]
    UnexpectedEndOfInput,

    #[error("expected character {expected:?}, found {actual:?} at byte {position}")]
    UnexpectedCharacter {
        expected: char,
        actual: char,
        position: usize,
    },

    #[error("expected an identifier at byte {position}")]
    ExpectedIdentifier { position: usize },

    #[error("expected a value at byte {position}")]
    ExpectedValue { position: usize },

    #[error("unterminated GDB/MI string")]
    UnterminatedString,

    #[error("unterminated GDB/MI escape sequence")]
    UnterminatedEscape,

    #[error("invalid GDB/MI token {value:?}")]
    InvalidToken {
        value: String,

        #[source]
        source: std::num::ParseIntError,
    },

    #[error("invalid octal escape {value:?}")]
    InvalidOctalEscape {
        value: String,

        #[source]
        source: std::num::ParseIntError,
    },

    #[error("unexpected trailing GDB/MI input: {0:?}")]
    TrailingInput(String),
}

#[cfg(test)]
mod tests {
    use crate::{MiAsyncRecord, MiListItem, MiRecord, MiResult, MiResultRecord, MiValue};

    use super::{MiParseError, parse_record};

    #[test]
    fn parses_prompt() {
        assert_eq!(
            parse_record("(gdb)").expect("prompt should parse"),
            MiRecord::Prompt
        );
    }

    #[test]
    fn parses_empty_line() {
        assert_eq!(
            parse_record("").expect("empty line should parse"),
            MiRecord::Empty
        );
    }

    #[test]
    fn parses_done_result() {
        assert_eq!(
            parse_record("12^done").expect("result should parse"),
            MiRecord::Result(MiResultRecord {
                token: Some(12),
                class: "done".to_owned(),
                results: Vec::new(),
            })
        );
    }

    #[test]
    fn parses_error_result() {
        assert_eq!(
            parse_record(r#"7^error,msg="No registers.\n""#).expect("error result should parse"),
            MiRecord::Result(MiResultRecord {
                token: Some(7),
                class: "error".to_owned(),
                results: vec![MiResult::new(
                    "msg",
                    MiValue::Const("No registers.\n".to_owned()),
                )],
            })
        );
    }

    #[test]
    fn parses_connected_result_without_token() {
        assert_eq!(
            parse_record("^connected").expect("connected result should parse"),
            MiRecord::Result(MiResultRecord {
                token: None,
                class: "connected".to_owned(),
                results: Vec::new(),
            })
        );
    }

    #[test]
    fn parses_exec_async_record() {
        assert_eq!(
            parse_record(r#"*stopped,reason="breakpoint-hit",thread-id="1""#)
                .expect("async record should parse"),
            MiRecord::ExecAsync(MiAsyncRecord {
                token: None,
                class: "stopped".to_owned(),
                results: vec![
                    MiResult::new("reason", MiValue::Const("breakpoint-hit".to_owned()),),
                    MiResult::new("thread-id", MiValue::Const("1".to_owned()),),
                ],
            })
        );
    }

    #[test]
    fn parses_console_stream() {
        assert_eq!(
            parse_record(r#"~"GNU gdb 6.8 qnx-nto\n""#).expect("console stream should parse"),
            MiRecord::ConsoleStream("GNU gdb 6.8 qnx-nto\n".to_owned())
        );
    }

    #[test]
    fn parses_log_stream() {
        assert_eq!(
            parse_record(r#"&"No registers.\n""#).expect("log stream should parse"),
            MiRecord::LogStream("No registers.\n".to_owned())
        );
    }

    #[test]
    fn parses_target_stream() {
        assert_eq!(
            parse_record(r#"@"hello from target\n""#).expect("target stream should parse"),
            MiRecord::TargetStream("hello from target\n".to_owned())
        );
    }

    #[test]
    fn parses_tuple() {
        assert_eq!(
            parse_record(r#"1^done,bkpt={number="1",type="breakpoint",enabled="y"}"#)
                .expect("tuple should parse"),
            MiRecord::Result(MiResultRecord {
                token: Some(1),
                class: "done".to_owned(),
                results: vec![MiResult::new(
                    "bkpt",
                    MiValue::Tuple(vec![
                        MiResult::new("number", MiValue::Const("1".to_owned()),),
                        MiResult::new("type", MiValue::Const("breakpoint".to_owned()),),
                        MiResult::new("enabled", MiValue::Const("y".to_owned()),),
                    ]),
                )],
            })
        );
    }

    #[test]
    fn parses_list_of_values() {
        assert_eq!(
            parse_record(r#"1^done,values=["one","two","three"]"#).expect("list should parse"),
            MiRecord::Result(MiResultRecord {
                token: Some(1),
                class: "done".to_owned(),
                results: vec![MiResult::new(
                    "values",
                    MiValue::List(vec![
                        MiListItem::Value(MiValue::Const("one".to_owned())),
                        MiListItem::Value(MiValue::Const("two".to_owned())),
                        MiListItem::Value(MiValue::Const("three".to_owned())),
                    ]),
                )],
            })
        );
    }

    #[test]
    fn parses_list_of_results() {
        assert_eq!(
            parse_record(r#"1^done,stack=[frame={level="0"},frame={level="1"}]"#)
                .expect("result list should parse"),
            MiRecord::Result(MiResultRecord {
                token: Some(1),
                class: "done".to_owned(),
                results: vec![MiResult::new(
                    "stack",
                    MiValue::List(vec![
                        MiListItem::Result(MiResult::new(
                            "frame",
                            MiValue::Tuple(vec![MiResult::new(
                                "level",
                                MiValue::Const("0".to_owned()),
                            )]),
                        )),
                        MiListItem::Result(MiResult::new(
                            "frame",
                            MiValue::Tuple(vec![MiResult::new(
                                "level",
                                MiValue::Const("1".to_owned()),
                            )]),
                        )),
                    ]),
                )],
            })
        );
    }

    #[test]
    fn parses_octal_escape() {
        assert_eq!(
            parse_record(r#"~"\101\102\103""#).expect("octal escapes should parse"),
            MiRecord::ConsoleStream("ABC".to_owned())
        );
    }

    #[test]
    fn rejects_unknown_prefix() {
        let error = parse_record("%invalid").expect_err("unknown prefix should fail");

        assert!(matches!(error, MiParseError::UnknownRecordPrefix('%')));
    }

    #[test]
    fn rejects_unterminated_string() {
        let error = parse_record(r#"~"unfinished"#).expect_err("unterminated string should fail");

        assert!(matches!(error, MiParseError::UnterminatedString));
    }
}
