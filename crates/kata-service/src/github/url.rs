//! Parsing of github.com pull-request URLs.
//!
//! Accepted forms (path-suffix tolerant):
//!
//! * `https://github.com/<owner>/<repo>/pull/<n>`
//! * `https://github.com/<owner>/<repo>/pull/<n>/files`
//! * `https://github.com/<owner>/<repo>/pull/<n>/commits`
//! * `https://github.com/<owner>/<repo>/pull/<n>/commits/<sha>`
//! * `https://github.com/<owner>/<repo>/pull/<n>/files#diff-...`
//!
//! Anything else — issues URLs, raw `/blob/` URLs, fork-comparison
//! URLs — is rejected. Trailing whitespace and surrounding whitespace
//! are stripped before parsing; case on `owner` / `repo` is preserved
//! verbatim because some downstream API calls treat them as case-
//! sensitive even though github.com itself canonicalises.
//!
//! Hostnames `github.com` and `www.github.com` are accepted; GHE
//! (enterprise) hostnames are NOT accepted for the MVP — when we
//! ship GHE support, this is the one function to extend.

/// A parsed PR reference. `number` is the PR number on github.com,
/// not a kata review id.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PullRequestRef {
    pub owner: String,
    pub repo: String,
    pub number: u32,
}

#[derive(Debug, thiserror::Error)]
pub enum ParsePullRequestUrlError {
    #[error("not a URL: {0}")]
    NotAUrl(String),
    #[error("only https://github.com URLs are supported (got {0})")]
    WrongHost(String),
    #[error("URL is not a pull-request URL: {0}")]
    WrongShape(String),
    #[error("pull-request number {0} is not a positive integer")]
    BadNumber(String),
}

pub fn parse_pull_request_url(input: &str) -> Result<PullRequestRef, ParsePullRequestUrlError> {
    let trimmed = input.trim();
    let url = reqwest::Url::parse(trimmed)
        .map_err(|_| ParsePullRequestUrlError::NotAUrl(trimmed.to_owned()))?;
    let host = url.host_str().unwrap_or("");
    // The MVP is github.com only. GHE support is one extra check
    // and a base-URL field on the runtime — explicitly deferred.
    if host != "github.com" && host != "www.github.com" {
        return Err(ParsePullRequestUrlError::WrongHost(host.to_owned()));
    }
    // Path segments, stripped of empties (leading `/` produces one,
    // trailing `/` produces one). `<owner>/<repo>/pull/<n>[/...]`.
    let segments: Vec<&str> = url
        .path_segments()
        .map(|it| it.filter(|s| !s.is_empty()).collect())
        .unwrap_or_default();
    if segments.len() < 4 || segments[2] != "pull" {
        return Err(ParsePullRequestUrlError::WrongShape(trimmed.to_owned()));
    }
    let number: u32 = segments[3]
        .parse()
        .map_err(|_| ParsePullRequestUrlError::BadNumber(segments[3].to_owned()))?;
    if number == 0 {
        return Err(ParsePullRequestUrlError::BadNumber(segments[3].to_owned()));
    }
    Ok(PullRequestRef {
        owner: segments[0].to_owned(),
        repo: segments[1].to_owned(),
        number,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(owner: &str, repo: &str, number: u32) -> PullRequestRef {
        PullRequestRef {
            owner: owner.into(),
            repo: repo.into(),
            number,
        }
    }

    #[test]
    fn basic_pull_url() {
        assert_eq!(
            parse_pull_request_url("https://github.com/octocat/Hello-World/pull/42").unwrap(),
            r("octocat", "Hello-World", 42),
        );
    }

    #[test]
    fn files_suffix_is_ok() {
        assert_eq!(
            parse_pull_request_url("https://github.com/octocat/Hello-World/pull/42/files").unwrap(),
            r("octocat", "Hello-World", 42),
        );
    }

    #[test]
    fn commits_sha_suffix_is_ok() {
        assert_eq!(
            parse_pull_request_url(
                "https://github.com/octocat/Hello-World/pull/42/commits/abc1234"
            )
            .unwrap(),
            r("octocat", "Hello-World", 42),
        );
    }

    #[test]
    fn fragment_is_ignored() {
        assert_eq!(
            parse_pull_request_url(
                "https://github.com/octocat/Hello-World/pull/42/files#diff-abc"
            )
            .unwrap(),
            r("octocat", "Hello-World", 42),
        );
    }

    #[test]
    fn whitespace_is_trimmed() {
        assert_eq!(
            parse_pull_request_url("  https://github.com/o/r/pull/1  \n").unwrap(),
            r("o", "r", 1),
        );
    }

    #[test]
    fn www_subdomain_accepted() {
        assert_eq!(
            parse_pull_request_url("https://www.github.com/o/r/pull/9").unwrap(),
            r("o", "r", 9),
        );
    }

    #[test]
    fn non_github_host_rejected() {
        let err = parse_pull_request_url("https://gitlab.example.com/o/r/pull/1").unwrap_err();
        assert!(matches!(err, ParsePullRequestUrlError::WrongHost(_)));
    }

    #[test]
    fn issues_url_rejected() {
        let err = parse_pull_request_url("https://github.com/o/r/issues/3").unwrap_err();
        assert!(matches!(err, ParsePullRequestUrlError::WrongShape(_)));
    }

    #[test]
    fn missing_number_rejected() {
        let err = parse_pull_request_url("https://github.com/o/r/pull/").unwrap_err();
        assert!(matches!(err, ParsePullRequestUrlError::WrongShape(_)));
    }

    #[test]
    fn non_numeric_number_rejected() {
        let err = parse_pull_request_url("https://github.com/o/r/pull/abc").unwrap_err();
        assert!(matches!(err, ParsePullRequestUrlError::BadNumber(_)));
    }

    #[test]
    fn zero_number_rejected() {
        let err = parse_pull_request_url("https://github.com/o/r/pull/0").unwrap_err();
        assert!(matches!(err, ParsePullRequestUrlError::BadNumber(_)));
    }

    #[test]
    fn garbage_input_rejected() {
        assert!(parse_pull_request_url("not a url").is_err());
        assert!(parse_pull_request_url("").is_err());
    }

    #[test]
    fn case_is_preserved() {
        // github.com canonicalises on its side, but some endpoints
        // (e.g. the GraphQL `repository(owner, name)`) are case-
        // sensitive on the input. Preserve what the user pasted.
        let r = parse_pull_request_url("https://github.com/OctoCat/Hello-World/pull/1").unwrap();
        assert_eq!(r.owner, "OctoCat");
        assert_eq!(r.repo, "Hello-World");
    }
}
