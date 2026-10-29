//! Moved-line detection: blocks of removed lines that reappear as
//! added lines elsewhere in the same diff, in the same file or another
//! one. Follows `git diff --color-moved=zebra
//! --color-moved-ws=allow-indentation-change`:
//!
//! - lines match on their content after the leading whitespace,
//! - every line of a block shifts its indentation by the same amount,
//! - a block needs at least [`MIN_ALNUM`] letters and digits, so short
//!   repeated lines such as `}` never count,
//! - unlike git, a block also needs at least [`MIN_LINES`] lines, so a
//!   single import that moves between files doesn't count,
//! - a block that directly follows another moved block on the same
//!   side is flagged `alt` so the two read as separate moves.
//!
//! Matching is greedy: each removed line starts at most one block, at
//! the longest run of added lines it matches. A block that occurs
//! more than once can therefore pair differently than git would.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::diff::{FileChange, Hunk, LineOrigin};
use crate::ids::{LineRange, Side};

/// Fewest letters and digits a block needs to count as moved. Same
/// threshold as git's `COLOR_MOVED_MIN_ALNUM_COUNT`.
pub const MIN_ALNUM: usize = 20;

/// Fewest lines a block needs to count as moved. Git has no such
/// minimum and marks single lines, which flags every import that
/// moves between files.
pub const MIN_LINES: usize = 2;

/// Diffs with more changed lines than this skip detection. Keeps the
/// review-open path fast on huge diffs.
pub const MAX_CHANGED_LINES: usize = 50_000;

/// Added lines considered per removed line. Bounds the work for keys
/// that repeat many times.
const MAX_CANDIDATES: usize = 64;

/// One end of a moved block in a file of the diff.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MovedLines {
    /// `base` for the removed end of the move, `tip` for the added end.
    pub side: Side,
    pub lines: LineRange,
    /// The block directly follows another moved block on this side.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub alt: bool,
    /// Where the block went (for `base`) or came from (for `tip`).
    pub other: MovedEnd,
}

/// The opposite end of a moved block.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MovedEnd {
    /// `FileChange::path` of the file holding that end.
    pub path: String,
    pub lines: LineRange,
}

/// Consecutive changed lines on one side of one file: removed lines
/// (`Side::Base`) or added lines (`Side::Tip`). `start` is the 1-based
/// line number of the first line.
#[derive(Clone, Debug)]
pub struct ChangedRun {
    pub file: usize,
    pub side: Side,
    pub start: u32,
    pub lines: Vec<String>,
}

/// Split a file's hunks into removed and added runs.
pub fn runs_from_hunks(file: usize, hunks: &[Hunk]) -> Vec<ChangedRun> {
    let mut runs: Vec<ChangedRun> = Vec::new();
    for hunk in hunks {
        let Hunk::Regular(h) = hunk else { continue };
        let mut prev: Option<(Side, u32)> = None;
        for line in &h.lines {
            let (side, n) = match (line.origin, line.base_line, line.tip_line) {
                (LineOrigin::Removed, Some(n), _) => (Side::Base, n),
                (LineOrigin::Added, _, Some(n)) => (Side::Tip, n),
                _ => {
                    prev = None;
                    continue;
                }
            };
            let continues = matches!(prev, Some((s, p)) if s == side && p + 1 == n);
            if !continues {
                runs.push(ChangedRun { file, side, start: n, lines: Vec::new() });
            }
            runs.last_mut().expect("run pushed above").lines.push(line.content.clone());
            prev = Some((side, n));
        }
    }
    runs
}

/// Set `moved` on every file from the changed runs of the whole diff.
/// Returns false when the diff has more than [`MAX_CHANGED_LINES`]
/// changed lines and detection was skipped.
pub fn detect_moves(files: &mut [FileChange], runs: &[ChangedRun]) -> bool {
    detect_moves_up_to(files, runs, MAX_CHANGED_LINES)
}

/// [`detect_moves`] with an explicit changed-line limit.
pub fn detect_moves_up_to(files: &mut [FileChange], runs: &[ChangedRun], limit: usize) -> bool {
    for f in files.iter_mut() {
        f.moved.clear();
    }
    if runs.iter().map(|r| r.lines.len()).sum::<usize>() > limit {
        return false;
    }

    let parsed: Vec<Vec<Line<'_>>> = runs
        .iter()
        .map(|r| r.lines.iter().map(|l| Line::new(l)).collect())
        .collect();
    let removed: Vec<usize> = (0..runs.len()).filter(|&i| runs[i].side == Side::Base).collect();

    // Added lines by content, in diff order.
    let mut index: HashMap<&str, Vec<(usize, usize)>> = HashMap::new();
    for (ri, run) in runs.iter().enumerate() {
        if run.side != Side::Tip {
            continue;
        }
        for (li, line) in parsed[ri].iter().enumerate() {
            if !line.blank() {
                index.entry(line.key).or_default().push((ri, li));
            }
        }
    }

    let mut used: Vec<Vec<bool>> = runs.iter().map(|r| vec![false; r.lines.len()]).collect();
    let mut blocks: Vec<Block> = Vec::new();

    for &r in &removed {
        let mut i = 0;
        while i < parsed[r].len() {
            if used[r][i] || parsed[r][i].blank() {
                i += 1;
                continue;
            }
            let mut best: Option<(usize, usize, usize)> = None;
            let candidates = index.get(parsed[r][i].key).map(Vec::as_slice).unwrap_or(&[]);
            for &(a, j) in candidates.iter().take(MAX_CANDIDATES) {
                if used[a][j] {
                    continue;
                }
                let len = match_len(&parsed[r], &used[r], i, &parsed[a], &used[a], j);
                if len > 0 && best.is_none_or(|(l, _, _)| len > l) {
                    best = Some((len, a, j));
                }
            }
            let Some((len, a, j)) = best else {
                i += 1;
                continue;
            };
            let alnum: usize = parsed[r][i..i + len].iter().map(|l| l.alnum).sum();
            if len < MIN_LINES || alnum < MIN_ALNUM {
                i += 1;
                continue;
            }
            for k in 0..len {
                used[r][i + k] = true;
                used[a][j + k] = true;
            }
            blocks.push(Block {
                base: (runs[r].file, range(runs[r].start + i as u32, len)),
                tip: (runs[a].file, range(runs[a].start + j as u32, len)),
            });
            i += len;
        }
    }

    for b in &blocks {
        let (bf, bl) = b.base;
        let (tf, tl) = b.tip;
        let tip_path = files[tf].path.clone();
        let base_path = files[bf].path.clone();
        files[bf].moved.push(MovedLines {
            side: Side::Base,
            lines: bl,
            alt: false,
            other: MovedEnd { path: tip_path, lines: tl },
        });
        files[tf].moved.push(MovedLines {
            side: Side::Tip,
            lines: tl,
            alt: false,
            other: MovedEnd { path: base_path, lines: bl },
        });
    }
    for f in files.iter_mut() {
        f.moved.sort_by_key(|m| (m.side == Side::Tip, m.lines.start));
        // Zebra: a block that starts right after the previous one on
        // the same side takes the other colour.
        for k in 1..f.moved.len() {
            let (prev, cur) = (&f.moved[k - 1], &f.moved[k]);
            let touches = prev.side == cur.side && prev.lines.end + 1 == cur.lines.start;
            let alt = touches && !prev.alt;
            f.moved[k].alt = alt;
        }
    }
    true
}

struct Block {
    base: (usize, LineRange),
    tip: (usize, LineRange),
}

fn range(start: u32, len: usize) -> LineRange {
    LineRange { start, end: start + len as u32 - 1 }
}

/// A changed line split for matching.
struct Line<'a> {
    /// Leading spaces and tabs.
    indent: &'a str,
    /// The rest of the line without its terminator.
    key: &'a str,
    alnum: usize,
}

impl<'a> Line<'a> {
    fn new(raw: &'a str) -> Self {
        let text = raw.strip_suffix('\n').unwrap_or(raw);
        let text = text.strip_suffix('\r').unwrap_or(text);
        let key = text.trim_start_matches([' ', '\t']);
        let indent = &text[..text.len() - key.len()];
        let alnum = key.chars().filter(|c| c.is_alphanumeric()).count();
        Self { indent, key, alnum }
    }

    fn blank(&self) -> bool {
        self.key.trim_end().is_empty()
    }
}

/// How the indentation changes from a removed line to an added one:
/// the whitespace one side has on top of the other. `None` when
/// neither indent is a prefix of the other.
fn indent_shift<'a>(removed: &'a str, added: &'a str) -> Option<(bool, &'a str)> {
    if let Some(extra) = added.strip_prefix(removed) {
        Some((true, extra))
    } else {
        removed.strip_prefix(added).map(|extra| (false, extra))
    }
}

/// Length of the block that starts at removed line `i` and added line
/// `j`. Lines must match on content with one indentation shift for the
/// whole block. Blank lines match each other and don't constrain the
/// shift, and trailing blank lines are left out of the block.
fn match_len(
    removed: &[Line<'_>],
    removed_used: &[bool],
    i: usize,
    added: &[Line<'_>],
    added_used: &[bool],
    j: usize,
) -> usize {
    let Some(shift) = indent_shift(removed[i].indent, added[j].indent) else {
        return 0;
    };
    let mut len = 0;
    while i + len < removed.len() && j + len < added.len() {
        let (r, a) = (&removed[i + len], &added[j + len]);
        if removed_used[i + len] || added_used[j + len] {
            break;
        }
        let same = if r.blank() || a.blank() {
            r.blank() && a.blank()
        } else {
            r.key == a.key && indent_shift(r.indent, a.indent) == Some(shift)
        };
        if !same {
            break;
        }
        len += 1;
    }
    while len > 0 && removed[i + len - 1].blank() {
        len -= 1;
    }
    len
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::FileStatus;

    fn file(path: &str) -> FileChange {
        FileChange {
            path: path.into(),
            status: FileStatus::Modified,
            hunks: None,
            binary: false,
            added: 0,
            removed: 0,
            moved: Vec::new(),
        }
    }

    fn run(file: usize, side: Side, start: u32, lines: &[&str]) -> ChangedRun {
        ChangedRun {
            file,
            side,
            start,
            lines: lines.iter().map(|l| format!("{l}\n")).collect(),
        }
    }

    fn lr(start: u32, end: u32) -> LineRange {
        LineRange { start, end }
    }

    fn moved(side: Side, lines: LineRange, alt: bool, path: &str, other: LineRange) -> MovedLines {
        MovedLines { side, lines, alt, other: MovedEnd { path: path.into(), lines: other } }
    }

    #[test]
    fn move_to_another_file_and_reindent_into_a_class() {
        let mut files = vec![file("billing.py"), file("discounts.py")];
        let runs = vec![
            run(0, Side::Base, 4, &[
                "def subtotal(items):",
                "    total = 0",
                "    for item in items:",
                "        total += item.price * item.quantity",
                "    return total",
                "",
                "",
                "def apply_discount(total, code):",
                "    if code == \"SAVE10\":",
                "        return total * 0.9",
                "    if code == \"HALF\":",
                "        return total * 0.5",
                "    return total",
                "",
                "",
            ]),
            run(0, Side::Tip, 6, &[
                "",
                "",
                "class Cart:",
                "    def subtotal(self, items):",
                "        total = 0",
                "        for item in items:",
                "            total += item.price * item.quantity",
                "        return total",
            ]),
            run(1, Side::Tip, 1, &[
                "def apply_discount(total, code):",
                "    if code == \"SAVE10\":",
                "        return total * 0.9",
                "    if code == \"HALF\":",
                "        return total * 0.5",
                "    return total",
            ]),
        ];
        assert!(detect_moves(&mut files, &runs));
        assert_eq!(files[0].moved, vec![
            moved(Side::Base, lr(5, 8), false, "billing.py", lr(10, 13)),
            moved(Side::Base, lr(11, 16), false, "discounts.py", lr(1, 6)),
            moved(Side::Tip, lr(10, 13), false, "billing.py", lr(5, 8)),
        ]);
        assert_eq!(files[1].moved, vec![
            moved(Side::Tip, lr(1, 6), false, "billing.py", lr(11, 16)),
        ]);
    }

    #[test]
    fn edited_line_ends_the_block() {
        let mut files = vec![file("app.py")];
        let runs = vec![
            run(0, Side::Base, 1, &[
                "def load(path):",
                "    with open(path) as f:",
                "        data = f.read()",
                "    return parse(data, strict=True)",
                "",
                "",
            ]),
            run(0, Side::Tip, 13, &[
                "",
                "",
                "def load(path):",
                "    with open(path) as f:",
                "        data = f.read()",
                "    return parse(data, strict=False)",
            ]),
        ];
        detect_moves(&mut files, &runs);
        assert_eq!(files[0].moved, vec![
            moved(Side::Base, lr(1, 3), false, "app.py", lr(15, 17)),
            moved(Side::Tip, lr(15, 17), false, "app.py", lr(1, 3)),
        ]);
    }

    #[test]
    fn touching_blocks_alternate_on_their_side() {
        let mut files = vec![file("config.py")];
        let runs = vec![
            run(0, Side::Tip, 1, &[
                "LOG_LEVEL = env(\"LOG_LEVEL\", default=\"info\")",
                "LOG_FORMAT = env(\"LOG_FORMAT\", default=\"text\")",
                "FEATURE_FLAGS = env.list(\"FEATURE_FLAGS\", default=[])",
                "FEATURE_DEFAULTS = env.list(\"FEATURE_DEFAULTS\", default=[])",
            ]),
            run(0, Side::Base, 3, &[
                "LOG_LEVEL = env(\"LOG_LEVEL\", default=\"info\")",
                "LOG_FORMAT = env(\"LOG_FORMAT\", default=\"text\")",
            ]),
            run(0, Side::Base, 8, &[
                "FEATURE_FLAGS = env.list(\"FEATURE_FLAGS\", default=[])",
                "FEATURE_DEFAULTS = env.list(\"FEATURE_DEFAULTS\", default=[])",
            ]),
        ];
        detect_moves(&mut files, &runs);
        assert_eq!(files[0].moved, vec![
            moved(Side::Base, lr(3, 4), false, "config.py", lr(1, 2)),
            moved(Side::Base, lr(8, 9), false, "config.py", lr(3, 4)),
            moved(Side::Tip, lr(1, 2), false, "config.py", lr(3, 4)),
            moved(Side::Tip, lr(3, 4), true, "config.py", lr(8, 9)),
        ]);
    }

    #[test]
    fn single_line_is_not_moved() {
        let mut files = vec![file("A.java"), file("B.java")];
        let runs = vec![
            run(0, Side::Base, 4, &["import io.starburst.stargate.id.EntityId;"]),
            run(1, Side::Tip, 3, &["import io.starburst.stargate.id.EntityId;"]),
        ];
        detect_moves(&mut files, &runs);
        assert_eq!(files[0].moved, vec![]);
        assert_eq!(files[1].moved, vec![]);
    }

    #[test]
    fn block_below_the_alnum_threshold_is_not_moved() {
        let mut files = vec![file("a.rs")];
        let runs = vec![
            run(0, Side::Base, 1, &["    }", "    return x;"]),
            run(0, Side::Tip, 9, &["    }", "    return x;"]),
        ];
        detect_moves(&mut files, &runs);
        assert_eq!(files[0].moved, vec![]);
    }

    #[test]
    fn blank_line_inside_a_block_moves_and_trailing_blanks_do_not() {
        let mut files = vec![file("d.py")];
        let runs = vec![
            run(0, Side::Base, 1, &[
                "def small(req):",
                "    a = first_step(req)",
                "",
                "    return second_step(a)",
                "",
                "",
            ]),
            run(0, Side::Tip, 7, &[
                "",
                "",
                "def small(req):",
                "    a = first_step(req)",
                "",
                "    return second_step(a)",
            ]),
        ];
        detect_moves(&mut files, &runs);
        assert_eq!(files[0].moved, vec![
            moved(Side::Base, lr(1, 4), false, "d.py", lr(9, 12)),
            moved(Side::Tip, lr(9, 12), false, "d.py", lr(1, 4)),
        ]);
    }

    #[test]
    fn reindent_in_place_is_a_move() {
        let mut files = vec![file("b.py")];
        let runs = vec![
            run(0, Side::Base, 2, &[
                "    result = compute_something(x)",
                "    return transform_result(result)",
            ]),
            run(0, Side::Tip, 2, &[
                "    if x is not None:",
                "        result = compute_something(x)",
                "        return transform_result(result)",
            ]),
        ];
        detect_moves(&mut files, &runs);
        assert_eq!(files[0].moved, vec![
            moved(Side::Base, lr(2, 3), false, "b.py", lr(3, 4)),
            moved(Side::Tip, lr(3, 4), false, "b.py", lr(2, 3)),
        ]);
    }

    #[test]
    fn a_different_indentation_shift_starts_a_new_block() {
        let mut files = vec![file("e.py")];
        let runs = vec![
            run(0, Side::Base, 1, &[
                "    first_statement_with_enough_letters()",
                "    second_statement_with_enough_letters()",
                "    third_statement_with_enough_letters()",
                "    fourth_statement_with_enough_letters()",
            ]),
            run(0, Side::Tip, 1, &[
                "        first_statement_with_enough_letters()",
                "        second_statement_with_enough_letters()",
                "third_statement_with_enough_letters()",
                "fourth_statement_with_enough_letters()",
            ]),
        ];
        detect_moves(&mut files, &runs);
        assert_eq!(files[0].moved, vec![
            moved(Side::Base, lr(1, 2), false, "e.py", lr(1, 2)),
            moved(Side::Base, lr(3, 4), true, "e.py", lr(3, 4)),
            moved(Side::Tip, lr(1, 2), false, "e.py", lr(1, 2)),
            moved(Side::Tip, lr(3, 4), true, "e.py", lr(3, 4)),
        ]);
    }

    #[test]
    fn diff_over_the_limit_is_skipped() {
        let mut files = vec![file("app.py")];
        files[0].moved.push(moved(Side::Base, lr(1, 1), false, "x", lr(1, 1)));
        let runs = vec![
            run(0, Side::Base, 1, &["def load_the_configuration_file(path):"]),
            run(0, Side::Tip, 5, &["def load_the_configuration_file(path):"]),
        ];
        assert!(!detect_moves_up_to(&mut files, &runs, 1));
        assert_eq!(files[0].moved, vec![]);
    }

    #[test]
    fn runs_from_hunks_splits_removed_and_added_lines() {
        use crate::diff::{HunkLine, RegularHunk};
        let line = |origin, base_line, tip_line, content: &str| HunkLine {
            origin,
            base_line,
            tip_line,
            content: content.into(),
        };
        let hunks = vec![Hunk::Regular(RegularHunk {
            base_range: Some(lr(1, 4)),
            tip_range: Some(lr(1, 3)),
            lines: vec![
                line(LineOrigin::Context, Some(1), Some(1), "a\n"),
                line(LineOrigin::Removed, Some(2), None, "b\n"),
                line(LineOrigin::Removed, Some(3), None, "c\n"),
                line(LineOrigin::Added, None, Some(2), "d\n"),
                line(LineOrigin::Context, Some(4), Some(3), "e\n"),
            ],
        })];
        let runs = runs_from_hunks(7, &hunks);
        let shape: Vec<(usize, Side, u32, Vec<String>)> = runs
            .into_iter()
            .map(|r| (r.file, r.side, r.start, r.lines))
            .collect();
        assert_eq!(shape, vec![
            (7, Side::Base, 2, vec!["b\n".to_string(), "c\n".to_string()]),
            (7, Side::Tip, 2, vec!["d\n".to_string()]),
        ]);
    }
}
