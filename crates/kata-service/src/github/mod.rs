//! GitHub interaction layer, scoped to what kata needs for the PR
//! import / publish features.
//!
//! Layering:
//!
//! * [`url`] — pure URL parsing, no I/O. Turns a github.com PR
//!   link into a [`PullRequestRef`].
//! * [`resolver`] — pure URL normalisation + workspace matching.
//! * [`client`] — stateless wrapper that delegates all REST and
//!   GraphQL calls to the `gh` CLI on the host. No in-process
//!   OAuth, no per-user tokens stored by kata — `gh` carries the
//!   user's identity and org authorizations.

pub mod client;
pub mod comments;
pub mod publish;
pub mod resolver;
pub mod url;

pub use client::{AuthStatus, GithubClient, GithubError, PullRef, PullRequest, PullState};
pub use resolver::{GithubRemote, ResolvedPrWorkspace, parse_github_remote};
pub use url::{ParsePullRequestUrlError, PullRequestRef, parse_pull_request_url};
