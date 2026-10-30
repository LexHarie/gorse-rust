// Ports: cmd/gorse-bench/main.go (the tables it prints, through github.com/olekukonko/tablewriter v1.1.4) @ add21b5

//! The tables `gorse-bench` prints, drawn as tablewriter's default renderer
//! draws them: a box of box-drawing characters, the header centred and the
//! cells left aligned with one space of padding.
//!
//! The header of each column is `tw.Title(strings.Join(tw.SplitCamelCase(cell),
//! " "))`, so `#interactions` becomes `# INTERACTIONS` and `NDCG@10` becomes
//! `NDCG @ 10`. The expected output in the tests is the output of the pinned
//! tablewriter, not a guess at it.

/// The horizontal positions of the tablewriter table lines.
#[derive(Clone, Copy)]
enum Align {
    /// A header cell: centred, with the extra space on the right.
    Centre,
    /// A body cell: one space of padding, then the text.
    Left,
}

/// Renders a table with `header` and `rows`, as tablewriter's `Render` does. A
/// row has one cell per header column. A table with no rows has no separator
/// under its header. The output ends with a newline.
pub fn render(header: &[&str], rows: &[Vec<String>]) -> String {
    let titles: Vec<String> = header.iter().map(|cell| header_title(cell)).collect();
    let widths: Vec<usize> = titles
        .iter()
        .enumerate()
        .map(|(col, title)| {
            let body = rows.iter().map(|row| width(&row[col])).max().unwrap_or(0);
            width(title).max(body) + 2
        })
        .collect();

    let mut out = rule('┌', '┬', '┐', &widths);
    out.push_str(&line(&titles, &widths, Align::Centre));
    if !rows.is_empty() {
        out.push_str(&rule('├', '┼', '┤', &widths));
        for row in rows {
            out.push_str(&line(row, &widths, Align::Left));
        }
    }
    out.push_str(&rule('└', '┴', '┘', &widths));
    out
}

/// A horizontal line of the table, from `left` through `mid` to `right`.
fn rule(left: char, mid: char, right: char, widths: &[usize]) -> String {
    let segments: Vec<String> = widths.iter().map(|&w| "─".repeat(w)).collect();
    format!("{left}{}{right}\n", segments.join(&mid.to_string()))
}

/// A row of the table: one padded cell per column between the vertical lines.
fn line(cells: &[String], widths: &[usize], align: Align) -> String {
    let cells: Vec<String> = cells
        .iter()
        .zip(widths)
        .map(|(cell, &w)| pad(cell, w, align))
        .collect();
    format!("│{}│\n", cells.join("│"))
}

/// Pads `cell` to the column width `w`. A centred cell gets the extra space on
/// the right, when the gap is odd.
fn pad(cell: &str, w: usize, align: Align) -> String {
    let len = width(cell);
    match align {
        Align::Centre => {
            let gap = (w - 2) - len;
            let left = 1 + gap / 2;
            let right = w - len - left;
            format!("{}{cell}{}", " ".repeat(left), " ".repeat(right))
        }
        Align::Left => format!(" {cell}{}", " ".repeat(w - 1 - len)),
    }
}

/// The display width of a cell. The bench's cells are ASCII names and numbers,
/// so a character count is the display width.
fn width(text: &str) -> usize {
    text.chars().count()
}

/// tablewriter's header text: the words of `cell` in upper case.
fn header_title(cell: &str) -> String {
    split_camel_case(cell).join(" ").to_uppercase()
}

/// The character classes `tw.SplitCamelCase` tells apart.
#[derive(Clone, Copy, PartialEq)]
enum Class {
    Upper,
    Lower,
    Digit,
    Other,
}

fn class_of(c: char) -> Class {
    if c.is_uppercase() {
        Class::Upper
    } else if c.is_lowercase() {
        Class::Lower
    } else if c.is_numeric() {
        Class::Digit
    } else {
        Class::Other
    }
}

/// Splits `text` into its words, as `tw.SplitCamelCase` does. A word starts at
/// a change of character class, except from an upper-case letter to a lower-case
/// one (`Embedding` stays whole), and at an upper-case letter that is followed
/// by a lower-case one (`HTTPServer` splits before `Server`). A run of spaces or
/// underscores on its own is dropped, and any other run is kept whole.
fn split_camel_case(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let classes: Vec<Class> = chars.iter().map(|&c| class_of(c)).collect();
    let mut runs: Vec<(Class, String)> = Vec::new();
    for (i, &c) in chars.iter().enumerate() {
        let class = classes[i];
        let starts_run = match i {
            0 => true,
            _ => {
                let prev = classes[i - 1];
                if prev == class {
                    class == Class::Upper && classes.get(i + 1) == Some(&Class::Lower)
                } else {
                    !(prev == Class::Upper && class == Class::Lower)
                }
            }
        };
        match runs.last_mut() {
            Some((_, run)) if !starts_run => run.push(c),
            _ => runs.push((class, c.to_string())),
        }
    }
    runs.into_iter()
        .filter(|(class, run)| {
            *class != Class::Other || run.chars().any(|c| !c.is_whitespace() && c != '_')
        })
        .map(|(_, run)| run)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The output of tablewriter v1.1.4 for the bench's first table, rendered
    /// by the pinned Go module.
    const INTERACTIONS: &str = "\
┌───────┬────────────────┬────────────┬────────────┐
│       │ # INTERACTIONS │ # POSITIVE │ # NEGATIVE │
├───────┼────────────────┼────────────┼────────────┤
│ total │ 1000           │ 120        │ 880        │
│ train │ 800            │ 96         │ 704        │
│ test  │ 200            │ 24         │ 176        │
└───────┴────────────────┴────────────┴────────────┘
";

    const GAUC: &str = "\
┌─────────────────┬────────┐
│     RANKER      │  GAUC  │
├─────────────────┼────────┤
│ AFM             │ 0.5123 │
│ bge-reranker-v2 │ 0.6000 │
└─────────────────┴────────┘
";

    const EMBEDDING: &str = "\
┌─────────────────────────────┬───────────┬────────────────┬─────────────┐
│       EMBEDDING MODEL       │ NDCG @ 10 │ PRECISION @ 10 │ RECALL @ 10 │
├─────────────────────────────┼───────────┼────────────────┼─────────────┤
│ text-embedding-3-small (16) │ 0.1234    │ 0.0500         │ 0.2000      │
└─────────────────────────────┴───────────┴────────────────┴─────────────┘
";

    const HEADER_ONLY: &str = "\
┌────────┬──────┐
│ RANKER │ GAUC │
└────────┴──────┘
";

    const USERS: &str = "\
┌───────┬──────────┬─────────┬────────────────┐
│       │ # USERS  │ # ITEMS │ # INTERACTIONS │
├───────┼──────────┼─────────┼────────────────┤
│ total │ 12345678 │ 9       │ 1              │
└───────┴──────────┴─────────┴────────────────┘
";

    fn rows(cells: &[&[&str]]) -> Vec<Vec<String>> {
        cells
            .iter()
            .map(|row| row.iter().map(|cell| (*cell).to_owned()).collect())
            .collect()
    }

    #[test]
    fn bench_table_renders_the_interactions_table_like_tablewriter() {
        let rows = rows(&[
            &["total", "1000", "120", "880"],
            &["train", "800", "96", "704"],
            &["test", "200", "24", "176"],
        ]);
        let text = render(&["", "#interactions", "#positive", "#negative"], &rows);
        assert_eq!(text, INTERACTIONS);
    }

    #[test]
    fn bench_table_renders_the_ranker_table_like_tablewriter() {
        let rows = rows(&[&["AFM", "0.5123"], &["bge-reranker-v2", "0.6000"]]);
        assert_eq!(render(&["Ranker", "GAUC"], &rows), GAUC);
    }

    #[test]
    fn bench_table_renders_the_embedding_table_like_tablewriter() {
        let rows = rows(&[&["text-embedding-3-small (16)", "0.1234", "0.0500", "0.2000"]]);
        let text = render(
            &["Embedding Model", "NDCG@10", "Precision@10", "Recall@10"],
            &rows,
        );
        assert_eq!(text, EMBEDDING);
    }

    #[test]
    fn bench_table_without_rows_has_no_separator() {
        assert_eq!(render(&["Ranker", "GAUC"], &[]), HEADER_ONLY);
    }

    #[test]
    fn bench_table_centres_the_header_with_the_odd_space_on_the_right() {
        let rows = rows(&[&["total", "12345678", "9", "1"]]);
        let text = render(&["", "#users", "#items", "#interactions"], &rows);
        assert_eq!(text, USERS);
    }

    #[test]
    fn bench_table_splits_headers_as_tablewriter_does() {
        // Each case is `tw.Title(strings.Join(tw.SplitCamelCase(cell), " "))`
        // from the pinned tablewriter.
        let cases = [
            ("", ""),
            ("#interactions", "# INTERACTIONS"),
            ("#users", "# USERS"),
            ("Ranker", "RANKER"),
            ("GAUC", "GAUC"),
            ("Embedding Model", "EMBEDDING MODEL"),
            ("NDCG@10", "NDCG @ 10"),
            ("Precision@10", "PRECISION @ 10"),
            ("NDCG @ 10", "NDCG  @  10"),
            ("embedding_model", "EMBEDDING MODEL"),
            ("a.b_c", "A . B C"),
            ("HTTPServer", "HTTP SERVER"),
            ("ID2Name", "ID 2 NAME"),
            ("x 1y", "X 1 Y"),
            ("Ab Cd", "AB CD"),
            ("ÉcoleA", "ÉCOLE A"),
            ("a1B2", "A 1 B 2"),
            ("--x", "-- X"),
            ("  lead", "LEAD"),
        ];
        for (cell, title) in cases {
            assert_eq!(header_title(cell), title, "header of {cell:?}");
        }
    }
}
