use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use async_trait::async_trait;
use kata_core::{
    Bookmark, ChangeId, CommitId, CommitInfo, FileChange, FileStatus, OpId, OpKind, OpSummary,
    RevSet,
};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

use crate::backend::{Endpoint, JjBackend, ReviewRange};
use crate::error::{Error, Result};

/// Subprocess-based jj backend. One per repository.
pub struct JjCli {
    repo: PathBuf,
    jj_binary: OsString,
    /// Path to a git directory if we found one alongside the jj working
    /// copy. Lets [`Self::read_files`] read every blob in a review with a
    /// single `git cat-file --batch` instead of one `jj file show` per
    /// (commit, path). `None` falls back to the sequential per-file
    /// `jj file show` path — slower but correct for non-colocated
    /// repos we don't have the git store handy for.
    git_dir: Option<PathBuf>,
}

impl JjCli {
    /// `repo` is the path to a directory inside a jj working copy (anywhere
    /// works — jj walks up to find `.jj/`).
    pub fn new(repo: impl Into<PathBuf>) -> Self {
        let repo = repo.into();
        let git_dir = detect_git_dir(&repo);
        Self {
            repo,
            jj_binary: OsString::from("jj"),
            git_dir,
        }
    }

    /// Override the jj binary path (mostly for tests).
    pub fn with_binary(mut self, bin: impl Into<OsString>) -> Self {
        self.jj_binary = bin.into();
        self
    }

    fn cmd(&self) -> Command {
        let mut c = Command::new(&self.jj_binary);
        // jj resolves file paths relative to cwd, not --repository. Setting
        // cwd to the repo lets us pass repo-relative paths verbatim.
        c.current_dir(&self.repo);
        c.arg("--repository").arg(&self.repo);
        c.env("JJ_CONFIG", "");
        c.arg("--no-pager");
        c.arg("--color=never");
        c.stdin(Stdio::null());
        c.stdout(Stdio::piped());
        c.stderr(Stdio::piped());
        c
    }

    async fn run(&self, args: &[&str]) -> Result<Vec<u8>> {
        let output = self.cmd().args(args).output().await?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
            return Err(Error::JjFailed {
                status: output.status.code().unwrap_or(-1),
                stderr,
            });
        }
        Ok(output.stdout)
    }

    /// Like `run`, but treats "revision doesn't exist" specifically as
    /// `Ok(None)` instead of an error.
    async fn run_or_missing(&self, args: &[&str]) -> Result<Option<Vec<u8>>> {
        let output = self.cmd().args(args).output().await?;
        if output.status.success() {
            return Ok(Some(output.stdout));
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("doesn't exist") || stderr.contains("No such path") {
            return Ok(None);
        }
        Err(Error::JjFailed {
            status: output.status.code().unwrap_or(-1),
            stderr: stderr.into_owned(),
        })
    }
}

const SINGLE_REV_TPL: &str = r#"change_id ++ " " ++ commit_id ++ "\n""#;
const BOOKMARK_TPL: &str = r#"if(normal_target, name ++ "\t" ++ remote ++ "\t" ++ normal_target.change_id() ++ "\t" ++ normal_target.commit_id() ++ "\t" ++ normal_target.author().timestamp().format("%+") ++ "\n", "")"#;
const DIFF_ENTRY_TPL: &str =
    r#"status ++ "\t" ++ path ++ "\t" ++ source.path() ++ "\n""#;
// Record terminator is `\0`: descriptions can contain `\t` and `\n`, so we
// can't split on either. Description is emitted last; everything before the
// 6th tab is structured, and what follows is the verbatim body. Files
// within the 5th field are joined by `\x1f` (ASCII unit separator) so the
// list stays parseable when paths contain unusual characters.
const COMMIT_INFO_TPL: &str = r#"change_id ++ "\t" ++ commit_id ++ "\t" ++ author.email() ++ "\t" ++ author.timestamp().format("%+") ++ "\t" ++ diff.files().map(|f| f.path()).join("\x1f") ++ "\t" ++ description ++ "\0""#;

/// `jj op log` template: id, snapshot-flag (Y/N), end-of-range time,
/// first-line description, newline-terminated.
const OP_LOG_TPL: &str = r#"self.id() ++ "\t" ++ if(snapshot, "Y", "N") ++ "\t" ++ time.end().format("%+") ++ "\t" ++ description.first_line() ++ "\n""#;

/// Map jj's operation description to one of the [`OpKind`] buckets. jj's
/// op descriptions all lead with the command verb (`amend commit X`,
/// `rebase commit X onto Y`, `git fetch from <remote>`, …) so a single
/// first-word match is enough for the common cases. Anything we don't
/// recognise becomes [`OpKind::Other`] with the verb preserved.
fn classify_op(description: &str) -> OpKind {
    let first = description.split_whitespace().next().unwrap_or("");
    match first {
        "amend" => OpKind::Amend,
        "rebase" => OpKind::Rebase,
        "abandon" => OpKind::Abandon,
        "describe" => OpKind::Describe,
        "new" => OpKind::New,
        "split" => OpKind::Split,
        "squash" => OpKind::Squash,
        "restore" => OpKind::Restore,
        "git" => OpKind::Git,
        _ => OpKind::Other(first.to_string()),
    }
}

#[async_trait]
impl JjBackend for JjCli {
    fn repo_path(&self) -> &Path {
        &self.repo
    }

    async fn list_bookmarks(&self) -> Result<Vec<Bookmark>> {
        let out = self
            .run(&["bookmark", "list", "-T", BOOKMARK_TPL])
            .await?;
        let text = String::from_utf8(out)
            .map_err(|e| Error::Parse(format!("bookmark list not utf-8: {e}")))?;
        let mut bookmarks = Vec::new();
        for line in text.lines() {
            if line.is_empty() {
                continue;
            }
            let mut parts = line.splitn(5, '\t');
            let name = parts
                .next()
                .ok_or_else(|| Error::Parse(format!("bookmark line missing name: {line:?}")))?;
            let remote = parts
                .next()
                .ok_or_else(|| Error::Parse(format!("bookmark line missing remote: {line:?}")))?;
            let change = parts
                .next()
                .ok_or_else(|| Error::Parse(format!("bookmark line missing change: {line:?}")))?;
            let commit = parts
                .next()
                .ok_or_else(|| Error::Parse(format!("bookmark line missing commit: {line:?}")))?;
            let timestamp = parts.next().unwrap_or("").to_string();
            // Local bookmarks only: even without `--all-remotes`, jj emits
            // remote-tracking rows for any bookmark whose local target has
            // diverged from a remote, and in colocated repos there's an
            // `@git` mirror for every local. Skip everything with a remote.
            if !remote.is_empty() {
                continue;
            }
            bookmarks.push(Bookmark {
                name: name.to_string(),
                change_id: ChangeId::new(change),
                commit_id: CommitId::new(commit),
                commit_timestamp: timestamp,
            });
        }
        // Newest-first by commit timestamp so the create-review screen can
        // surface recent branches without re-sorting client-side. Anything
        // missing a timestamp drops to the bottom.
        bookmarks.sort_by(|a, b| b.commit_timestamp.cmp(&a.commit_timestamp));
        Ok(bookmarks)
    }

    async fn resolve_endpoint(&self, expr: &str) -> Result<Option<crate::backend::Endpoint>> {
        let Some(out) = self
            .run_or_missing(&["log", "--no-graph", "-r", expr, "-T", SINGLE_REV_TPL])
            .await?
        else {
            return Ok(None);
        };
        let text = String::from_utf8(out)
            .map_err(|e| Error::Parse(format!("endpoint not utf-8: {e}")))?;
        let Some(line) = text.lines().find(|l| !l.is_empty()) else {
            return Ok(None);
        };
        let (change, commit) = line
            .split_once(' ')
            .ok_or_else(|| Error::Parse(format!("endpoint missing space: {line:?}")))?;
        Ok(Some(crate::backend::Endpoint {
            change_id: ChangeId::new(change),
            commit_id: CommitId::new(commit),
        }))
    }

    async fn change_to_commit(&self, change: &ChangeId) -> Result<Option<CommitId>> {
        // `latest(change_id(<id>))` picks one commit even when the
        // change is divergent — multiple visible commits sharing a
        // change_id, e.g. after `jj op restore` to a state with two
        // amend chains, or concurrent edits in two workspaces. A
        // bare `<id>` revset errors out in that case, which kills
        // any flow that just wants "the commit for this change."
        let revset = format!("latest(change_id({}))", change.as_str());
        let args = [
            "log",
            "--no-graph",
            "-r",
            &revset,
            "-T",
            r#"commit_id ++ "\n""#,
        ];
        let Some(out) = self.run_or_missing(&args).await? else {
            return Ok(None);
        };
        let text = String::from_utf8(out)
            .map_err(|e| Error::Parse(format!("commit_id not utf-8: {e}")))?;
        let first = text.lines().next().unwrap_or("").trim();
        if first.is_empty() {
            Ok(None)
        } else {
            Ok(Some(CommitId::new(first)))
        }
    }

    async fn read_file(&self, commit: &CommitId, path: &str) -> Result<Option<Vec<u8>>> {
        self.run_or_missing(&["file", "show", "-r", commit.as_str(), path])
            .await
    }

    async fn read_files(
        &self,
        pairs: &[(CommitId, String)],
    ) -> Result<Vec<Option<Vec<u8>>>> {
        if pairs.is_empty() {
            return Ok(Vec::new());
        }
        if let Some(git_dir) = &self.git_dir {
            return cat_file_batch(git_dir, pairs).await;
        }
        // No git store handy; fall back to the trait's sequential
        // default. The work is one `jj file show` per pair — fine for
        // correctness, slower than the batch path.
        let mut out = Vec::with_capacity(pairs.len());
        for (commit, path) in pairs {
            out.push(self.read_file(commit, path).await?);
        }
        Ok(out)
    }

    async fn changed_files(
        &self,
        base: &CommitId,
        tip: &CommitId,
    ) -> Result<Vec<FileChange>> {
        let out = self
            .run(&[
                "diff",
                "--from",
                base.as_str(),
                "--to",
                tip.as_str(),
                "-T",
                DIFF_ENTRY_TPL,
            ])
            .await?;
        let text = String::from_utf8(out)
            .map_err(|e| Error::Parse(format!("diff entries not utf-8: {e}")))?;

        let mut entries = Vec::new();
        for line in text.lines() {
            if line.is_empty() {
                continue;
            }
            let mut parts = line.splitn(3, '\t');
            let status = parts
                .next()
                .ok_or_else(|| Error::Parse(format!("diff entry missing status: {line:?}")))?;
            let path = parts
                .next()
                .ok_or_else(|| Error::Parse(format!("diff entry missing path: {line:?}")))?
                .to_string();
            let source = parts
                .next()
                .ok_or_else(|| Error::Parse(format!("diff entry missing source: {line:?}")))?
                .to_string();

            let status = match status {
                "modified" => FileStatus::Modified,
                "added" => FileStatus::Added,
                "removed" => FileStatus::Deleted,
                "renamed" => FileStatus::Renamed { old_path: source },
                // Treat copies as additions of the target — the source is unchanged.
                "copied" => FileStatus::Added,
                other => {
                    return Err(Error::Parse(format!(
                        "unknown diff entry status {other:?} in line {line:?}"
                    )));
                }
            };

            entries.push(FileChange {
                path,
                status,
                hunks: None,
                binary: false,
                added: 0,
                removed: 0,
            });
        }
        Ok(entries)
    }

    async fn resolve_range(&self, revset: &RevSet) -> Result<ReviewRange> {
        let tip = self.solo_endpoint(&format!("heads({revset})"), revset).await?;
        let base = self
            .solo_endpoint(&format!("roots({revset})-"), revset)
            .await?;
        Ok(ReviewRange { base, tip })
    }

    async fn list_commits(&self, revset: &RevSet) -> Result<Vec<CommitInfo>> {
        // `--reversed` so the oldest commit in the revset comes first; the
        // UI renders the list top-down and stacks read most naturally that
        // way.
        let out = self
            .run(&["log", "--no-graph", "--reversed", "-r", revset.as_str(), "-T", COMMIT_INFO_TPL])
            .await?;
        let text = String::from_utf8(out)
            .map_err(|e| Error::Parse(format!("commit log not utf-8: {e}")))?;
        let mut commits = Vec::new();
        for record in text.split('\0') {
            if record.is_empty() {
                continue;
            }
            let mut parts = record.splitn(6, '\t');
            let change = parts
                .next()
                .ok_or_else(|| Error::Parse(format!("commit missing change: {record:?}")))?;
            let commit = parts
                .next()
                .ok_or_else(|| Error::Parse(format!("commit missing commit_id: {record:?}")))?;
            let email = parts
                .next()
                .ok_or_else(|| Error::Parse(format!("commit missing email: {record:?}")))?
                .to_string();
            let ts = parts
                .next()
                .ok_or_else(|| Error::Parse(format!("commit missing timestamp: {record:?}")))?
                .to_string();
            let files = parts
                .next()
                .ok_or_else(|| Error::Parse(format!("commit missing files: {record:?}")))?;
            let changed_files = if files.is_empty() {
                Vec::new()
            } else {
                files.split('\u{1f}').map(str::to_string).collect()
            };
            let description = parts.next().unwrap_or("").to_string();
            let first_line = description.lines().next().unwrap_or("").to_string();
            commits.push(CommitInfo {
                change_id: ChangeId::new(change),
                commit_id: CommitId::new(commit),
                author_email: email,
                author_timestamp: ts,
                description_first_line: first_line,
                description,
                changed_files,
            });
        }
        Ok(commits)
    }


    async fn is_ancestor(
        &self,
        ancestor: &CommitId,
        descendant: &CommitId,
    ) -> Result<bool> {
        if ancestor == descendant {
            return Ok(true);
        }
        // `A::B` is the set of commits reachable from B that descend from A.
        // Empty exactly when A is not an ancestor of B.
        let expr = format!("{}::{}", ancestor.as_str(), descendant.as_str());
        let out = self
            .run(&["log", "--no-graph", "-r", &expr, "-T", r#""x\n""#])
            .await?;
        Ok(!out.is_empty())
    }

    async fn current_op_id(&self) -> Result<OpId> {
        let out = self
            .run(&[
                "op",
                "log",
                "-n",
                "1",
                "--no-graph",
                "-T",
                r#"self.id() ++ "\n""#,
            ])
            .await?;
        let text = String::from_utf8(out)
            .map_err(|e| Error::Parse(format!("op id not utf-8: {e}")))?;
        let id = text.lines().next().unwrap_or("").trim();
        if id.is_empty() {
            return Err(Error::Parse("empty op log".into()));
        }
        Ok(OpId::new(id))
    }

    async fn ops_between(
        &self,
        prev: &OpId,
        current: &OpId,
    ) -> Result<Vec<OpSummary>> {
        if prev == current {
            return Ok(Vec::new());
        }
        // jj's op log lacks a `from..to` range filter, so we ask for the
        // last N entries reachable from `current` (oldest first via
        // --reversed) and walk forward until we cross `prev`. 200 covers a
        // very active week of repo activity; if `prev` is older than that,
        // the resulting list silently caps at 200 — the user just doesn't
        // see operations from before the window.
        const WINDOW: &str = "200";
        let out = self
            .run(&[
                "op",
                "log",
                "--at-op",
                current.as_str(),
                "-n",
                WINDOW,
                "--reversed",
                "--no-graph",
                "-T",
                OP_LOG_TPL,
            ])
            .await?;
        let text = String::from_utf8(out)
            .map_err(|e| Error::Parse(format!("op log not utf-8: {e}")))?;
        let mut entries = Vec::new();
        let mut crossed_prev = false;
        for line in text.split('\n') {
            if line.is_empty() {
                continue;
            }
            let mut parts = line.splitn(4, '\t');
            let id = parts.next().unwrap_or("");
            let is_snapshot = parts.next().unwrap_or("") == "Y";
            let time = parts.next().unwrap_or("").to_string();
            let description = parts.next().unwrap_or("").to_string();
            // Skip everything up to and including `prev`.
            if !crossed_prev {
                if id == prev.as_str() {
                    crossed_prev = true;
                }
                continue;
            }
            if is_snapshot {
                continue;
            }
            entries.push(OpSummary {
                op_id: OpId::new(id),
                kind: classify_op(&description),
                time,
                description,
            });
        }
        // If we never saw `prev` in the window, `crossed_prev` is still
        // false and `entries` is empty — but the gap is real, so fall
        // back to "everything non-snapshot in the window" so the reader
        // sees *something*.
        if !crossed_prev {
            for line in text.split('\n') {
                if line.is_empty() {
                    continue;
                }
                let mut parts = line.splitn(4, '\t');
                let id = parts.next().unwrap_or("");
                let is_snapshot = parts.next().unwrap_or("") == "Y";
                let time = parts.next().unwrap_or("").to_string();
                let description = parts.next().unwrap_or("").to_string();
                if is_snapshot {
                    continue;
                }
                entries.push(OpSummary {
                    op_id: OpId::new(id),
                    kind: classify_op(&description),
                    time,
                    description,
                });
            }
        }
        Ok(entries)
    }
}

impl JjCli {
    async fn solo_endpoint(&self, expr: &str, revset: &RevSet) -> Result<Endpoint> {
        let out = self
            .run(&["log", "--no-graph", "-r", expr, "-T", SINGLE_REV_TPL])
            .await?;
        let text = String::from_utf8(out)
            .map_err(|e| Error::Parse(format!("endpoint not utf-8: {e}")))?;
        let mut lines = text.lines().filter(|l| !l.is_empty());
        let first = lines.next().ok_or_else(|| Error::EmptyRevset {
            revset: revset.to_string(),
        })?;
        if lines.next().is_some() {
            return Err(Error::MultipleHeads {
                revset: revset.to_string(),
            });
        }
        let mut parts = first.splitn(2, ' ');
        let change = parts
            .next()
            .ok_or_else(|| Error::Parse(format!("endpoint missing change_id: {first:?}")))?;
        let commit = parts
            .next()
            .ok_or_else(|| Error::Parse(format!("endpoint missing commit_id: {first:?}")))?;
        Ok(Endpoint {
            change_id: ChangeId::new(change),
            commit_id: CommitId::new(commit),
        })
    }
}

/// Look for a git directory we can drive `git cat-file --batch`
/// against. Two common shapes:
///
///   - colocated repo: `<repo>/.git` is a real git directory.
///   - non-colocated jj: `<repo>/.jj/repo/store/git` is git-format storage.
///
/// `.git` may also be a *file* (pointing at the real dir, e.g. worktrees
/// or submodules). We don't currently follow those — fall back to
/// per-file `jj file show` in that case.
fn detect_git_dir(repo: &Path) -> Option<PathBuf> {
    let dot_git = repo.join(".git");
    if dot_git.is_dir() {
        return Some(dot_git);
    }
    let internal = repo.join(".jj/repo/store/git");
    if internal.is_dir() {
        return Some(internal);
    }
    None
}

/// Read every `(commit, path)` blob with a single `git cat-file --batch`
/// invocation. One fork+exec for the whole batch instead of one per
/// pair — on a 144-file review this turns ~5 s of subprocess startup
/// into a few hundred ms of streaming.
///
/// Protocol (per `git-cat-file(1)`):
///   - Each input line is `<rev>:<path>` (we feed `<commit_id>:<path>`).
///   - Output for a hit: `<sha> <type> <size>\n<size bytes of content>\n`.
///   - Output for a miss: `<original input> missing\n`, no content.
///
/// Order of the input drives the order of the output, so the result
/// `Vec` lines up with `pairs` slot-for-slot.
async fn cat_file_batch(
    git_dir: &Path,
    pairs: &[(CommitId, String)],
) -> Result<Vec<Option<Vec<u8>>>> {
    let mut cmd = Command::new("git");
    cmd.arg("--git-dir")
        .arg(git_dir)
        .arg("cat-file")
        .arg("--batch")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(Error::Io)?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| Error::Parse("git cat-file stdin missing".into()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| Error::Parse("git cat-file stdout missing".into()))?;

    let mut buf = String::with_capacity(pairs.len() * 64);
    for (commit, path) in pairs {
        buf.push_str(commit.as_str());
        buf.push(':');
        buf.push_str(path);
        buf.push('\n');
    }
    stdin.write_all(buf.as_bytes()).await.map_err(Error::Io)?;
    stdin.shutdown().await.map_err(Error::Io)?;
    drop(stdin);

    let mut reader = BufReader::new(stdout);
    let mut header = String::new();
    let mut out = Vec::with_capacity(pairs.len());
    for _ in 0..pairs.len() {
        header.clear();
        let n = reader.read_line(&mut header).await.map_err(Error::Io)?;
        if n == 0 {
            return Err(Error::Parse(
                "git cat-file ended before all batch lines were answered".into(),
            ));
        }
        let line = header.trim_end_matches('\n');
        if line.ends_with(" missing") {
            out.push(None);
            continue;
        }
        // "<sha> <type> <size>" — the size field is the last
        // space-separated token; the others we don't need.
        let size: usize = line
            .rsplit(' ')
            .next()
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| {
                Error::Parse(format!("git cat-file header not parseable: {line:?}"))
            })?;
        let mut content = vec![0u8; size];
        reader
            .read_exact(&mut content)
            .await
            .map_err(Error::Io)?;
        // Each blob is followed by a trailing LF that's NOT part of the
        // content — consume it.
        let mut lf = [0u8; 1];
        reader.read_exact(&mut lf).await.map_err(Error::Io)?;
        out.push(Some(content));
    }

    let status = child.wait().await.map_err(Error::Io)?;
    if !status.success() {
        let mut stderr = String::new();
        if let Some(mut err) = child.stderr.take() {
            let _ = err.read_to_string(&mut stderr).await;
        }
        return Err(Error::JjFailed {
            status: status.code().unwrap_or(-1),
            stderr: format!("git cat-file --batch: {stderr}"),
        });
    }
    Ok(out)
}
