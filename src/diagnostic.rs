use crate::source::{SourceMap, Span};
use std::fmt::Write;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
}

#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub severity: Severity,
    pub message: String,
    pub span: Span,
    pub label: Option<String>,
    pub help: Option<String>,
    pub notes: Vec<String>,
}

impl Diagnostic {
    pub fn error(span: Span, message: impl Into<String>) -> Self {
        Diagnostic {
            severity: Severity::Error,
            message: message.into(),
            span,
            label: None,
            help: None,
            notes: Vec::new(),
        }
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }

    pub fn render(&self, sources: &SourceMap, color: bool) -> String {
        let (kind, tint) = match self.severity {
            Severity::Error => ("error", "\x1b[1;31m"),
        };
        let paint = |text: &str, code: &str| {
            if color {
                format!("{code}{text}\x1b[0m")
            } else {
                text.to_string()
            }
        };
        let mut out = String::new();
        let _ = writeln!(
            out,
            "{}{} {}",
            paint(kind, tint),
            paint(":", "\x1b[1m"),
            paint(&self.message, "\x1b[1m")
        );

        let Some(file) = sources.try_get(self.span.file) else {
            if let Some(help) = &self.help {
                let _ = writeln!(
                    out,
                    "  {} {}: {help}",
                    paint("=", "\x1b[1;34m"),
                    paint("help", "\x1b[1m")
                );
            }
            for note in &self.notes {
                let _ = writeln!(
                    out,
                    "  {} {}: {note}",
                    paint("=", "\x1b[1;34m"),
                    paint("note", "\x1b[1m")
                );
            }
            return out;
        };
        let (line, col) = file.line_col(self.span.lo as usize);
        let (end_line, end_col) = file.line_col(self.span.hi as usize);
        let gutter = line.to_string().len();
        let pad = " ".repeat(gutter);
        let _ = writeln!(
            out,
            "{pad}{} {}:{line}:{col}",
            paint("-->", "\x1b[1;34m"),
            file.path.display()
        );
        let _ = writeln!(out, "{pad} {}", paint("|", "\x1b[1;34m"));

        let text = file.line_text(line);
        let _ = writeln!(
            out,
            "{} {} {text}",
            paint(&line.to_string(), "\x1b[1;34m"),
            paint("|", "\x1b[1;34m")
        );
        let width = if end_line == line {
            end_col.saturating_sub(col).max(1)
        } else {
            text.chars().count().saturating_sub(col - 1).max(1)
        };
        let marker = format!("{}{}", " ".repeat(col - 1), "^".repeat(width));
        let label = self
            .label
            .as_deref()
            .map(|l| format!(" {l}"))
            .unwrap_or_default();
        let _ = writeln!(
            out,
            "{pad} {} {}",
            paint("|", "\x1b[1;34m"),
            paint(&format!("{marker}{label}"), tint)
        );

        if let Some(help) = &self.help {
            let _ = writeln!(
                out,
                "{pad} {} {}: {help}",
                paint("=", "\x1b[1;34m"),
                paint("help", "\x1b[1m")
            );
        }
        for note in &self.notes {
            let _ = writeln!(
                out,
                "{pad} {} {}: {note}",
                paint("=", "\x1b[1;34m"),
                paint("note", "\x1b[1m")
            );
        }
        out
    }

    pub fn to_json(&self, sources: &SourceMap) -> String {
        let (path, line, col, end_line, end_col) = match sources.try_get(self.span.file) {
            Some(file) => {
                let (line, col) = file.line_col(self.span.lo as usize);
                let (end_line, end_col) = file.line_col(self.span.hi as usize);
                (
                    json_string(&file.path.display().to_string()),
                    line,
                    col,
                    end_line,
                    end_col,
                )
            }
            None => ("null".into(), 0, 0, 0, 0),
        };
        let severity = match self.severity {
            Severity::Error => "error",
        };
        let opt = |s: &Option<String>| {
            s.as_deref()
                .map(json_string)
                .unwrap_or_else(|| "null".into())
        };
        format!(
            "{{\"severity\":\"{severity}\",\"message\":{},\"file\":{},\"line\":{line},\"column\":{col},\"end_line\":{end_line},\"end_column\":{end_col},\"label\":{},\"help\":{}}}",
            json_string(&self.message),
            path,
            opt(&self.label),
            opt(&self.help),
        )
    }
}

pub fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
