use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use async_trait::async_trait;
use kata_core::{Bookmark, ChangeId, CommitId, CommitInfo, FileChange, FileStatus, RevSet};
use tokio::process::Command;

use crate::backend::{Endpoint, JjBackend, ReviewRange};
use crate::error::{Error, Result};

/// Subprocess-based jj backend. One per repository.
pub struct JjCli {
    repo: PathBuf,
    jj_binary: OsString,
}

impl JjCli {
    /// `repo` is the path to a directory inside a jj working copy (anywhere
    /// works — jj walks up to find `.jj/`).
    pub fn new(repo: impl Into<PathBuf>) -> Self {
        Self {
            repo: repo.into(),
            jj_binary: OsString::from("jj"),
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
        tracing::debug!(repo = ?self.repo, args = ?args, "jj");
        let output = self.cmd().args(args).output().await?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
            tracing::warn!(
                repo = ?self.repo,
                args = ?args,
                status = ?output.status.code(),
                stderr = %stderr,
                "jj failed"
            );
            return Err(Error::JjFailed {
                status: output.status.code().unwrap_or(-1),
                stderr,
            });
        }
        tracing::trace!(stdout_len = output.stdout.len(), "jj ok");
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
// 5th tab is structured, and what follows is the verbatim body.
const COMMIT_INFO_TPL: &str = r#"change_id ++ "\t" ++ commit_id ++ "\t" ++ author.email() ++ "\t" ++ author.timestamp().format("%+") ++ "\t" ++ description ++ "\0""#;

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
        tracing::debug!(count = bookmarks.len(), "list_bookmarks");
        Ok(bookmarks)
    }

    async fn change_to_commit(&self, change: &ChangeId) -> Result<Option<CommitId>> {
        let args = [
            "log",
            "--no-graph",
            "-r",
            change.as_str(),
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
        let out = self
            .run(&["log", "--no-graph", "-r", revset.as_str(), "-T", COMMIT_INFO_TPL])
            .await?;
        let text = String::from_utf8(out)
            .map_err(|e| Error::Parse(format!("commit log not utf-8: {e}")))?;
        let mut commits = Vec::new();
        for record in text.split('\0') {
            if record.is_empty() {
                continue;
            }
            let mut parts = record.splitn(5, '\t');
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
            let description = parts.next().unwrap_or("").to_string();
            let first_line = description.lines().next().unwrap_or("").to_string();
            commits.push(CommitInfo {
                change_id: ChangeId::new(change),
                commit_id: CommitId::new(commit),
                author_email: email,
                author_timestamp: ts,
                description_first_line: first_line,
                description,
            });
        }
        tracing::debug!(count = commits.len(), revset = %revset, "list_commits");
        Ok(commits)
    }

    async fn git_diff(
        &self,
        base: &CommitId,
        tip: &CommitId,
        context_lines: usize,
    ) -> Result<Vec<u8>> {
        let ctx = context_lines.to_string();
        self.run(&[
            "diff",
            "--git",
            "--context",
            &ctx,
            "--from",
            base.as_str(),
            "--to",
            tip.as_str(),
        ])
        .await
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
