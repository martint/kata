//! Read and write Markdown files with TOML frontmatter.
//!
//! The format follows Hugo's `+++`-fenced convention:
//!
//! ```text
//! +++
//! <toml>
//! +++
//! <markdown body...>
//! ```
//!
//! We hand-split the fences instead of pulling in a heavier crate because
//! the format is trivial and the error messages are clearer this way.

use std::path::Path;

use serde::{Serialize, de::DeserializeOwned};

use crate::error::{Error, Result};

const FENCE: &str = "+++";

pub fn encode<T: Serialize>(frontmatter: &T, body: &str) -> Result<String> {
    // The `body` field lives in the markdown after the closing fence, not
    // inside the TOML. Strip it out of the serialized form here so the
    // type can keep a normal `body: String` field for JSON/in-memory use.
    let value = toml::Value::try_from(frontmatter).map_err(|e| Error::Toml {
        path: Path::new("<frontmatter>").to_path_buf(),
        message: e.to_string(),
    })?;
    let toml::Value::Table(mut table) = value else {
        return Err(Error::Frontmatter {
            path: Path::new("<frontmatter>").to_path_buf(),
            detail: "frontmatter struct must serialize as a TOML table".into(),
        });
    };
    table.remove("body");
    let toml_text = toml::to_string(&toml::Value::Table(table)).map_err(|e| Error::Toml {
        path: Path::new("<frontmatter>").to_path_buf(),
        message: e.to_string(),
    })?;
    let mut out = String::with_capacity(toml_text.len() + body.len() + 16);
    out.push_str(FENCE);
    out.push('\n');
    out.push_str(toml_text.trim_end());
    out.push('\n');
    out.push_str(FENCE);
    out.push('\n');
    out.push_str(body);
    if !body.ends_with('\n') {
        out.push('\n');
    }
    Ok(out)
}

pub fn decode<T: DeserializeOwned>(path: &Path, source: &str) -> Result<(T, String)> {
    let mut lines = source.split_inclusive('\n');

    let opening = lines.next().ok_or_else(|| Error::Frontmatter {
        path: path.to_path_buf(),
        detail: "file is empty".into(),
    })?;
    if opening.trim_end_matches(['\r', '\n']) != FENCE {
        return Err(Error::Frontmatter {
            path: path.to_path_buf(),
            detail: format!("expected leading {FENCE:?} fence, got {opening:?}"),
        });
    }

    let mut toml_text = String::new();
    let mut found_close = false;
    for line in lines.by_ref() {
        if line.trim_end_matches(['\r', '\n']) == FENCE {
            found_close = true;
            break;
        }
        toml_text.push_str(line);
    }
    if !found_close {
        return Err(Error::Frontmatter {
            path: path.to_path_buf(),
            detail: format!("missing closing {FENCE:?} fence"),
        });
    }

    let value = toml::from_str(&toml_text).map_err(|e| Error::Toml {
        path: path.to_path_buf(),
        message: e.to_string(),
    })?;
    let body: String = lines.collect();
    Ok((value, body))
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;

    #[derive(Debug, Serialize, Deserialize, Eq, PartialEq)]
    struct Sample {
        id: String,
        n: u32,
    }

    #[test]
    fn round_trips_simple_frontmatter() {
        let s = Sample {
            id: "abc".into(),
            n: 7,
        };
        let body = "# Hello\n\nworld\n";
        let encoded = encode(&s, body).unwrap();
        assert!(encoded.starts_with("+++\n"));
        let (parsed, parsed_body): (Sample, String) =
            decode(Path::new("test.md"), &encoded).unwrap();
        assert_eq!(parsed, s);
        assert_eq!(parsed_body, body);
    }

    #[test]
    fn rejects_missing_opening_fence() {
        let err = decode::<Sample>(Path::new("x.md"), "id: a\n").unwrap_err();
        assert!(matches!(err, Error::Frontmatter { .. }));
    }

    #[test]
    fn rejects_missing_closing_fence() {
        let err = decode::<Sample>(Path::new("x.md"), "+++\nid = \"a\"\nbody\n").unwrap_err();
        assert!(matches!(err, Error::Frontmatter { .. }));
    }

    #[test]
    fn trailing_newline_is_normalized() {
        let s = Sample {
            id: "abc".into(),
            n: 1,
        };
        let no_trailing = encode(&s, "body").unwrap();
        assert!(no_trailing.ends_with('\n'));
    }
}
