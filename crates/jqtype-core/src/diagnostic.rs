use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Severity {
    Warning,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceSpan {
    pub start: usize,
    pub end: usize,
}

impl SourceSpan {
    pub fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    pub fn from_range(range: core::ops::Range<usize>) -> Self {
        Self {
            start: range.start,
            end: range.end,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub severity: Severity,
    pub message: String,
    pub span: Option<SourceSpan>,
    pub source_name: Option<String>,
}

impl Diagnostic {
    pub fn warning(message: impl Into<String>, span: Option<SourceSpan>) -> Self {
        Self {
            severity: Severity::Warning,
            message: message.into(),
            span,
            source_name: None,
        }
    }

    pub fn error(message: impl Into<String>, span: Option<SourceSpan>) -> Self {
        Self {
            severity: Severity::Error,
            message: message.into(),
            span,
            source_name: None,
        }
    }

    pub fn with_source_name(mut self, source_name: Option<String>) -> Self {
        self.source_name = source_name;
        self
    }
}
