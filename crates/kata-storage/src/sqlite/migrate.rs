//! Embedded SQL migrations applied in order at startup.
//!
//! Each migration is one `Vxxx__name.sql` file in `migrations/`, embedded
//! at compile time via `include_str!`. On startup we read which versions
//! have already run from a `_kata_migrations` table and apply the rest in
//! numeric order, each inside a transaction so a half-applied schema
//! never lands on disk.
//!
//! Adding a new migration: drop a new file in `migrations/` with the next
//! version number and append a [`Migration`] entry to [`MIGRATIONS`].
//! Never edit an old file in place — even an additive change there would
//! mean the migration runs differently on fresh-install vs upgrade.

use rusqlite::{Connection, Transaction};

use crate::error::{Error, Result};

/// One versioned schema step. `version` must be globally unique and
/// monotonically increasing across entries; `name` is informational, used
/// in the audit log so the user can `SELECT * FROM _kata_migrations` and
/// see what ran when.
#[derive(Clone, Copy, Debug)]
pub struct Migration {
    pub version: u32,
    pub name: &'static str,
    pub sql: &'static str,
}

/// All migrations known to this build, in apply order. New entries go at
/// the end with a higher `version`.
pub const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "init",
        sql: include_str!("../../migrations/V001__init.sql"),
    },
    Migration {
        version: 2,
        name: "review_number_and_name",
        sql: include_str!("../../migrations/V002__review_number_and_name.sql"),
    },
    Migration {
        version: 3,
        name: "review_archived_at",
        sql: include_str!("../../migrations/V003__review_archived_at.sql"),
    },
    Migration {
        version: 4,
        name: "review_visits",
        sql: include_str!("../../migrations/V004__review_visits.sql"),
    },
];

/// Bring `conn` up to the latest schema. Idempotent: re-running on an
/// already-current DB is a no-op (just reads the audit table). Designed
/// to be called once at startup, before any other access touches the
/// database — concurrent connections opening against the same file are
/// safe because each migration runs in its own transaction.
pub fn run(conn: &mut Connection) -> Result<()> {
    ensure_audit_table(conn)?;
    let applied = applied_versions(conn)?;
    let pending: Vec<&Migration> = MIGRATIONS
        .iter()
        .filter(|m| !applied.contains(&m.version))
        .collect();

    // Cheap correctness check: the embedded list must be strictly
    // increasing. A duplicate or out-of-order version would otherwise
    // silently produce nondeterministic ordering depending on what's
    // already applied; catch it at startup instead.
    for w in MIGRATIONS.windows(2) {
        if w[0].version >= w[1].version {
            return Err(Error::Sqlite(rusqlite::Error::InvalidQuery));
        }
    }

    for m in pending {
        tracing::info!(version = m.version, name = m.name, "applying migration");
        let tx = conn.transaction()?;
        apply_one(&tx, m)?;
        tx.commit()?;
    }
    Ok(())
}

fn ensure_audit_table(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS _kata_migrations (
            version INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            applied_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
        );",
    )?;
    Ok(())
}

fn applied_versions(conn: &Connection) -> Result<Vec<u32>> {
    let mut stmt = conn.prepare("SELECT version FROM _kata_migrations")?;
    let rows = stmt.query_map([], |row| row.get::<_, u32>(0))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

fn apply_one(tx: &Transaction<'_>, m: &Migration) -> Result<()> {
    tx.execute_batch(m.sql)?;
    tx.execute(
        "INSERT INTO _kata_migrations (version, name) VALUES (?1, ?2)",
        rusqlite::params![m.version, m.name],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem() -> Connection {
        Connection::open_in_memory().unwrap()
    }

    #[test]
    fn fresh_db_applies_all_migrations() {
        let mut conn = mem();
        run(&mut conn).unwrap();
        let applied = applied_versions(&conn).unwrap();
        assert_eq!(applied, MIGRATIONS.iter().map(|m| m.version).collect::<Vec<_>>());
    }

    #[test]
    fn rerun_is_a_noop() {
        let mut conn = mem();
        run(&mut conn).unwrap();
        // Mutate something a migration created, then re-run: if migrations
        // re-applied we'd lose the mutation (or hit a duplicate-table
        // error). Neither should happen.
        conn.execute(
            "INSERT INTO repos (repo_id, canonical_path, schema_version, created_at) \
             VALUES ('r1', '/tmp/r1', 1, '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        run(&mut conn).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM repos", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn audit_log_records_each_migration() {
        let mut conn = mem();
        run(&mut conn).unwrap();
        let rows: Vec<(u32, String)> = conn
            .prepare("SELECT version, name FROM _kata_migrations ORDER BY version")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(
            rows,
            MIGRATIONS
                .iter()
                .map(|m| (m.version, m.name.to_owned()))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn one_draft_per_author_constraint_holds() {
        // V001 declares `sessions_one_draft_per_author` as a partial
        // UNIQUE index. This is the lever that makes
        // open_or_create_session race-free in the SQLite impl; if a
        // future migration breaks it, the storage layer silently allows
        // two open drafts and the bug only shows up under concurrent
        // load — so guard it here.
        let mut conn = mem();
        run(&mut conn).unwrap();
        conn.execute_batch(
            "INSERT INTO repos (repo_id, canonical_path, schema_version, created_at)
                VALUES ('r1', '/tmp/r1', 1, '2026-01-01T00:00:00Z');
             INSERT INTO reviews (repo_id, review_id, schema_version, revset, created_by,
                                  created_at, current_patchset, patchsets_json)
                VALUES ('r1', 'rv1', 1, 'trunk()..rv1', 'a', '2026-01-01T00:00:00Z', 1, '[]');
             INSERT INTO sessions (session_id, repo_id, review_id, schema_version, author,
                                   status, created_at)
                VALUES ('s1', 'r1', 'rv1', 1, 'a', 'draft', '2026-01-01T00:00:00Z');",
        )
        .unwrap();

        // Second draft for the same author should be rejected.
        let res = conn.execute(
            "INSERT INTO sessions (session_id, repo_id, review_id, schema_version, author,
                                   status, created_at)
             VALUES ('s2', 'r1', 'rv1', 1, 'a', 'draft', '2026-01-01T00:00:00Z')",
            [],
        );
        assert!(res.is_err(), "expected uniqueness violation, got {res:?}");

        // But a published session can coexist with a new draft (the
        // index is partial), and a different author can have their own
        // draft on the same review.
        conn.execute(
            "UPDATE sessions SET status='published', published_at='2026-01-01T01:00:00Z'
             WHERE session_id='s1'",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions (session_id, repo_id, review_id, schema_version, author,
                                   status, created_at)
             VALUES ('s2', 'r1', 'rv1', 1, 'a', 'draft', '2026-01-01T02:00:00Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions (session_id, repo_id, review_id, schema_version, author,
                                   status, created_at)
             VALUES ('s3', 'r1', 'rv1', 1, 'b', 'draft', '2026-01-01T02:00:00Z')",
            [],
        )
        .unwrap();
    }
}
