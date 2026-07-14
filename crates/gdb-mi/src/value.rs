use std::fmt;

/// A value returned by GDB/MI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MiValue {
    /// A quoted or unquoted scalar value.
    Const(String),

    /// A tuple such as `{name="value",child={...}}`.
    Tuple(Vec<MiResult>),

    /// A list containing values or named results.
    List(Vec<MiListItem>),
}

impl MiValue {
    /// Returns the scalar value when this is [`MiValue::Const`].
    #[must_use]
    pub fn as_const(&self) -> Option<&str> {
        match self {
            Self::Const(value) => Some(value),
            Self::Tuple(_) | Self::List(_) => None,
        }
    }

    /// Returns the tuple members when this is [`MiValue::Tuple`].
    #[must_use]
    pub fn as_tuple(&self) -> Option<&[MiResult]> {
        match self {
            Self::Tuple(results) => Some(results),
            Self::Const(_) | Self::List(_) => None,
        }
    }

    /// Returns the list items when this is [`MiValue::List`].
    #[must_use]
    pub fn as_list(&self) -> Option<&[MiListItem]> {
        match self {
            Self::List(items) => Some(items),
            Self::Const(_) | Self::Tuple(_) => None,
        }
    }
}

/// A named result in a GDB/MI record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MiResult {
    /// Result variable name.
    pub variable: String,

    /// Result value.
    pub value: MiValue,
}

impl MiResult {
    /// Creates a named MI result.
    #[must_use]
    pub fn new(variable: impl Into<String>, value: MiValue) -> Self {
        Self {
            variable: variable.into(),
            value,
        }
    }
}

/// One item inside a GDB/MI list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MiListItem {
    /// An unnamed list value.
    Value(MiValue),

    /// A named result.
    Result(MiResult),
}

impl fmt::Display for MiValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Const(value) => write!(formatter, "{value:?}"),
            Self::Tuple(results) => {
                formatter.write_str("{")?;

                for (index, result) in results.iter().enumerate() {
                    if index != 0 {
                        formatter.write_str(",")?;
                    }

                    write!(formatter, "{}={}", result.variable, result.value)?;
                }

                formatter.write_str("}")
            }
            Self::List(items) => {
                formatter.write_str("[")?;

                for (index, item) in items.iter().enumerate() {
                    if index != 0 {
                        formatter.write_str(",")?;
                    }

                    match item {
                        MiListItem::Value(value) => {
                            write!(formatter, "{value}")?;
                        }
                        MiListItem::Result(result) => {
                            write!(formatter, "{}={}", result.variable, result.value)?;
                        }
                    }
                }

                formatter.write_str("]")
            }
        }
    }
}
