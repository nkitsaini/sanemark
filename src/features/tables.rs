//! GFM table formatting.
//!
//! This is intentionally *forgiving*: any run of consecutive "table-like" lines
//! (lines that contain pipes and aren't headings/list items/etc.) is normalised
//! into a well-formed, column-aligned GFM table. In particular:
//!
//! * a lone header row (`a | b | c`) gains a separator row **and** one empty
//!   body row, plus the surrounding pipes;
//! * columns are padded so the pipes line up;
//! * a trailing line that is just a stray `|` (a half-typed new row) becomes a
//!   full empty row;
//! * extra cells typed onto the header become new columns (the separator and
//!   body rows grow to match).
//!
//! The formatter rewrites whole lines, so it changes line counts freely; callers
//! apply the result as a full-document edit.

/// Column alignment, derived from the separator row's colons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Align {
    None,
    Left,
    Right,
    Center,
}

/// Format every table-like block in `source`, leaving all other text verbatim.
pub fn format_tables(source: &str) -> String {
    let crlf = source.contains("\r\n");
    let lines: Vec<String> = source
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l).to_string())
        .collect();

    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut i = 0;
    let n = lines.len();
    let mut fence: Option<char> = None;

    while i < n {
        let line = &lines[i];
        let trimmed = line.trim_start();

        // Track fenced code blocks — never format inside them.
        if let Some(marker) = fence {
            out.push(line.clone());
            if is_fence(trimmed, marker) {
                fence = None;
            }
            i += 1;
            continue;
        }
        if let Some(marker) = fence_marker(trimmed) {
            fence = Some(marker);
            out.push(line.clone());
            i += 1;
            continue;
        }

        // YAML/TOML front matter at the very top of the document.
        if i == 0 && (line.trim() == "---" || line.trim() == "+++") {
            let close = line.trim().to_string();
            out.push(line.clone());
            i += 1;
            while i < n && lines[i].trim() != close {
                out.push(lines[i].clone());
                i += 1;
            }
            if i < n {
                out.push(lines[i].clone());
                i += 1;
            }
            continue;
        }

        if is_table_row(line) {
            let start = i;
            while i < n && is_table_row(&lines[i]) {
                i += 1;
            }
            let group = &lines[start..i];
            match format_group(group) {
                Some(formatted) => out.extend(formatted),
                None => out.extend(group.iter().cloned()),
            }
            continue;
        }

        out.push(line.clone());
        i += 1;
    }

    out.join(if crlf { "\r\n" } else { "\n" })
}

/// Reformat one contiguous block of table-like lines, or `None` to leave it be.
fn format_group(group: &[String]) -> Option<Vec<String>> {
    let indent = leading_ws(&group[0]);
    let parsed: Vec<Vec<String>> = group.iter().map(|l| parse_row(l)).collect();

    // The separator is the first all-dashes row after the header.
    let sep_idx = parsed
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_, r)| is_separator_row(r))
        .map(|(idx, _)| idx);

    let header = &parsed[0];
    let body: Vec<&Vec<String>> = parsed
        .iter()
        .enumerate()
        .filter(|(idx, _)| *idx != 0 && Some(*idx) != sep_idx)
        .map(|(_, r)| r)
        .collect();

    let mut max_cols = header.len();
    for r in &body {
        max_cols = max_cols.max(r.len());
    }
    if let Some(idx) = sep_idx {
        max_cols = max_cols.max(parsed[idx].len());
    }

    // Only rewrite things that actually look like a table.
    let qualifies = sep_idx.is_some() || max_cols >= 2 || group.len() >= 2;
    if max_cols == 0 || !qualifies {
        return None;
    }

    let aligns: Vec<Align> = {
        let sep = sep_idx.map(|idx| &parsed[idx]);
        (0..max_cols)
            .map(|c| {
                sep.and_then(|s| s.get(c))
                    .map(|cell| cell_align(cell))
                    .unwrap_or(Align::None)
            })
            .collect()
    };

    // Column widths from header + body cells (min 3 so `---` fits).
    let mut widths = vec![3usize; max_cols];
    let mut absorb = |row: &Vec<String>| {
        for (c, w) in widths.iter_mut().enumerate() {
            let len = row.get(c).map(|s| s.chars().count()).unwrap_or(0);
            if len > *w {
                *w = len;
            }
        }
    };
    absorb(header);
    for r in &body {
        absorb(r);
    }

    let mut out = Vec::with_capacity(body.len() + 3);
    out.push(render_row(&indent, header, &widths, &aligns));
    out.push(render_separator(&indent, &widths, &aligns));
    if body.is_empty() {
        out.push(render_row(&indent, &[], &widths, &aligns));
    } else {
        for r in &body {
            out.push(render_row(&indent, r, &widths, &aligns));
        }
    }
    Some(out)
}

fn render_row(indent: &str, cells: &[String], widths: &[usize], aligns: &[Align]) -> String {
    let mut s =
        String::with_capacity(indent.len() + widths.iter().sum::<usize>() + widths.len() * 3 + 1);
    s.push_str(indent);
    s.push('|');
    for c in 0..widths.len() {
        let content = cells.get(c).map(String::as_str).unwrap_or("");
        s.push(' ');
        s.push_str(&pad(content, widths[c], aligns[c]));
        s.push(' ');
        s.push('|');
    }
    s
}

fn render_separator(indent: &str, widths: &[usize], aligns: &[Align]) -> String {
    let mut s = String::new();
    s.push_str(indent);
    s.push('|');
    for c in 0..widths.len() {
        s.push(' ');
        s.push_str(&separator_cell(widths[c], aligns[c]));
        s.push(' ');
        s.push('|');
    }
    s
}

fn pad(content: &str, width: usize, align: Align) -> String {
    let len = content.chars().count();
    if len >= width {
        return content.to_string();
    }
    let total = width - len;
    match align {
        Align::Right => format!("{}{}", " ".repeat(total), content),
        Align::Center => {
            let left = total / 2;
            format!(
                "{}{}{}",
                " ".repeat(left),
                content,
                " ".repeat(total - left)
            )
        }
        Align::None | Align::Left => format!("{}{}", content, " ".repeat(total)),
    }
}

fn separator_cell(width: usize, align: Align) -> String {
    let width = width.max(3);
    match align {
        Align::None => "-".repeat(width),
        Align::Left => format!(":{}", "-".repeat(width - 1)),
        Align::Right => format!("{}:", "-".repeat(width - 1)),
        Align::Center => format!(":{}:", "-".repeat(width - 2)),
    }
}

/// Split a line into trimmed cells, honouring `\|` escapes and dropping the
/// empty cells produced by surrounding pipes.
fn parse_row(line: &str) -> Vec<String> {
    let s = line.trim();
    let mut cells: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut escaped = false;
    for ch in s.chars() {
        if escaped {
            cur.push(ch);
            escaped = false;
            continue;
        }
        match ch {
            '\\' => {
                cur.push(ch);
                escaped = true;
            }
            '|' => {
                cells.push(cur.trim().to_string());
                cur.clear();
            }
            _ => cur.push(ch),
        }
    }
    cells.push(cur.trim().to_string());

    if s.starts_with('|') && cells.first().is_some_and(String::is_empty) {
        cells.remove(0);
    }
    if s.ends_with('|') && cells.last().is_some_and(String::is_empty) {
        cells.pop();
    }
    cells
}

fn is_separator_row(cells: &[String]) -> bool {
    !cells.is_empty() && cells.iter().all(|c| is_separator_cell(c))
}

fn is_separator_cell(cell: &str) -> bool {
    let c = cell.trim();
    if c.is_empty() {
        return false;
    }
    let core = c.trim_start_matches(':').trim_end_matches(':');
    !core.is_empty() && core.chars().all(|ch| ch == '-')
}

fn cell_align(cell: &str) -> Align {
    let c = cell.trim();
    match (c.starts_with(':'), c.ends_with(':')) {
        (true, true) => Align::Center,
        (true, false) => Align::Left,
        (false, true) => Align::Right,
        (false, false) => Align::None,
    }
}

/// Whether `line` should be treated as part of a table.
fn is_table_row(line: &str) -> bool {
    let s = line.trim();
    if s.is_empty() {
        return false;
    }
    // Exclude other block constructs that may legitimately contain pipes.
    if s.starts_with('#') || s.starts_with('>') || is_list_item(s) {
        return false;
    }
    let pipes = count_unescaped_pipes(s);
    if pipes == 0 {
        return false;
    }
    s.starts_with('|') || s.ends_with('|') || pipes >= 2 || is_separator_row(&parse_row(line))
}

fn is_list_item(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.len() >= 2 && matches!(bytes[0], b'-' | b'*' | b'+') && bytes[1] == b' ' {
        return true;
    }
    let digits = s.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 {
        let rest = &s[digits..];
        if rest.starts_with(". ") || rest.starts_with(") ") {
            return true;
        }
    }
    false
}

fn count_unescaped_pipes(s: &str) -> usize {
    let mut count = 0;
    let mut escaped = false;
    for ch in s.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' => escaped = true,
            '|' => count += 1,
            _ => {}
        }
    }
    count
}

fn leading_ws(s: &str) -> String {
    s.chars().take_while(|c| *c == ' ' || *c == '\t').collect()
}

fn fence_marker(trimmed: &str) -> Option<char> {
    if trimmed.starts_with("```") {
        Some('`')
    } else if trimmed.starts_with("~~~") {
        Some('~')
    } else {
        None
    }
}

fn is_fence(trimmed: &str, marker: char) -> bool {
    trimmed.starts_with(&marker.to_string().repeat(3))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_line_becomes_full_table() {
        let out = format_tables("asdf | asdf| asdf| asdf\n");
        assert_eq!(
            out,
            "\
| asdf | asdf | asdf | asdf |
| ---- | ---- | ---- | ---- |
|      |      |      |      |
"
        );
    }

    #[test]
    fn adds_missing_pipes_and_aligns() {
        let input = "| name | value |\n|-|-|\n| a | 100 |\n| bb | 2 |\n";
        let out = format_tables(input);
        assert_eq!(
            out,
            "\
| name | value |
| ---- | ----- |
| a    | 100   |
| bb   | 2     |
"
        );
    }

    #[test]
    fn stray_pipe_after_table_becomes_empty_row() {
        let input = "| a | b |\n| - | - |\n| 1 | 2 |\n|\n";
        let out = format_tables(input);
        // Columns pad to a minimum width of 3 so the `---` separator is valid.
        assert_eq!(
            out,
            "\
| a   | b   |
| --- | --- |
| 1   | 2   |
|     |     |
"
        );
    }

    #[test]
    fn extra_header_cell_becomes_new_column() {
        let input = "| a | b | c\n| - | - |\n| 1 | 2 |\n";
        let out = format_tables(input);
        assert_eq!(
            out,
            "\
| a   | b   | c   |
| --- | --- | --- |
| 1   | 2   |     |
"
        );
    }

    #[test]
    fn preserves_alignment_markers() {
        let input = "| a | b | c |\n| :- | :-: | -: |\n| 1 | 2 | 3 |\n";
        let out = format_tables(input);
        assert_eq!(
            out,
            "\
| a   |  b  |   c |
| :-- | :-: | --: |
| 1   |  2  |   3 |
"
        );
    }

    #[test]
    fn leaves_non_tables_untouched() {
        let input = "# Heading | with pipe\n\nSome text.\n\n- a | b | c\n";
        assert_eq!(format_tables(input), input);
    }

    #[test]
    fn ignores_pipes_in_code_fence() {
        let input = "```\na | b | c\n```\n";
        assert_eq!(format_tables(input), input);
    }

    #[test]
    fn single_pipe_prose_untouched() {
        let input = "this | that\n";
        assert_eq!(format_tables(input), input);
    }
}
