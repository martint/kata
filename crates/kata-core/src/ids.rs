use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

macro_rules! string_newtype {
    ($name:ident) => {
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
        #[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(s: impl Into<String>) -> Self {
                Self(s.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }

            pub fn into_inner(self) -> String {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl From<String> for $name {
            fn from(s: String) -> Self {
                Self(s)
            }
        }

        impl From<&str> for $name {
            fn from(s: &str) -> Self {
                Self(s.to_owned())
            }
        }
    };
}

string_newtype!(ChangeId);
string_newtype!(CommitId);
string_newtype!(RepoId);
string_newtype!(ReviewId);
string_newtype!(SessionId);
string_newtype!(CommentId);
string_newtype!(ResponseId);
string_newtype!(AnnotationId);
string_newtype!(Author);
string_newtype!(OpId);

/// Categorization of a single entry in `jj op log`. The first word of jj's
/// operation description usually maps cleanly to one of these kinds; anything
/// we don't recognize falls under [`OpKind::Other`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum OpKind {
    /// `jj amend` / `jj squash` into an existing change.
    Amend,
    /// `jj rebase`.
    Rebase,
    /// `jj abandon`.
    Abandon,
    /// `jj describe`.
    Describe,
    /// `jj new` — created a new change.
    New,
    /// `jj split`.
    Split,
    /// `jj squash` that wasn't an in-place amend (commits got combined).
    Squash,
    /// `jj restore` working-copy contents.
    Restore,
    /// `jj git push` / `jj git fetch`.
    Git,
    /// Anything else; carries the operation's leading word for display.
    Other(String),
}

/// One entry from a `jj op log` range, summarized for the UI.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct OpSummary {
    pub op_id: OpId,
    pub kind: OpKind,
    /// ISO 8601 with timezone (jj's `time` field, end of the range).
    pub time: String,
    /// jj's full operation description, e.g. `amend commit abc123...`.
    pub description: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Bookmark {
    pub name: String,
    pub change_id: ChangeId,
    pub commit_id: CommitId,
    /// Author timestamp of the commit the bookmark points at, in ISO 8601
    /// with timezone (as emitted by jj's template language). Empty if jj
    /// returned no value. Used to surface "recently updated" branches.
    #[serde(default)]
    pub commit_timestamp: String,
}

/// Public-facing description of a registered repository.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RepoSummary {
    /// URL slug (typically the workspace directory's basename).
    pub name: String,
    pub repo_id: RepoId,
    /// Canonical filesystem path of the repo's `.jj/repo`.
    pub canonical_path: String,
}

/// Per-commit metadata returned for the commits panel.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CommitInfo {
    pub change_id: ChangeId,
    pub commit_id: CommitId,
    pub author_email: String,
    /// ISO 8601 with timezone, as emitted by jj's template language.
    pub author_timestamp: String,
    /// First line of the description — convenient for compact summaries.
    pub description_first_line: String,
    /// Full commit description (may be empty, may contain newlines).
    pub description: String,
    /// Files this commit modified, added, deleted, or renamed (parent..@).
    /// Used by the UI to bucket comments per commit.
    pub changed_files: Vec<String>,
}

/// A jj revset expression. We keep this as a string and let jj parse it;
/// callers can build well-known shapes via the constructors below.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(transparent)]
pub struct RevSet(String);

impl RevSet {
    pub fn new(expr: impl Into<String>) -> Self {
        Self(expr.into())
    }

    /// `<base>..<tip>` — commits reachable from tip but not from base.
    pub fn range(base: &str, tip: &str) -> Self {
        Self(format!("{base}..{tip}"))
    }

    /// `trunk()..<bookmark>` — the canonical "review this branch" shape.
    pub fn trunk_to(bookmark: &str) -> Self {
        Self(format!("trunk()..{bookmark}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RevSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Which side of a diff a line range refers to.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Base,
    Tip,
}

/// 1-based, inclusive line range in a file at a specific revision.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct LineRange {
    pub start: u32,
    pub end: u32,
}

impl LineRange {
    pub fn single(line: u32) -> Self {
        Self { start: line, end: line }
    }

    pub fn new(start: u32, end: u32) -> Self {
        assert!(start <= end, "LineRange start must be <= end");
        assert!(start >= 1, "LineRange is 1-based");
        Self { start, end }
    }

    pub fn line_count(&self) -> u32 {
        self.end - self.start + 1
    }

    pub fn contains(&self, line: u32) -> bool {
        line >= self.start && line <= self.end
    }
}

impl fmt::Display for LineRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.start == self.end {
            write!(f, "{}", self.start)
        } else {
            write!(f, "{}-{}", self.start, self.end)
        }
    }
}

impl FromStr for LineRange {
    type Err = LineRangeParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.split_once('-') {
            None => {
                let n: u32 = s.parse().map_err(|_| LineRangeParseError(s.to_owned()))?;
                if n == 0 {
                    return Err(LineRangeParseError(s.to_owned()));
                }
                Ok(LineRange::single(n))
            }
            Some((a, b)) => {
                let start: u32 = a.parse().map_err(|_| LineRangeParseError(s.to_owned()))?;
                let end: u32 = b.parse().map_err(|_| LineRangeParseError(s.to_owned()))?;
                if start == 0 || end < start {
                    return Err(LineRangeParseError(s.to_owned()));
                }
                Ok(LineRange { start, end })
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("not a valid line range: {0:?}")]
pub struct LineRangeParseError(String);
