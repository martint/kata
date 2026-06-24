//! Match a github.com PR to an already-registered kata workspace.
//!
//! Per the simplified MVP scope (no auto-clone): kata refuses to
//! import a PR whose `(owner, repo)` doesn't match any configured
//! workspace's git remote URL. The operator owns the
//! `--workspace` set; kata never adds repos at runtime.
//!
//! Matching is owner+repo, case-insensitive (github.com canonicalises
//! both case-insensitively). Remote URL forms handled:
//!
//! * `https://github.com/<owner>/<repo>(.git)?`
//! * `git@github.com:<owner>/<repo>(.git)?`
//! * `ssh://git@github.com/<owner>/<repo>(.git)?`
//! * `git://github.com/<owner>/<repo>(.git)?`
//!
//! Anything else (gitlab, GHE, local paths, `file://`, etc.) yields
//! `None` from the normaliser and is skipped during matching.

use kata_core::RepoId;

use super::url::PullRequestRef;

/// Result of [`crate::ReviewService::resolve_pr_workspace`].
#[derive(Clone, Debug)]
pub struct ResolvedPrWorkspace {
    pub repo: RepoId,
    /// User-facing slug (what `list_repos` prints; what URL paths
    /// use). Returned alongside the canonical [`RepoId`] so callers
    /// don't have to round-trip back through the registry.
    pub repo_name: String,
    /// The remote on that workspace whose URL matched. Phase 4 hands
    /// this to [`crate::ReviewService::fetch_github_pr_head`] so it
    /// knows which remote to `git fetch` from.
    pub remote_name: String,
}

/// Owner and repo extracted from a git remote URL, normalised to
/// lowercase for case-insensitive matching against a PR ref.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GithubRemote {
    pub owner_lower: String,
    pub repo_lower: String,
}

impl GithubRemote {
    pub fn matches(&self, pr: &PullRequestRef) -> bool {
        self.owner_lower == pr.owner.to_lowercase()
            && self.repo_lower == pr.repo.to_lowercase()
    }
}

/// Parse a configured git-remote URL into a `GithubRemote`. `None`
/// for URLs that don't point at github.com or don't look like a
/// `<owner>/<repo>` path. Intentionally tolerant: we ignore the
/// scheme, port, and any user-info; we just care whether two refs
/// point at the same github.com repo.
pub fn parse_github_remote(url: &str) -> Option<GithubRemote> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return None;
    }
    let (host, path) = split_host_and_path(trimmed)?;
    if !is_github_host(host) {
        return None;
    }
    let (owner, repo) = split_owner_repo(path)?;
    Some(GithubRemote {
        owner_lower: owner.to_ascii_lowercase(),
        repo_lower: repo.to_ascii_lowercase(),
    })
}

/// Extract `(host, "<owner>/<repo>[.git][/...]")` from any of the
/// remote-URL shapes git understands. Returns `None` for forms we
/// don't intend to support — local paths, `file://`, anything
/// without a host.
fn split_host_and_path(url: &str) -> Option<(&str, &str)> {
    // 1. SCP-style `[user@]host:path` — `git@github.com:owner/repo.git`.
    //    Recognised by the *first* `:` appearing before the *first* `/`
    //    in a URL that isn't a `scheme://` URL. (For `scheme://` URLs
    //    the `://` shows up before the host:port colon.)
    if !url.contains("://")
        && let Some(colon) = url.find(':')
    {
        let pre = &url[..colon];
        let post = &url[colon + 1..];
        let host = pre.rsplit_once('@').map(|x| x.1).unwrap_or(pre);
        return Some((host, post));
    }
    // 2. `scheme://[user[:pass]@]host[:port]/path...`. Strip scheme,
    //    user-info, port. Path is whatever's left after the next `/`.
    let after_scheme = url.split_once("://").map(|x| x.1)?;
    let (authority, path) = after_scheme.split_once('/')?;
    let host_with_port = authority.rsplit_once('@').map(|x| x.1).unwrap_or(authority);
    let host = host_with_port.split_once(':').map(|x| x.0).unwrap_or(host_with_port);
    Some((host, path))
}

fn is_github_host(host: &str) -> bool {
    let h = host.to_ascii_lowercase();
    h == "github.com" || h == "www.github.com"
}

/// `<owner>/<repo>[.git][/...]` → `(owner, repo)`. Trailing `.git`,
/// `/`-suffixes (e.g. wikis, `/issues`), and fragments are tolerated.
fn split_owner_repo(path: &str) -> Option<(&str, &str)> {
    let path = path.trim_start_matches('/');
    let mut parts = path.splitn(3, '/');
    let owner = parts.next()?;
    let repo_raw = parts.next()?;
    if owner.is_empty() || repo_raw.is_empty() {
        return None;
    }
    // Stop at the first fragment / query, just in case.
    let stop = repo_raw
        .find(|c: char| c == '#' || c == '?')
        .unwrap_or(repo_raw.len());
    let repo = &repo_raw[..stop];
    let repo = repo.strip_suffix(".git").unwrap_or(repo);
    if repo.is_empty() {
        return None;
    }
    Some((owner, repo))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gr(owner: &str, repo: &str) -> GithubRemote {
        GithubRemote {
            owner_lower: owner.into(),
            repo_lower: repo.into(),
        }
    }

    #[test]
    fn https_with_dot_git() {
        assert_eq!(
            parse_github_remote("https://github.com/octocat/Hello-World.git"),
            Some(gr("octocat", "hello-world")),
        );
    }

    #[test]
    fn https_without_dot_git() {
        assert_eq!(
            parse_github_remote("https://github.com/octocat/Hello-World"),
            Some(gr("octocat", "hello-world")),
        );
    }

    #[test]
    fn ssh_scp_style() {
        assert_eq!(
            parse_github_remote("git@github.com:OctoCat/Hello-World.git"),
            Some(gr("octocat", "hello-world")),
        );
    }

    #[test]
    fn ssh_url_style() {
        assert_eq!(
            parse_github_remote("ssh://git@github.com/octocat/Hello-World.git"),
            Some(gr("octocat", "hello-world")),
        );
    }

    #[test]
    fn git_protocol() {
        assert_eq!(
            parse_github_remote("git://github.com/octocat/Hello-World.git"),
            Some(gr("octocat", "hello-world")),
        );
    }

    #[test]
    fn host_with_port_ok() {
        assert_eq!(
            parse_github_remote("ssh://git@github.com:22/octocat/Hello-World.git"),
            Some(gr("octocat", "hello-world")),
        );
    }

    #[test]
    fn www_subdomain_ok() {
        assert_eq!(
            parse_github_remote("https://www.github.com/octocat/Hello-World"),
            Some(gr("octocat", "hello-world")),
        );
    }

    #[test]
    fn non_github_host_rejected() {
        assert_eq!(
            parse_github_remote("https://gitlab.com/octocat/Hello-World.git"),
            None,
        );
        assert_eq!(
            parse_github_remote("https://github.example.com/o/r.git"),
            None,
        );
    }

    #[test]
    fn local_paths_rejected() {
        assert_eq!(parse_github_remote("/srv/repos/proj.git"), None);
        assert_eq!(parse_github_remote("file:///srv/repos/proj.git"), None);
    }

    #[test]
    fn empty_or_garbage_rejected() {
        assert_eq!(parse_github_remote(""), None);
        assert_eq!(parse_github_remote("not a url"), None);
        assert_eq!(parse_github_remote("https://github.com/onlyowner"), None);
        assert_eq!(parse_github_remote("https://github.com//"), None);
    }

    #[test]
    fn matches_is_case_insensitive() {
        let r = parse_github_remote("https://github.com/OctoCat/Hello-World.git").unwrap();
        let pr = PullRequestRef {
            owner: "octocat".into(),
            repo: "hello-world".into(),
            number: 1,
        };
        assert!(r.matches(&pr));
        let pr_upper = PullRequestRef {
            owner: "OCTOCAT".into(),
            repo: "HELLO-WORLD".into(),
            number: 1,
        };
        assert!(r.matches(&pr_upper));
        let mismatch = PullRequestRef {
            owner: "octocat".into(),
            repo: "other".into(),
            number: 1,
        };
        assert!(!r.matches(&mismatch));
    }

    #[test]
    fn trailing_path_components_tolerated() {
        // Some folks paste the repo's web URL as a remote — be
        // generous and still match.
        assert_eq!(
            parse_github_remote("https://github.com/octocat/Hello-World/tree/main"),
            Some(gr("octocat", "hello-world")),
        );
    }
}
