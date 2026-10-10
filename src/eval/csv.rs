//! Comma-separated values as described by RFC 4180: fields may be quoted, and
//! quoted fields may hold commas, line breaks and doubled quotes.

pub fn parse(text: &str) -> Result<Vec<Vec<String>>, String> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut chars = text.chars().peekable();
    let mut line = 1;
    let mut at_field_start = true;
    while let Some(c) = chars.next() {
        match c {
            '"' if at_field_start => {
                let start = line;
                loop {
                    match chars.next() {
                        Some('"') if chars.peek() == Some(&'"') => {
                            chars.next();
                            field.push('"');
                        }
                        Some('"') => break,
                        Some(c) => {
                            line += (c == '\n') as usize;
                            field.push(c);
                        }
                        None => {
                            return Err(format!(
                                "csv: the quoted field starting on line {start} is never closed"
                            ));
                        }
                    }
                }
                if !matches!(chars.peek(), None | Some(',' | '\n' | '\r')) {
                    return Err(format!(
                        "csv: unexpected text after a quoted field on line {line}"
                    ));
                }
                at_field_start = false;
            }
            ',' => {
                row.push(std::mem::take(&mut field));
                at_field_start = true;
            }
            '\r' if chars.peek() == Some(&'\n') => {}
            '\n' => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
                line += 1;
                at_field_start = true;
            }
            c => {
                field.push(c);
                at_field_start = false;
            }
        }
    }
    if !field.is_empty() || !row.is_empty() || !at_field_start {
        row.push(field);
        rows.push(row);
    }
    Ok(rows)
}

pub fn format(rows: &[Vec<String>]) -> String {
    let mut out = String::new();
    for row in rows {
        for (i, field) in row.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            if field.contains([',', '"', '\n', '\r']) {
                out.push('"');
                out.push_str(&field.replace('"', "\"\""));
                out.push('"');
            } else {
                out.push_str(field);
            }
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_quoted_fields() {
        let rows = vec![
            vec!["a".to_string(), "b,c".into(), "say \"hi\"".into()],
            vec!["line\nbreak".into(), "".into(), "x".into()],
        ];
        assert_eq!(parse(&format(&rows)).unwrap(), rows);
    }

    #[test]
    fn handles_crlf_and_missing_final_newline() {
        assert_eq!(
            parse("a,b\r\n1,2").unwrap(),
            vec![vec!["a", "b"], vec!["1", "2"]]
        );
        assert_eq!(parse("a,\n").unwrap(), vec![vec!["a", ""]]);
        assert!(parse("\"open").is_err());
    }
}
