//! Build a [`Diff`] by running each changed file's two sides through
//! the histogram diff algorithm (via the `imara-diff` crate). jj's own
//! `diff --git` uses a Myers variant that, for stacks with repeated
//! short lines (`addType(...)`, closing braces, etc.), regularly
//! mis-groups a `-X` and a `+Y` across context that "matched cheaply",
//! which the user sees as the wrong line being marked added/removed.
//!
//! The flow:
//!   1. Ask jj for the set of changed files (status + rename info) —
//!      that's already a single subprocess call.
//!   2. For each modified / renamed file, read both sides via
//!      `read_file` and feed them to `imara-diff`'s histogram.
//!   3. Round-trip through [`parse_git_diff`] so we keep one place
//!      that knows the per-line shape of a Hunk.
//!
//! Added / deleted files just diff against an empty side.

use imara_diff::intern::InternedInput;
use imara_diff::sink::Counter;
use imara_diff::{Algorithm, UnifiedDiffBuilder, diff};
use kata_core::{
    CommitId, Diff, FileChange, FileStatus, Hunk, HunkLine, LineOrigin, LineRange,
};

use crate::backend::JjBackend;
use crate::error::{Error, Result};

/// File list + per-file `added` / `removed` counts. No hunks, no
/// per-line content. Used by `open_review` to keep the initial JSON
/// tiny (hunks ship lazily via [`compute_one_file_hunks`] as the
/// user scrolls each file into view) while still giving the file
/// tree enough information to render its +/- summaries.
///
/// The count path runs the same blob reads and histogram as
/// [`build_diff`], but pipes the algorithm through imara-diff's
/// [`Counter`] sink rather than `UnifiedDiffBuilder` — so we skip
/// materialising and parsing the unified-diff text.
pub async fn build_diff_metadata<B: JjBackend + ?Sized>(
    backend: &B,
    base: &CommitId,
    tip: &CommitId,
) -> Result<Diff> {
    let mut files = backend.changed_files(base, tip).await?;
    let (pairs, indices) = collect_blob_pairs(base, tip, &files);
    let blobs = backend.read_files(&pairs).await?;
    for (f, (bi, ti)) in files.iter_mut().zip(indices.into_iter()) {
        let base_bytes: &[u8] = bi.and_then(|i| blobs[i].as_deref()).unwrap_or(&[]);
        let tip_bytes: &[u8] = ti.and_then(|i| blobs[i].as_deref()).unwrap_or(&[]);
        if looks_binary(base_bytes) || looks_binary(tip_bytes) {
            f.binary = true;
            continue;
        }
        let base_text = String::from_utf8_lossy(base_bytes);
        let tip_text = String::from_utf8_lossy(tip_bytes);
        let input = InternedInput::new(base_text.as_ref(), tip_text.as_ref());
        let counter = diff(Algorithm::Histogram, &input, Counter::default());
        f.added = counter.insertions;
        f.removed = counter.removals;
    }
    Ok(Diff {
        base: base.clone(),
        tip: tip.clone(),
        files,
    })
}

/// Build the `(commit, path)` pair list + per-file index map used by
/// the diff loops. Shared between `build_diff_metadata` (count-only)
/// and `build_diff` (full hunks): they make identical reads, just
/// feed the bytes into different sinks.
fn collect_blob_pairs(
    base: &CommitId,
    tip: &CommitId,
    files: &[FileChange],
) -> (Vec<(CommitId, String)>, Vec<(Option<usize>, Option<usize>)>) {
    let mut pairs: Vec<(CommitId, String)> = Vec::with_capacity(files.len() * 2);
    let mut indices: Vec<(Option<usize>, Option<usize>)> = Vec::with_capacity(files.len());
    for f in files {
        let (base_path, tip_path) = side_paths(f);
        let bi = base_path.map(|p| {
            pairs.push((base.clone(), p.to_string()));
            pairs.len() - 1
        });
        let ti = tip_path.map(|p| {
            pairs.push((tip.clone(), p.to_string()));
            pairs.len() - 1
        });
        indices.push((bi, ti));
    }
    (pairs, indices)
}

/// Sum added/removed lines from a hunk list. Used by `parse_git_diff`
/// to populate `FileChange::added`/`removed` from the parsed shape so
/// every code path produces consistent counts.
fn count_lines(hunks: &[Hunk]) -> (u32, u32) {
    let mut added: u32 = 0;
    let mut removed: u32 = 0;
    for h in hunks {
        for l in &h.lines {
            match l.origin {
                LineOrigin::Added => added += 1,
                LineOrigin::Removed => removed += 1,
                LineOrigin::Context => {}
            }
        }
    }
    (added, removed)
}

/// Compute hunks + binary flag for one file. Reads both sides via
/// `read_files` (one git cat-file --batch call for the pair) and
/// runs histogram. Returns the input `file` with `hunks` / `binary`
/// filled in. Pure with respect to `file` — caller may pass an
/// owned [`FileChange`] from `changed_files` and assume it back
/// updated.
pub async fn compute_one_file_hunks<B: JjBackend + ?Sized>(
    backend: &B,
    base: &CommitId,
    tip: &CommitId,
    mut file: FileChange,
) -> Result<FileChange> {
    let (base_path, tip_path) = side_paths(&file);
    let mut pairs: Vec<(CommitId, String)> = Vec::with_capacity(2);
    let base_idx = base_path.map(|p| {
        pairs.push((base.clone(), p.to_string()));
        pairs.len() - 1
    });
    let tip_idx = tip_path.map(|p| {
        pairs.push((tip.clone(), p.to_string()));
        pairs.len() - 1
    });
    let blobs = backend.read_files(&pairs).await?;
    let base_bytes: &[u8] = base_idx
        .and_then(|i| blobs[i].as_deref())
        .unwrap_or(&[]);
    let tip_bytes: &[u8] = tip_idx
        .and_then(|i| blobs[i].as_deref())
        .unwrap_or(&[]);
    if looks_binary(base_bytes) || looks_binary(tip_bytes) {
        file.binary = true;
        file.hunks = None;
        return Ok(file);
    }
    let base_text = String::from_utf8_lossy(base_bytes);
    let tip_text = String::from_utf8_lossy(tip_bytes);
    let hunks = histogram_hunks(&base_text, &tip_text, &file.path)?;
    let (added, removed) = count_lines(&hunks);
    file.hunks = Some(hunks);
    file.added = added;
    file.removed = removed;
    Ok(file)
}

/// Which side(s) of a diff each file's hunk comes from. Modified
/// files read both, added/deleted only one, renamed reads the old
/// name on base and the new name on tip.
fn side_paths(file: &FileChange) -> (Option<&str>, Option<&str>) {
    match &file.status {
        FileStatus::Added => (None, Some(file.path.as_str())),
        FileStatus::Deleted => (Some(file.path.as_str()), None),
        FileStatus::Modified => (Some(file.path.as_str()), Some(file.path.as_str())),
        FileStatus::Renamed { old_path } => {
            (Some(old_path.as_str()), Some(file.path.as_str()))
        }
    }
}

pub async fn build_diff<B: JjBackend + ?Sized>(
    backend: &B,
    base: &CommitId,
    tip: &CommitId,
) -> Result<Diff> {
    let mut files = backend.changed_files(base, tip).await?;

    let (pairs, pair_indices) = collect_blob_pairs(base, tip, &files);
    let blobs = backend.read_files(&pairs).await?;

    for (f, (bi, ti)) in files.iter_mut().zip(pair_indices.into_iter()) {
        let base_bytes: &[u8] = bi
            .and_then(|i| blobs[i].as_deref())
            .unwrap_or(&[]);
        let tip_bytes: &[u8] = ti
            .and_then(|i| blobs[i].as_deref())
            .unwrap_or(&[]);
        if looks_binary(base_bytes) || looks_binary(tip_bytes) {
            f.binary = true;
            f.hunks = None;
            continue;
        }
        let base_text = String::from_utf8_lossy(base_bytes);
        let tip_text = String::from_utf8_lossy(tip_bytes);
        let hunks = histogram_hunks(&base_text, &tip_text, &f.path)?;
        let (added, removed) = count_lines(&hunks);
        f.hunks = Some(hunks);
        f.added = added;
        f.removed = removed;
    }

    Ok(Diff {
        base: base.clone(),
        tip: tip.clone(),
        files,
    })
}

/// Run histogram diff on the two sides and convert the result into our
/// [`Hunk`] shape. The round-trip via `parse_git_diff` is intentional —
/// it keeps one parser for the per-line shape, and means anything that
/// goes wrong here surfaces with the same diagnostics as everything
/// else.
fn histogram_hunks(base: &str, tip: &str, path: &str) -> Result<Vec<Hunk>> {
    let input = InternedInput::new(base, tip);
    let unified = diff(Algorithm::Histogram, &input, UnifiedDiffBuilder::new(&input));
    if unified.is_empty() {
        return Ok(Vec::new());
    }
    // Wrap imara's hunks with the headers `parse_git_diff` expects.
    // The path doesn't have to match a real on-disk path — it's only
    // used to satisfy the parser's per-file framing; the resulting
    // `FileChange.path` is thrown away (the caller already has it).
    let mut synthetic = String::with_capacity(unified.len() + path.len() * 3 + 64);
    synthetic.push_str("diff --git a/");
    synthetic.push_str(path);
    synthetic.push_str(" b/");
    synthetic.push_str(path);
    synthetic.push('\n');
    synthetic.push_str("--- a/");
    synthetic.push_str(path);
    synthetic.push('\n');
    synthetic.push_str("+++ b/");
    synthetic.push_str(path);
    synthetic.push('\n');
    synthetic.push_str(&unified);
    let parsed = parse_git_diff(synthetic.as_bytes())?;
    Ok(parsed.into_iter().next().and_then(|f| f.hunks).unwrap_or_default())
}

/// Git's binary-detection heuristic: a NUL byte in the first 8 KB.
/// Cheap and roughly what `git diff` does.
fn looks_binary(bytes: &[u8]) -> bool {
    const PROBE_LEN: usize = 8000;
    bytes.iter().take(PROBE_LEN).any(|&b| b == 0)
}

/// State carried while parsing one file's section of the git-diff output.
#[derive(Default)]
struct PartialFile {
    /// `Some` once we've seen any path indication, regardless of source.
    a_path: Option<String>,
    b_path: Option<String>,
    rename_from: Option<String>,
    rename_to: Option<String>,
    saw_new_file: bool,
    saw_deleted_file: bool,
    binary: bool,
    hunks: Vec<Hunk>,
    /// Non-`None` once we're inside `@@ … @@` and haven't moved on yet.
    cur: Option<PartialHunk>,
}

struct PartialHunk {
    base_start: u32,
    tip_start: u32,
    base_cursor: u32,
    tip_cursor: u32,
    base_end: u32,
    tip_end: u32,
    saw_base: bool,
    saw_tip: bool,
    lines: Vec<HunkLine>,
}

impl PartialHunk {
    fn new(base_start: u32, base_count: u32, tip_start: u32, tip_count: u32) -> Self {
        Self {
            base_start,
            tip_start,
            // Empty range (count == 0) means "before line `start`"; cursors
            // are still 1-based for the first real line on each side.
            base_cursor: if base_count == 0 { base_start + 1 } else { base_start },
            tip_cursor: if tip_count == 0 { tip_start + 1 } else { tip_start },
            base_end: 0,
            tip_end: 0,
            saw_base: false,
            saw_tip: false,
            lines: Vec::new(),
        }
    }

    fn push(&mut self, origin: LineOrigin, content: String) {
        let (base_line, tip_line) = match origin {
            LineOrigin::Context => {
                let b = self.base_cursor;
                let t = self.tip_cursor;
                self.base_cursor += 1;
                self.tip_cursor += 1;
                self.base_end = b;
                self.tip_end = t;
                self.saw_base = true;
                self.saw_tip = true;
                (Some(b), Some(t))
            }
            LineOrigin::Added => {
                let t = self.tip_cursor;
                self.tip_cursor += 1;
                self.tip_end = t;
                self.saw_tip = true;
                (None, Some(t))
            }
            LineOrigin::Removed => {
                let b = self.base_cursor;
                self.base_cursor += 1;
                self.base_end = b;
                self.saw_base = true;
                (Some(b), None)
            }
        };
        self.lines.push(HunkLine {
            origin,
            base_line,
            tip_line,
            content,
        });
    }

    fn into_hunk(self) -> Hunk {
        let base_range = if self.saw_base {
            Some(LineRange {
                start: self.base_start,
                end: self.base_end,
            })
        } else {
            None
        };
        let tip_range = if self.saw_tip {
            Some(LineRange {
                start: self.tip_start,
                end: self.tip_end,
            })
        } else {
            None
        };
        Hunk {
            base_range,
            tip_range,
            lines: self.lines,
        }
    }
}

impl PartialFile {
    fn commit_hunk(&mut self) {
        if let Some(h) = self.cur.take() {
            self.hunks.push(h.into_hunk());
        }
    }

    fn finalize(mut self) -> Result<FileChange> {
        self.commit_hunk();

        // Determine status + paths from whatever signals jj gave us. We
        // prefer the `rename from/to` headers when present; fall back to
        // the `+++ /dev/null` / `--- /dev/null` markers, then to the
        // `new file` / `deleted file` lines.
        let (path, status) = if let (Some(from), Some(to)) =
            (self.rename_from.clone(), self.rename_to.clone())
        {
            (to, FileStatus::Renamed { old_path: from })
        } else if self.saw_new_file || self.a_path.as_deref() == Some("/dev/null") {
            let path = self
                .b_path
                .clone()
                .ok_or_else(|| Error::Parse("added file with no +++ path".into()))?;
            (path, FileStatus::Added)
        } else if self.saw_deleted_file || self.b_path.as_deref() == Some("/dev/null") {
            let path = self
                .a_path
                .clone()
                .ok_or_else(|| Error::Parse("deleted file with no --- path".into()))?;
            (path, FileStatus::Deleted)
        } else {
            let path = self
                .b_path
                .clone()
                .or_else(|| self.a_path.clone())
                .ok_or_else(|| Error::Parse("file section with no path".into()))?;
            (path, FileStatus::Modified)
        };

        // Tally +/- before we move `self.hunks` into the option. Binary
        // files have no line concept, so their counts stay zero.
        let (added, removed) = if self.binary {
            (0, 0)
        } else {
            count_lines(&self.hunks)
        };
        let hunks = if self.binary { None } else { Some(self.hunks) };
        Ok(FileChange {
            path,
            status,
            hunks,
            binary: self.binary,
            added,
            removed,
        })
    }
}

/// Parse the output of `jj diff --git` into structured per-file changes.
///
/// The parser walks the output line-by-line as a small state machine.
/// `diff --git` lines split files; `--- ` / `+++ ` declare path & status;
/// `@@ ` opens a hunk; subsequent `+`/`-`/space-prefixed lines feed it.
fn parse_git_diff(bytes: &[u8]) -> Result<Vec<FileChange>> {
    let text = std::str::from_utf8(bytes)
        .map_err(|e| Error::Parse(format!("git diff not utf-8: {e}")))?;
    let mut files: Vec<FileChange> = Vec::new();
    let mut cur: Option<PartialFile> = None;

    for raw in text.split_inclusive('\n') {
        let line = raw.strip_suffix('\n').unwrap_or(raw);

        if let Some(rest) = line.strip_prefix("diff --git ") {
            if let Some(p) = cur.take() {
                files.push(p.finalize()?);
            }
            let mut next = PartialFile::default();
            // Seed paths from the `diff --git a/X b/Y` line. `--- `/`+++ `
            // override these later if jj emits them (binary diffs don't).
            if let Some((a, b)) = parse_diff_git_paths(rest) {
                next.a_path = Some(a);
                next.b_path = Some(b);
            }
            cur = Some(next);
            continue;
        }

        let Some(p) = cur.as_mut() else { continue };

        // Inside a hunk body, only the marker character dictates routing.
        if p.cur.is_some() && matches_hunk_body(line) {
            // jj follows git's convention of `\ No newline at end of file`
            // on its own line — skip; it doesn't add a logical line.
            if let Some(rest) = line.strip_prefix('\\') {
                let _ = rest;
                continue;
            }
            let marker = line.as_bytes().first().copied();
            let origin = match marker {
                Some(b' ') => LineOrigin::Context,
                Some(b'+') => LineOrigin::Added,
                Some(b'-') => LineOrigin::Removed,
                _ => continue,
            };
            // `raw[1..]` keeps the trailing newline if present, matching
            // the convention the UI expects (it strips `\n` at render).
            let content = raw.get(1..).unwrap_or("").to_string();
            if let Some(h) = p.cur.as_mut() {
                h.push(origin, content);
            }
            continue;
        }

        if line.starts_with("@@ ") {
            p.commit_hunk();
            let (bs, bc, ts, tc) = parse_hunk_header(line)?;
            p.cur = Some(PartialHunk::new(bs, bc, ts, tc));
            continue;
        }

        if let Some(rest) = line.strip_prefix("--- ") {
            p.a_path = Some(strip_prefix(rest, "a/").to_string());
            continue;
        }
        if let Some(rest) = line.strip_prefix("+++ ") {
            p.b_path = Some(strip_prefix(rest, "b/").to_string());
            continue;
        }
        if let Some(rest) = line.strip_prefix("rename from ") {
            p.rename_from = Some(rest.to_string());
            continue;
        }
        if let Some(rest) = line.strip_prefix("rename to ") {
            p.rename_to = Some(rest.to_string());
            continue;
        }
        if line.starts_with("new file mode ") {
            p.saw_new_file = true;
            continue;
        }
        if line.starts_with("deleted file mode ") {
            p.saw_deleted_file = true;
            continue;
        }
        if line.starts_with("Binary files ") {
            p.binary = true;
            continue;
        }
        // `index`, `similarity`, `copy from/to`, mode changes, etc.
        // Everything else in the header we don't care about.
    }

    if let Some(p) = cur.take() {
        files.push(p.finalize()?);
    }

    Ok(files)
}

/// True when `line` looks like a hunk-body line (` `, `+`, `-`, `\`). We
/// need this discriminator because some header lines like `--- a/foo` and
/// `+++ b/foo` would otherwise be mistaken for `-` / `+` content.
fn matches_hunk_body(line: &str) -> bool {
    match line.as_bytes().first() {
        Some(b' ') | Some(b'\\') => true,
        Some(b'+') => !line.starts_with("+++ "),
        Some(b'-') => !line.starts_with("--- "),
        _ => false,
    }
}

fn strip_prefix<'a>(s: &'a str, prefix: &str) -> &'a str {
    s.strip_prefix(prefix).unwrap_or(s)
}

/// Parse the path pair from a `diff --git a/X b/Y` line's tail (after the
/// `diff --git ` prefix). Returns `None` for shapes we don't recognise
/// (e.g. quoted paths, which jj only emits for unusual filenames — we'd
/// rather fall through to the `--- `/`+++ ` lines for those).
fn parse_diff_git_paths(rest: &str) -> Option<(String, String)> {
    if rest.starts_with('"') {
        return None;
    }
    let (a, b) = rest.split_once(' ')?;
    let a = a.strip_prefix("a/")?;
    let b = b.strip_prefix("b/")?;
    Some((a.to_string(), b.to_string()))
}

/// Parse `@@ -ba[,bn] +ta[,tn] @@ [optional]` into `(ba, bn, ta, tn)`.
/// Missing counts default to 1, matching git's convention.
fn parse_hunk_header(line: &str) -> Result<(u32, u32, u32, u32)> {
    let rest = line
        .strip_prefix("@@ ")
        .ok_or_else(|| Error::Parse(format!("hunk header missing @@: {line:?}")))?;
    let end = rest
        .find(" @@")
        .ok_or_else(|| Error::Parse(format!("hunk header missing closing @@: {line:?}")))?;
    let spec = &rest[..end];
    let mut parts = spec.split_whitespace();
    let base = parts
        .next()
        .ok_or_else(|| Error::Parse(format!("hunk header missing base spec: {line:?}")))?;
    let tip = parts
        .next()
        .ok_or_else(|| Error::Parse(format!("hunk header missing tip spec: {line:?}")))?;
    let base = base
        .strip_prefix('-')
        .ok_or_else(|| Error::Parse(format!("hunk header base missing `-`: {line:?}")))?;
    let tip = tip
        .strip_prefix('+')
        .ok_or_else(|| Error::Parse(format!("hunk header tip missing `+`: {line:?}")))?;
    let (bs, bc) = parse_range(base, line)?;
    let (ts, tc) = parse_range(tip, line)?;
    Ok((bs, bc, ts, tc))
}

fn parse_range(spec: &str, line: &str) -> Result<(u32, u32)> {
    let (start, count) = match spec.split_once(',') {
        Some((s, c)) => (s, c),
        None => (spec, "1"),
    };
    let start: u32 = start
        .parse()
        .map_err(|_| Error::Parse(format!("hunk start not a u32: {line:?}")))?;
    let count: u32 = count
        .parse()
        .map_err(|_| Error::Parse(format!("hunk count not a u32: {line:?}")))?;
    Ok((start, count))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(s: &str) -> Vec<(LineOrigin, Option<u32>, Option<u32>, String)> {
        let files = parse_git_diff(s.as_bytes()).unwrap();
        let mut out = Vec::new();
        for f in &files {
            for h in f.hunks.as_ref().unwrap() {
                for l in &h.lines {
                    out.push((l.origin, l.base_line, l.tip_line, l.content.clone()));
                }
            }
        }
        out
    }

    #[test]
    fn parses_modified_file_with_one_hunk() {
        let input = "\
diff --git a/foo.txt b/foo.txt
--- a/foo.txt
+++ b/foo.txt
@@ -1,3 +1,3 @@
 one
-two
+TWO
 three
";
        let files = parse_git_diff(input.as_bytes()).unwrap();
        assert_eq!(files.len(), 1);
        let f = &files[0];
        assert_eq!(f.path, "foo.txt");
        assert!(matches!(f.status, FileStatus::Modified));
        let h = &f.hunks.as_ref().unwrap()[0];
        assert_eq!(h.base_range, Some(LineRange { start: 1, end: 3 }));
        assert_eq!(h.tip_range, Some(LineRange { start: 1, end: 3 }));
        assert_eq!(h.lines.len(), 4);
        assert_eq!(h.lines[1].origin, LineOrigin::Removed);
        assert_eq!(h.lines[1].base_line, Some(2));
        assert_eq!(h.lines[1].tip_line, None);
        assert_eq!(h.lines[2].origin, LineOrigin::Added);
        assert_eq!(h.lines[2].base_line, None);
        assert_eq!(h.lines[2].tip_line, Some(2));
    }

    #[test]
    fn parses_added_file_via_dev_null() {
        let input = "\
diff --git a/new.txt b/new.txt
new file mode 100644
--- /dev/null
+++ b/new.txt
@@ -0,0 +1,2 @@
+hello
+world
";
        let files = parse_git_diff(input.as_bytes()).unwrap();
        assert_eq!(files[0].path, "new.txt");
        assert!(matches!(files[0].status, FileStatus::Added));
        let h = &files[0].hunks.as_ref().unwrap()[0];
        assert!(h.base_range.is_none());
        assert_eq!(h.tip_range, Some(LineRange { start: 1, end: 2 }));
    }

    #[test]
    fn parses_deleted_file() {
        let input = "\
diff --git a/gone.txt b/gone.txt
deleted file mode 100644
--- a/gone.txt
+++ /dev/null
@@ -1,2 +0,0 @@
-bye
-bye
";
        let files = parse_git_diff(input.as_bytes()).unwrap();
        assert!(matches!(files[0].status, FileStatus::Deleted));
        let h = &files[0].hunks.as_ref().unwrap()[0];
        assert_eq!(h.base_range, Some(LineRange { start: 1, end: 2 }));
        assert!(h.tip_range.is_none());
    }

    #[test]
    fn parses_rename_with_modifications() {
        let input = "\
diff --git a/old.txt b/new.txt
rename from old.txt
rename to new.txt
--- a/old.txt
+++ b/new.txt
@@ -1 +1 @@
-old
+new
";
        let files = parse_git_diff(input.as_bytes()).unwrap();
        assert_eq!(files[0].path, "new.txt");
        match &files[0].status {
            FileStatus::Renamed { old_path } => assert_eq!(old_path, "old.txt"),
            other => panic!("expected renamed, got {other:?}"),
        }
    }

    #[test]
    fn parses_binary_file() {
        let input = "\
diff --git a/bin b/bin
Binary files a/bin and b/bin differ
";
        let files = parse_git_diff(input.as_bytes()).unwrap();
        assert!(files[0].binary);
        assert!(files[0].hunks.is_none());
    }

    #[test]
    fn parses_multiple_files() {
        let input = "\
diff --git a/a.txt b/a.txt
--- a/a.txt
+++ b/a.txt
@@ -1 +1 @@
-a
+A
diff --git a/b.txt b/b.txt
--- a/b.txt
+++ b/b.txt
@@ -1 +1 @@
-b
+B
";
        let files = parse_git_diff(input.as_bytes()).unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, "a.txt");
        assert_eq!(files[1].path, "b.txt");
    }

    #[test]
    fn line_cursors_advance_correctly_across_multiple_hunks() {
        let input = "\
diff --git a/x b/x
--- a/x
+++ b/x
@@ -10,3 +10,3 @@
 a
-b
+B
 c
@@ -100,2 +100,2 @@
-d
+D
 e
";
        let parsed = lines(input);
        // First hunk: context a (b=10,t=10), removed b (b=11), added B (t=11), context c (b=12,t=12).
        assert_eq!(parsed[0].1, Some(10)); // base
        assert_eq!(parsed[0].2, Some(10)); // tip
        assert_eq!(parsed[1].1, Some(11));
        assert_eq!(parsed[1].2, None);
        assert_eq!(parsed[2].1, None);
        assert_eq!(parsed[2].2, Some(11));
        assert_eq!(parsed[3].1, Some(12));
        assert_eq!(parsed[3].2, Some(12));
        // Second hunk: removed d (b=100), added D (t=100), context e (b=101,t=101).
        assert_eq!(parsed[4].1, Some(100));
        assert_eq!(parsed[4].2, None);
        assert_eq!(parsed[5].1, None);
        assert_eq!(parsed[5].2, Some(100));
        assert_eq!(parsed[6].1, Some(101));
        assert_eq!(parsed[6].2, Some(101));
    }

    #[test]
    fn handles_no_newline_marker() {
        let input = "\
diff --git a/f b/f
--- a/f
+++ b/f
@@ -1 +1 @@
-old
\\ No newline at end of file
+new
";
        let files = parse_git_diff(input.as_bytes()).unwrap();
        let h = &files[0].hunks.as_ref().unwrap()[0];
        assert_eq!(h.lines.len(), 2);
        assert_eq!(h.lines[0].origin, LineOrigin::Removed);
        assert_eq!(h.lines[1].origin, LineOrigin::Added);
    }
}
