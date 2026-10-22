//! A small RFC 4180 parser for the CSV / TSV preview.

/// Parse delimited text into rows of cells, keeping at most `max_rows` rows and
/// `max_cols` columns. The flag says whether anything was dropped.
pub fn parse_delimited(
    text: &str,
    delimiter: char,
    max_rows: usize,
    max_cols: usize,
) -> (Vec<Vec<String>>, bool) {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut cell = String::new();
    let mut in_quotes = false;
    let mut cut = false;
    let mut chars = text.chars().peekable();

    let mut finish_row = |row: &mut Vec<String>, cell: &mut String, rows: &mut Vec<Vec<String>>| {
        row.push(std::mem::take(cell));
        // A blank line is not a one-cell row.
        if !(row.len() == 1 && row[0].is_empty()) {
            if row.len() > max_cols {
                row.truncate(max_cols);
                cut = true;
            }
            rows.push(std::mem::take(row));
        } else {
            row.clear();
        }
    };

    while let Some(c) = chars.next() {
        if in_quotes {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    cell.push('"');
                    chars.next();
                }
                '"' => in_quotes = false,
                other => cell.push(other),
            }
            continue;
        }
        match c {
            '"' if cell.is_empty() => in_quotes = true,
            c if c == delimiter => row.push(std::mem::take(&mut cell)),
            '\r' => {}
            '\n' => {
                finish_row(&mut row, &mut cell, &mut rows);
                if rows.len() >= max_rows {
                    return (rows, true);
                }
            }
            other => cell.push(other),
        }
    }
    if !cell.is_empty() || !row.is_empty() {
        finish_row(&mut row, &mut cell, &mut rows);
    }
    (rows, cut)
}

/// Delimiter for a file extension.
pub fn delimiter_for(ext: &str) -> char {
    if ext == "tsv" { '\t' } else { ',' }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Vec<Vec<String>> {
        parse_delimited(text, ',', 100, 20).0
    }

    #[test]
    fn splits_rows_and_cells() {
        assert_eq!(parse("a,b,c\n1,2,3\n"), [["a", "b", "c"], ["1", "2", "3"]]);
    }

    #[test]
    fn quoted_cells_keep_commas_newlines_and_escaped_quotes() {
        let rows = parse("name,note\n\"Smith, J\",\"line1\nline2\"\n\"say \"\"hi\"\"\",x\n");
        assert_eq!(rows[1], ["Smith, J", "line1\nline2"]);
        assert_eq!(rows[2], ["say \"hi\"", "x"]);
    }

    #[test]
    fn crlf_line_endings_and_a_missing_final_newline_work() {
        assert_eq!(parse("a,b\r\nc,d"), [["a", "b"], ["c", "d"]]);
    }

    #[test]
    fn blank_lines_are_skipped_but_empty_cells_are_kept() {
        assert_eq!(parse("a,,c\n\n1,2,3\n"), [["a", "", "c"], ["1", "2", "3"]]);
    }

    #[test]
    fn tabs_split_tsv() {
        let (rows, _) = parse_delimited("a\tb\n1\t2\n", '\t', 10, 10);
        assert_eq!(rows, [["a", "b"], ["1", "2"]]);
        assert_eq!(delimiter_for("tsv"), '\t');
        assert_eq!(delimiter_for("csv"), ',');
    }

    #[test]
    fn row_and_column_caps_report_truncation() {
        let (rows, cut) = parse_delimited("a,b,c\n1,2,3\n4,5,6\n", ',', 2, 10);
        assert_eq!(rows.len(), 2);
        assert!(cut);
        let (rows, cut) = parse_delimited("a,b,c\n", ',', 10, 2);
        assert_eq!(rows[0], ["a", "b"]);
        assert!(cut);
        assert!(!parse_delimited("a,b\n", ',', 10, 10).1);
    }

    #[test]
    fn empty_input_has_no_rows() {
        assert!(parse("").is_empty());
    }
}
