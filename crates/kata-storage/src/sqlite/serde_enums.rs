//! String <-> domain-enum conversions used when binding/extracting SQLite
//! columns. The on-the-wire forms match the serde rename rules in
//! `kata_core::documents` and `kata_core::ids` so the values in the database
//! line up exactly with what the file-based archive format writes (which
//! makes the export/import path a straight copy, no translation).

use kata_core::{Flag, ResolutionAction, SessionStatus, Side};

use crate::error::Error;

pub fn flag_to_str(f: Flag) -> &'static str {
    match f {
        Flag::MustDo => "must-do",
        Flag::Suggestion => "suggestion",
        Flag::Question => "question",
    }
}

pub fn flag_from_str(s: &str) -> Result<Flag, Error> {
    match s {
        "must-do" => Ok(Flag::MustDo),
        "suggestion" => Ok(Flag::Suggestion),
        "question" => Ok(Flag::Question),
        _ => Err(Error::InvalidId {
            label: "flag".into(),
            value: s.into(),
            reason: "unknown flag",
        }),
    }
}

pub fn action_to_str(a: ResolutionAction) -> &'static str {
    match a {
        ResolutionAction::Comment => "comment",
        ResolutionAction::Resolve => "resolve",
        ResolutionAction::Unresolve => "unresolve",
        ResolutionAction::WontFix => "wont-fix",
    }
}

pub fn action_from_str(s: &str) -> Result<ResolutionAction, Error> {
    match s {
        "comment" => Ok(ResolutionAction::Comment),
        "resolve" => Ok(ResolutionAction::Resolve),
        "unresolve" => Ok(ResolutionAction::Unresolve),
        "wont-fix" => Ok(ResolutionAction::WontFix),
        _ => Err(Error::InvalidId {
            label: "resolution_action".into(),
            value: s.into(),
            reason: "unknown action",
        }),
    }
}

pub fn session_status_to_str(s: SessionStatus) -> &'static str {
    match s {
        SessionStatus::Draft => "draft",
        SessionStatus::Published => "published",
        SessionStatus::Discarded => "discarded",
    }
}

/// Inverse of [`session_status_to_str`]. Used when reading rows that
/// preserve every session's status (the archive export path), unlike
/// the trait-level reads that only ever return one known status.
pub fn session_status_to_str_inverse(s: &str) -> Result<SessionStatus, Error> {
    match s {
        "draft" => Ok(SessionStatus::Draft),
        "published" => Ok(SessionStatus::Published),
        "discarded" => Ok(SessionStatus::Discarded),
        _ => Err(Error::InvalidId {
            label: "session_status".into(),
            value: s.into(),
            reason: "unknown status",
        }),
    }
}

pub fn side_to_str(s: Side) -> &'static str {
    match s {
        Side::Base => "base",
        Side::Tip => "tip",
    }
}

pub fn side_from_str(s: &str) -> Result<Side, Error> {
    match s {
        "base" => Ok(Side::Base),
        "tip" => Ok(Side::Tip),
        _ => Err(Error::InvalidId {
            label: "side".into(),
            value: s.into(),
            reason: "unknown side",
        }),
    }
}
