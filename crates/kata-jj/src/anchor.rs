//! Line re-anchoring for comments whose commit_id no longer matches the
//! current commit of the anchored change_id.
//!
//! Strategy:
//! 1. Extract the original line range's content from `original_commit`.
//! 2. Try an exact substring match in `current_commit`'s file.
//!    - Single match → re-anchor cleanly (Valid).
//!    - Multiple matches → fall back to fuzzy with line-window scoring.
//! 3. If no exact match, slide a window of the original length over the
//!    current file and score by similarity ratio; if the best score clears
//!    [`FUZZY_THRESHOLD`], return Moved; otherwise Outdated.
//!
//! Outdated never drops the comment — the caller surfaces it with the
//! original lines and content preserved.

use std::collections::HashMap;
use std::time::Instant;

use kata_core::{CommitId, LineRange};

use crate::backend::JjBackend;
use crate::error::Result;

/// Minimum similarity ratio to accept a fuzzy match. Tunable.
const FUZZY_THRESHOLD: f32 = 0.75;

#[derive(Clone, Debug, PartialEq)]
pub enum AnchorResolution {
    /// Original commit matches current — lines render at the stored range.
    Valid,
    /// Exact content found at a different line range.
    Moved { new_range: LineRange },
    /// Fuzzy match accepted at a different line range. UI shows a "moved"
    /// marker; content has drifted but is recognisably the same region.
    Drifted { new_range: LineRange, similarity: f32 },
    /// No reasonable match found. UI shows an "outdated" marker and surfaces
    /// the original lines and content from the original commit.
    Outdated { original_content: String },
}

/// Resolve where (if anywhere) `original_range` from `original_commit`'s
/// version of `path` now lives in `current_commit`'s version.
///
/// `current_commit == original_commit` is a fast path that returns
/// [`AnchorResolution::Valid`] without I/O on the file contents.
pub async fn resolve_anchor<B: JjBackend + ?Sized>(
    backend: &B,
    path: &str,
    original_commit: &CommitId,
    original_range: LineRange,
    current_commit: &CommitId,
) -> Result<AnchorResolution> {
    let t_all = Instant::now();
    let mut read_ms: u64 = 0;
    let mut fuzzy_ms: u64 = 0;
    let outcome: &'static str;
    let result = resolve_anchor_inner(
        backend,
        path,
        original_commit,
        original_range,
        current_commit,
        &mut read_ms,
        &mut fuzzy_ms,
    )
    .await?;
    outcome = match &result {
        AnchorResolution::Valid => "valid",
        AnchorResolution::Moved { .. } => "moved",
        AnchorResolution::Drifted { .. } => "drifted",
        AnchorResolution::Outdated { .. } => "outdated",
    };
    tracing::debug!(
        elapsed_ms = t_all.elapsed().as_millis() as u64,
        read_ms,
        fuzzy_ms,
        outcome,
        path,
        "resolve_anchor",
    );
    Ok(result)
}

async fn resolve_anchor_inner<B: JjBackend + ?Sized>(
    backend: &B,
    path: &str,
    original_commit: &CommitId,
    original_range: LineRange,
    current_commit: &CommitId,
    read_ms: &mut u64,
    fuzzy_ms: &mut u64,
) -> Result<AnchorResolution> {
    if original_commit == current_commit {
        return Ok(AnchorResolution::Valid);
    }

    let t = Instant::now();
    let original_bytes = backend.read_file(original_commit, path).await?;
    let current_bytes = backend.read_file(current_commit, path).await?;
    *read_ms = t.elapsed().as_millis() as u64;

    let (Some(original_bytes), Some(current_bytes)) = (original_bytes, current_bytes) else {
        // File missing on one side. The UI will already show this as a deleted
        // or added file; the comment is outdated either way.
        let original_content = original_bytes_to_excerpt(None, original_range);
        return Ok(AnchorResolution::Outdated { original_content });
    };

    let original_text = String::from_utf8_lossy(&original_bytes);
    let current_text = String::from_utf8_lossy(&current_bytes);

    let original_lines: Vec<&str> = original_text.split_inclusive('\n').collect();
    let current_lines: Vec<&str> = current_text.split_inclusive('\n').collect();

    let snippet = slice_range(&original_lines, original_range);
    let snippet_text: String = snippet.iter().copied().collect();

    if snippet.is_empty() {
        return Ok(AnchorResolution::Outdated {
            original_content: String::new(),
        });
    }

    // 1. Exact line-sequence match.
    if let Some(range) = find_exact(&current_lines, &snippet) {
        return Ok(AnchorResolution::Moved { new_range: range });
    }

    // 2. Fuzzy: sliding window of the same length, ranked by similarity.
    let t = Instant::now();
    let fuzzy = find_fuzzy(&current_lines, &snippet);
    *fuzzy_ms = t.elapsed().as_millis() as u64;
    if let Some((range, ratio)) = fuzzy
        && ratio >= FUZZY_THRESHOLD
    {
        return Ok(AnchorResolution::Drifted {
            new_range: range,
            similarity: ratio,
        });
    }

    Ok(AnchorResolution::Outdated {
        original_content: snippet_text,
    })
}

fn slice_range<'a>(lines: &'a [&'a str], range: LineRange) -> Vec<&'a str> {
    let start = (range.start as usize).saturating_sub(1);
    let end = (range.end as usize).min(lines.len());
    if start >= end {
        return Vec::new();
    }
    lines[start..end].to_vec()
}

fn find_exact(haystack: &[&str], needle: &[&str]) -> Option<LineRange> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    let mut hits = haystack
        .windows(needle.len())
        .enumerate()
        .filter(|(_, w)| *w == needle);
    let (idx, _) = hits.next()?;
    // Multiple matches → ambiguous; let the fuzzy pass decide.
    if hits.next().is_some() {
        return None;
    }
    Some(LineRange {
        start: (idx as u32) + 1,
        end: (idx as u32) + needle.len() as u32,
    })
}

/// Score each sliding window of `needle.len()` lines in `haystack` by
/// how many of `needle`'s lines it contains (multiset overlap),
/// returning the best-scoring window's line range and ratio.
///
/// Switched from character-level Myers (`TextDiff::from_chars`) — that
/// was O(n · window · line_len²) per window and easily ate seconds per
/// comment on large files (e.g. `JsonItems.java` with a multi-line
/// snippet hit ~8.5 s). Line-level multiset overlap is O(window) per
/// position, total O(n · window). Same `ratio = 2·matches / (a+b)`
/// shape that `similar::TextDiff::ratio` produces, just measured over
/// line tokens instead of characters.
fn find_fuzzy(haystack: &[&str], needle: &[&str]) -> Option<(LineRange, f32)> {
    let window = needle.len();
    if window == 0 || haystack.len() < window {
        return None;
    }
    // Multiset of needle line → count, so a candidate with N copies of
    // the same line only matches up to N times. Plain `HashSet`
    // contains-counts would over-credit repeated context lines (think
    // closing braces, blank lines).
    let mut needle_counts: HashMap<&str, u32> = HashMap::with_capacity(window);
    for &line in needle {
        *needle_counts.entry(line).or_insert(0) += 1;
    }
    let mut best: Option<(usize, f32)> = None;
    let total = (window + window) as f32;
    let mut remaining = HashMap::with_capacity(needle_counts.len());
    for i in 0..=haystack.len() - window {
        remaining.clone_from(&needle_counts);
        let mut overlap: u32 = 0;
        for &line in &haystack[i..i + window] {
            if let Some(c) = remaining.get_mut(line)
                && *c > 0
            {
                *c -= 1;
                overlap += 1;
            }
        }
        let ratio = (overlap as f32 * 2.0) / total;
        match best {
            Some((_, b)) if ratio <= b => {}
            _ => best = Some((i, ratio)),
        }
    }
    let (i, ratio) = best?;
    Some((
        LineRange {
            start: (i as u32) + 1,
            end: (i as u32) + window as u32,
        },
        ratio,
    ))
}

fn original_bytes_to_excerpt(bytes: Option<Vec<u8>>, range: LineRange) -> String {
    let Some(bytes) = bytes else { return String::new() };
    let text = String::from_utf8_lossy(&bytes);
    let start = (range.start as usize).saturating_sub(1);
    let end = (range.end as usize).max(start + 1);
    text.split_inclusive('\n')
        .skip(start)
        .take(end - start)
        .collect()
}
