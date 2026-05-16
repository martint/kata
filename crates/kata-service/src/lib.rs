//! Application service — the layer between transports (HTTP, MCP) and the
//! storage + jj backends. Pure async functions; transports adapt requests
//! and responses around them.

pub mod error;
pub mod events;

use std::collections::HashMap;
use std::sync::Arc;

use chrono::Utc;
use kata_core::{
    Author, Bookmark, ChangeId, ChangeStatus, Comment, CommentId, CommitId, CommitInfo, Diff,
    Flag, LineRange, OpSummary, PairDiffCounts, Patchset, PatchsetCompareView,
    PatchsetEndpoints, PatchsetPair, RepoId, RepoSummary, ResolutionAction, Response,
    ResponseId, ReviewId, ReviewManifest, RevSet, SCHEMA_VERSION, Session, SessionId, Side,
};
use kata_jj::{
    AnchorResolution, FileCache, JjBackend, build_diff, build_diff_metadata,
    compute_one_file_hunks, resolve_anchor,
};
use kata_storage::{ReviewSummary, Storage};
use serde::{Deserialize, Serialize};

pub use crate::error::{ServiceError, ServiceResult};
pub use crate::events::{Event, EventBus};

/// `(commit, path)` pairs `resolve_anchor` will need for `comment`,
/// given the patchset currently being rendered. Empty for non-line /
/// non-file comments (no anchoring) and for the trivial case where
/// the comment already anchors to the active commit on its side.
fn anchor_read_keys(comment: &Comment, viewing: &Patchset) -> Vec<(CommitId, String)> {
    let (Some(path), Some(_), Some(side)) = (&comment.file, comment.lines, comment.side)
    else {
        return Vec::new();
    };
    let current = match side {
        Side::Tip => viewing.tip_commit.clone(),
        Side::Base => viewing.base_commit.clone(),
    };
    if current == comment.anchor_commit_id {
        return Vec::new();
    }
    vec![
        (comment.anchor_commit_id.clone(), path.clone()),
        (current, path.clone()),
    ]
}

/// Pair two patchsets' commit lists by jj `change_id`, classifying each
/// pair as Same / Changed / AddedInTo / RemovedFromFrom and emitting one
/// [`PatchsetPair`] per change-id that appears in either side.
///
/// Output order: pairs that exist in the *to* patchset first, in the
/// order `to_commits` lists them (topological, oldest first), then the
/// `RemovedFromFrom` pairs at the end. This keeps the typical reader
/// flow ("what does the new round look like?") at the top of the panel
/// and pushes dropped commits — usually fewer, less interesting at a
/// glance — to the bottom.
fn pair_patchset_commits(
    from_commits: &[CommitInfo],
    to_commits: &[CommitInfo],
) -> Vec<PatchsetPair> {
    use std::collections::HashMap;
    let from_by_change: HashMap<&ChangeId, &CommitInfo> = from_commits
        .iter()
        .map(|c| (&c.change_id, c))
        .collect();
    let to_by_change: HashMap<&ChangeId, &CommitInfo> =
        to_commits.iter().map(|c| (&c.change_id, c)).collect();

    let mut out: Vec<PatchsetPair> = Vec::with_capacity(to_commits.len());
    for to_c in to_commits {
        let from = from_by_change.get(&to_c.change_id).copied();
        let status = match from {
            None => ChangeStatus::AddedInTo,
            Some(f) if f.commit_id == to_c.commit_id => ChangeStatus::Same,
            Some(_) => ChangeStatus::Changed,
        };
        out.push(PatchsetPair {
            change_id: to_c.change_id.clone(),
            status,
            from_commit: from.map(|c| c.commit_id.clone()),
            to_commit: Some(to_c.commit_id.clone()),
            from_description: from.map(|c| c.description_first_line.clone()),
            to_description: Some(to_c.description_first_line.clone()),
            parent_commit: None,
            diff_counts: None,
        });
    }
    for from_c in from_commits {
        if to_by_change.contains_key(&from_c.change_id) {
            continue;
        }
        out.push(PatchsetPair {
            change_id: from_c.change_id.clone(),
            status: ChangeStatus::RemovedFromFrom,
            from_commit: Some(from_c.commit_id.clone()),
            to_commit: None,
            from_description: Some(from_c.description_first_line.clone()),
            to_description: None,
            parent_commit: None,
            diff_counts: None,
        });
    }
    out
}

/// Resolve the (base, tip) commit pair the UI would diff for a given
/// pair-row's "click here for details" action. Mirrors the frontend's
/// `interdiffEndpoints` derivation so the diff-count chip in the
/// side panel matches the diff the user lands on when they click.
/// `None` for `Same` (nothing to count) and as a fallback when the
/// row is missing the commit-ids it needs.
fn effective_endpoints(p: &PatchsetPair) -> Option<(CommitId, CommitId)> {
    match p.status {
        ChangeStatus::Changed => match (&p.from_commit, &p.to_commit) {
            (Some(f), Some(t)) => Some((f.clone(), t.clone())),
            _ => None,
        },
        ChangeStatus::AddedInTo => match (&p.parent_commit, &p.to_commit) {
            (Some(f), Some(t)) => Some((f.clone(), t.clone())),
            _ => None,
        },
        ChangeStatus::RemovedFromFrom => match (&p.parent_commit, &p.from_commit) {
            (Some(f), Some(t)) => Some((f.clone(), t.clone())),
            _ => None,
        },
        ChangeStatus::Same => None,
    }
}

/// Stamp `diff_counts` on every pair that has an effective endpoint
/// pair. Runs `build_diff_metadata` per pair in parallel; the cost
/// per pair is one `jj diff -T template` + per-file blob reads for
/// line counts, same as `build_diff_metadata` everywhere else.
/// Failures leave the field as `None` so the UI just omits the chip
/// rather than failing the whole compare response.
async fn compute_pair_diff_counts<B: JjBackend + ?Sized>(
    backend: &B,
    workspace_path: Option<&std::path::Path>,
    pairs: &mut [PatchsetPair],
) {
    // Split the pairs into two work queues:
    // - one-sided (added/removed) → cheap CLI build_diff_metadata
    //   on (parent..commit). Counts the commit's own contribution.
    // - changed → libjj rebase-based interdiff. The literal
    //   diff(from, to) is wrong for downstream-of-rewrite commits
    //   (it bakes in inherited changes), so we route those through
    //   the in-memory rebase path when a workspace path is available.
    //   Falls back to the CLI path when no workspace is set (test
    //   harness or backends that don't carry a path).
    let mut cli_lookups: Vec<(usize, CommitId, CommitId)> = Vec::new();
    let mut interdiff_lookups: Vec<(usize, CommitId, CommitId)> = Vec::new();
    for (i, p) in pairs.iter().enumerate() {
        let Some((f, t)) = effective_endpoints(p) else { continue };
        match p.status {
            ChangeStatus::Changed if workspace_path.is_some() => {
                interdiff_lookups.push((i, f, t));
            }
            _ => cli_lookups.push((i, f, t)),
        }
    }

    // CLI path: parallel.
    if !cli_lookups.is_empty() {
        let futs = cli_lookups
            .iter()
            .map(|(_, f, t)| build_diff_metadata(backend, f, t));
        let results = futures::future::join_all(futs).await;
        for ((i, _, _), res) in cli_lookups.into_iter().zip(results.into_iter()) {
            if let Ok(diff) = res {
                apply_diff_counts(&mut pairs[i], &diff);
            }
        }
    }

    // libjj path: each call wraps a blocking jj-lib invocation in
    // spawn_blocking. Run them in parallel via try_join_all so
    // multiple Changed pairs don't serialise. The rebase machinery
    // is per-commit; nothing shared across pairs that could
    // benefit from batching.
    if let Some(workspace_path) = workspace_path {
        let workspace_path = workspace_path.to_path_buf();
        let futs = interdiff_lookups.iter().map(|(_, f, t)| {
            let wp = workspace_path.clone();
            let from = f.clone();
            let to = t.clone();
            tokio::task::spawn_blocking(move || -> kata_jj::Result<kata_core::Diff> {
                let handle = kata_jj::libjj::open_repo(&wp)?;
                handle.compute_rebased_diff(&from, &to)
            })
        });
        let results = futures::future::join_all(futs).await;
        for ((i, _, _), res) in interdiff_lookups.into_iter().zip(results.into_iter()) {
            // Outer Result = JoinError; inner = kata_jj::Result.
            // Both failure modes leave diff_counts=None (chip omitted).
            match res {
                Ok(Ok(diff)) => apply_diff_counts(&mut pairs[i], &diff),
                Ok(Err(e)) => {
                    tracing::warn!(error = ?e, "libjj rebased interdiff failed");
                }
                Err(e) => {
                    tracing::warn!(error = ?e, "libjj rebased interdiff task panicked");
                }
            }
        }
    }
}

fn apply_diff_counts(pair: &mut PatchsetPair, diff: &kata_core::Diff) {
    let added = diff.files.iter().map(|f| f.added).sum();
    let removed = diff.files.iter().map(|f| f.removed).sum();
    pair.diff_counts = Some(PairDiffCounts {
        file_count: diff.files.len() as u32,
        added,
        removed,
    });
}

/// Resolve and stamp `parent_commit` on every one-sided pair (the
/// `AddedInTo` / `RemovedFromFrom` entries) so the UI can render their
/// `parent..commit` diff when the user clicks the row. Two-sided
/// pairs (`Same` / `Changed`) don't need a parent — their endpoint
/// pair is already determined by the two commit-ids they carry. A
/// failed parent lookup (e.g. a root commit, or a transient jj
/// error) leaves the field as `None`; the renderer treats those rows
/// as inert in that case rather than failing the whole compare
/// response.
async fn resolve_parents_for_one_sided<B: JjBackend + ?Sized>(
    backend: &B,
    pairs: &mut [PatchsetPair],
) {
    // Collect indices + the commit whose parent we need so we can
    // launch them all in parallel and apply the results in one pass.
    let lookups: Vec<(usize, CommitId)> = pairs
        .iter()
        .enumerate()
        .filter_map(|(i, p)| {
            let commit = match p.status {
                ChangeStatus::AddedInTo => p.to_commit.as_ref(),
                ChangeStatus::RemovedFromFrom => p.from_commit.as_ref(),
                _ => return None,
            };
            commit.map(|c| (i, c.clone()))
        })
        .collect();
    if lookups.is_empty() {
        return;
    }
    // Build the revset strings up-front so each future borrows owned
    // data rather than a temporary `format!()` allocation that would
    // drop at the end of the map closure.
    let revsets: Vec<String> =
        lookups.iter().map(|(_, c)| format!("{c}-")).collect();
    let futures = revsets.iter().map(|r| backend.resolve_endpoint(r));
    let results = futures::future::join_all(futures).await;
    for ((i, _), res) in lookups.into_iter().zip(results.into_iter()) {
        if let Ok(Some(parent)) = res {
            pairs[i].parent_commit = Some(parent.commit_id);
        }
    }
}

/// Result of [`ReviewService::diff_commits`]: either the file-level
/// metadata for a whole commit-pair diff or the hunks for a single
/// file within it, depending on whether a `path` was supplied.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum DiffCommitsResult {
    Diff(Diff),
    File(kata_core::FileChange),
}

/// Internal per-repo entry: friendly name + canonical path + a jj backend
/// rooted at that workspace.
struct RepoEntry {
    summary: RepoSummary,
    jj: Arc<dyn JjBackend>,
}

#[derive(Clone)]
pub struct ReviewService {
    storage: Arc<dyn Storage>,
    /// Per-repo state, looked up by canonical `RepoId`.
    repos: Arc<HashMap<RepoId, RepoEntry>>,
    /// URL slug → canonical repo id. Preserves the order repos were
    /// registered in for `list_repos()`.
    by_name: Arc<Vec<(String, RepoId)>>,
    events: EventBus,
}

/// Builder used at startup to register repos before sealing the service.
pub struct ReviewServiceBuilder {
    storage: Arc<dyn Storage>,
    repos: HashMap<RepoId, RepoEntry>,
    by_name: Vec<(String, RepoId)>,
}

impl ReviewServiceBuilder {
    pub fn new(storage: Arc<dyn Storage>) -> Self {
        Self {
            storage,
            repos: HashMap::new(),
            by_name: Vec::new(),
        }
    }

    /// Register a repository under `name`. Returns an error if either the
    /// name or the repo_id is already registered.
    pub fn add_repo(
        &mut self,
        name: String,
        repo_id: RepoId,
        canonical_path: String,
        jj: Arc<dyn JjBackend>,
    ) -> ServiceResult<()> {
        if self.by_name.iter().any(|(n, _)| n == &name) {
            return Err(ServiceError::BadRequest(format!(
                "duplicate repo name {name:?}",
            )));
        }
        if self.repos.contains_key(&repo_id) {
            return Err(ServiceError::BadRequest(format!(
                "duplicate repo (canonical path {canonical_path:?} already registered)",
            )));
        }
        let summary = RepoSummary {
            name: name.clone(),
            repo_id: repo_id.clone(),
            canonical_path,
        };
        self.repos.insert(repo_id.clone(), RepoEntry { summary, jj });
        self.by_name.push((name, repo_id));
        Ok(())
    }

    pub fn build(self) -> ReviewService {
        ReviewService {
            storage: self.storage,
            repos: Arc::new(self.repos),
            by_name: Arc::new(self.by_name),
            events: events::new_bus(),
        }
    }
}

impl ReviewService {
    pub fn builder(storage: Arc<dyn Storage>) -> ReviewServiceBuilder {
        ReviewServiceBuilder::new(storage)
    }

    /// Public state-change feed. Transports can subscribe via `.subscribe()`
    /// to receive events as other clients make changes.
    pub fn events(&self) -> &EventBus {
        &self.events
    }

    fn emit(&self, event: Event) {
        let _ = self.events.send(event);
    }

    /// Spawn a background task that polls each registered repo on a
    /// timer, comparing every review's recorded patchset endpoints to
    /// the live revset resolution. When the live tip / base differ from
    /// the latest patchset — and the live state has changed since the
    /// last tick — we emit [`Event::ReviewBranchMoved`] so subscribers
    /// (the web UI, mostly) can surface the "Refresh" affordance
    /// without the user reloading the page.
    ///
    /// Cost per tick: one `jj log` per review per repo. For tiny review
    /// counts that's negligible; the IDEAS.md notes call out
    /// concurrency / subscription-scoping if we ever grow past it.
    pub fn spawn_branch_watcher(
        self: Arc<Self>,
        interval: std::time::Duration,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut state: HashMap<(RepoId, ReviewId), (CommitId, CommitId)> =
                HashMap::new();
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            // `tokio::time::interval` fires immediately on the first
            // tick — swallow it so we don't flood the bus the instant
            // the server starts. The first real check happens after
            // `interval`.
            ticker.tick().await;
            loop {
                ticker.tick().await;
                self.branch_watcher_tick(&mut state).await;
            }
        })
    }

    async fn branch_watcher_tick(
        &self,
        state: &mut HashMap<(RepoId, ReviewId), (CommitId, CommitId)>,
    ) {
        let repos: Vec<(String, RepoId)> = self.by_name.as_ref().clone();
        for (repo_name, repo_id) in repos {
            let Ok(summaries) = self.storage.list_reviews(&repo_id).await else {
                continue;
            };
            let jj = match self.jj_for(&repo_id) {
                Ok(j) => j.clone(),
                Err(_) => continue,
            };
            for summary in summaries {
                let review_id = summary.manifest.review_id.clone();
                let Ok(range) = jj.resolve_range(&summary.manifest.revset).await else {
                    continue;
                };
                let cur = summary.manifest.current();
                let live = (range.tip.commit_id, range.base.commit_id);
                let stale = live.0 != cur.tip_commit || live.1 != cur.base_commit;
                let key = (repo_id.clone(), review_id.clone());
                let prev = state.insert(key, live.clone());
                // Emit when the review is stale AND the live endpoints
                // moved since the last tick we saw. That covers:
                //   - first time we see this review and it's already stale;
                //   - amend → amend → amend (each new tip re-pings the UI);
                //   - skip when nothing actually changed since last poll.
                if stale && prev.as_ref() != Some(&live) {
                    let _ = self.events.send(Event::ReviewBranchMoved {
                        repo: repo_name.clone(),
                        review_id,
                    });
                }
            }
        }
    }

    /// All registered repos, in registration order.
    pub fn list_repos(&self) -> Vec<RepoSummary> {
        self.by_name
            .iter()
            .filter_map(|(_, id)| self.repos.get(id).map(|e| e.summary.clone()))
            .collect()
    }

    /// Resolve a URL-slug to its canonical [`RepoId`].
    pub fn resolve_repo(&self, name: &str) -> ServiceResult<RepoId> {
        self.by_name
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, id)| id.clone())
            .ok_or_else(|| ServiceError::NotFound(format!("repo {name:?}")))
    }

    /// Friendly name of a registered repo (inverse of `resolve_repo`).
    pub fn repo_name(&self, repo: &RepoId) -> Option<String> {
        self.repos.get(repo).map(|e| e.summary.name.clone())
    }

    fn entry(&self, repo: &RepoId) -> ServiceResult<&RepoEntry> {
        self.repos
            .get(repo)
            .ok_or_else(|| ServiceError::NotFound(format!("repo {repo}")))
    }

    fn jj_for(&self, repo: &RepoId) -> ServiceResult<&Arc<dyn JjBackend>> {
        Ok(&self.entry(repo)?.jj)
    }

    // ---- repo / bookmarks ----------------------------------------------

    pub async fn list_bookmarks(&self, repo: &RepoId) -> ServiceResult<Vec<Bookmark>> {
        Ok(self.jj_for(repo)?.list_bookmarks().await?)
    }

    /// Try to resolve `expr` as a revset and report how many commits
    /// it contains. Used by the new-review form to warn before the
    /// user creates a review with an empty diff (the bookmark IS the
    /// trunk, the range is `nothing..something`, the user fat-fingered
    /// the syntax, etc.). jj process failures (bad syntax, ambiguous
    /// prefix, missing revision) come back as `BadRequest` with jj's
    /// stderr cleaned of its 'Error:' framing — the form surfaces
    /// the result inline, so the message has to read as user-facing
    /// rather than process-failure.
    pub async fn preview_revset(
        &self,
        repo: &RepoId,
        expr: &str,
    ) -> ServiceResult<usize> {
        let revset = kata_core::RevSet::new(expr);
        let commits = self
            .jj_for(repo)?
            .list_commits(&revset)
            .await
            .map_err(|e| match e {
                kata_jj::Error::JjFailed { stderr, .. } => {
                    ServiceError::BadRequest(clean_jj_stderr(&stderr))
                }
                other => ServiceError::Jj(other),
            })?;
        Ok(commits.len())
    }

    pub async fn list_reviews(&self, repo: &RepoId) -> ServiceResult<Vec<ReviewSummary>> {
        Ok(self.storage.list_reviews(repo).await?)
    }

    /// Resolve the per-repo `number` carried in URLs to the opaque
    /// `ReviewId` that every other API surface uses internally. Errors
    /// with `NotFound` when no review with that number exists.
    pub async fn resolve_review_number(
        &self,
        repo: &RepoId,
        number: u32,
    ) -> ServiceResult<ReviewId> {
        self.storage
            .resolve_review_number(repo, number)
            .await?
            .ok_or_else(|| ServiceError::NotFound(format!("review #{number}")))
    }

    // ---- review lifecycle ----------------------------------------------

    pub async fn create_review(
        &self,
        repo: &RepoId,
        params: CreateReviewParams,
    ) -> ServiceResult<ReviewManifest> {
        let jj = self.jj_for(repo)?;
        let CreateReviewParams {
            name,
            revset,
            bookmark,
            created_by,
            summary,
        } = params;
        let range = jj.resolve_range(&revset).await?;
        let now = Utc::now();
        // Server-generated internal id. The user-facing identifier is
        // the per-repo `number` that storage assigns inside the
        // create_review transaction.
        let review_id = kata_storage::ids::new_review_id();
        let manifest = ReviewManifest {
            schema_version: SCHEMA_VERSION,
            review_id,
            number: 0, // storage assigns
            name,
            revset,
            created_at: now,
            created_by,
            bookmark,
            summary: summary.filter(|s| !s.is_empty()),
            patchsets: vec![Patchset {
                n: 1,
                base_change: range.base.change_id,
                base_commit: range.base.commit_id,
                tip_change: range.tip.change_id,
                tip_commit: range.tip.commit_id,
                recorded_at: now,
                parent_patchset: None,
            }],
            current_patchset: 1,
            archived_at: None,
        };
        let manifest = self.storage.create_review(repo, &manifest).await?;
        let repo_name = self.repo_name(repo).unwrap_or_default();
        self.emit(Event::ReviewCreated {
            repo: repo_name,
            review_id: manifest.review_id.clone(),
        });
        Ok(manifest)
    }

    /// Open a review for viewing. `patchset` selects which round to display;
    /// `None` means the latest. The diff is built against that patchset's
    /// endpoints, and comments are filtered to those that originated in it
    /// or an earlier patchset.
    ///
    /// `compare`, when set, swaps the diff's *base* for the named
    /// patchset's tip — so the response shows what changed between
    /// patchset *compare* and patchset *patchset*, instead of the
    /// usual base..tip. Comments, anchors, and the commits list are
    /// still scoped to the destination patchset; only the file/hunk
    /// diff changes.
    ///
    /// Anchor pre-fetch runs with `ANCHOR_READ_PARALLELISM` reads in
    /// flight — see the constant below.
    pub async fn open_review(
        &self,
        repo: &RepoId,
        review: &ReviewId,
        viewer: &Author,
        patchset: Option<u32>,
        compare: Option<u32>,
    ) -> ServiceResult<ReviewView> {
        let jj = self.jj_for(repo)?;
        let manifest = self.storage.open_review(repo, review).await?;

        let selected_n = patchset.unwrap_or(manifest.current_patchset);
        let selected = manifest
            .patchset(selected_n)
            .ok_or_else(|| ServiceError::NotFound(format!("patchset {selected_n}")))?
            .clone();

        // The "from" side of a patchset-compare diff. `None` for the
        // normal base..tip view; `Some` for compare mode.
        let compare_base = match compare {
            None => None,
            Some(n) if n == selected_n => {
                return Err(ServiceError::NotFound(format!(
                    "cannot compare patchset {n} with itself"
                )));
            }
            Some(n) => Some(
                manifest
                    .patchset(n)
                    .ok_or_else(|| ServiceError::NotFound(format!("patchset {n}")))?
                    .tip_commit
                    .clone(),
            ),
        };
        let diff_base = compare_base.as_ref().unwrap_or(&selected.base_commit);

        // The commits panel enumerates `diff_base..selected.tip_commit` —
        // built from immutable commit IDs the manifest pinned at create /
        // refresh time, so the listing is stable regardless of what the
        // live revset evaluates to today (or whether it evaluates at all).
        // Also matches the diff metadata above, which renders the same
        // pair of endpoints.
        let commits_revset = kata_core::RevSet::new(format!(
            "{}..{}",
            diff_base, selected.tip_commit,
        ));

        // `live_range` lets us tell the UI whether re-resolving the revset
        // would advance the latest patchset (the "is_stale" flag below).
        // We resolve here, in parallel with the diff/commit work, to avoid
        // paying for a separate round-trip.
        // Metadata only — hunks ship lazily, one file at a time, via
        // `/file-diff`. Keeps the open_review JSON tiny so the
        // browser's `JSON.parse` stays under ~10 ms instead of the
        // ~1 s it took when the whole diff was inlined.
        //
        // `live_range` uses the live revset and is allowed to fail (e.g.
        // the revset references a change ID that's gone divergent); we
        // fall back to "not stale" rather than failing the whole open.
        let (diff_res, commits_res, live_res, current_op_res) = tokio::join!(
            build_diff_metadata(&**jj, diff_base, &selected.tip_commit),
            jj.list_commits(&commits_revset),
            jj.resolve_range(&manifest.revset),
            jj.current_op_id(),
        );
        let diff = diff_res?;
        let commits = commits_res?;
        let revset_error = match &live_res {
            Err(e) => Some(build_revset_error(&**jj, e).await),
            Ok(_) => None,
        };
        let live_range = live_res.ok();

        // "Since you were here": diff the current jj op-id against the
        // op-id we recorded the last time this viewer opened this review.
        // First visit (`None` from storage) shows no list; we just record
        // the current op-id so the *next* visit has a baseline. A failure
        // to read the op-id is treated as "skip the feature for this
        // open" — it's never load-bearing.
        // Read the previous visit BEFORE recording the new one so we
        // have a stable baseline for both the op-log "since you were
        // here" diff (jj operations) and the unread-replies signal on
        // comments (responses landed after `prev.visited_at`).
        let prev_visit = if viewer.as_str().is_empty() {
            None
        } else {
            self.storage
                .last_review_visit(repo, review, viewer)
                .await
                .ok()
                .flatten()
        };
        let ops_since = match (&current_op_res, viewer.as_str().is_empty()) {
            (Ok(current_op), false) => {
                let list = match prev_visit.as_ref() {
                    Some(prev) => {
                        jj.ops_between(&prev.op_id, current_op).await.unwrap_or_default()
                    }
                    None => Vec::new(),
                };
                if let Err(e) = self
                    .storage
                    .record_review_visit(repo, review, viewer, current_op)
                    .await
                {
                    // Recording the baseline is best-effort — losing it
                    // just means the next open shows an empty
                    // "since you were here" list instead of failing the
                    // open. But silently swallowing the error is what
                    // hid a broken FK in this code path for weeks, so
                    // log it loudly.
                    tracing::warn!(error = ?e, "failed to record review visit");
                }
                list
            }
            _ => Vec::new(),
        };
        let last_visit_at = prev_visit.map(|p| p.visited_at);

        let latest = manifest.current();
        let is_stale = match &live_range {
            Some(r) => {
                r.tip.commit_id != latest.tip_commit
                    || r.base.commit_id != latest.base_commit
            }
            None => false,
        };

        let (published, responses, drafts) = tokio::try_join!(
            self.storage.list_published_comments(repo, review),
            self.storage.list_published_responses(repo, review),
            self.storage.list_drafts_for(repo, review, viewer),
        )?;

        // Many comments resolve against the same `(commit, path)` — every
        // line/file comment on a given file needs both its anchor_commit
        // and the current patchset endpoint. Read each pair once, in
        // parallel, then let `resolve_anchor` hit the cache.
        let cache = FileCache::new();
        let prefetch_keys: std::collections::HashSet<(CommitId, String)> = published
            .iter()
            .filter(|c| c.patchset <= selected_n)
            .chain(drafts.comments.iter().filter(|c| c.patchset <= selected_n))
            .flat_map(|c| anchor_read_keys(c, &selected))
            .collect();
        cache.prefetch(&**jj, prefetch_keys).await?;
        let mut comments = Vec::with_capacity(published.len());
        for c in published {
            if c.patchset > selected_n {
                continue;
            }
            comments.push(
                self.build_comment_view(repo, &cache, c, &selected, false)
                    .await?,
            );
        }
        let mut draft_comments = Vec::with_capacity(drafts.comments.len());
        for c in drafts.comments {
            if c.patchset > selected_n {
                continue;
            }
            draft_comments.push(
                self.build_comment_view(repo, &cache, c, &selected, true)
                    .await?,
            );
        }

        let response_views: Vec<ResponseView> = responses
            .into_iter()
            .map(|r| ResponseView { response: r, draft: false })
            .collect();
        let draft_response_views: Vec<ResponseView> = drafts
            .responses
            .into_iter()
            .map(|r| ResponseView { response: r, draft: true })
            .collect();

        Ok(ReviewView {
            manifest,
            diff,
            commits,
            comments,
            responses: response_views,
            drafts: DraftsView {
                session: drafts.session,
                comments: draft_comments,
                responses: draft_response_views,
            },
            is_stale,
            revset_error,
            ops_since,
            last_visit_at,
        })
    }

    async fn build_comment_view(
        &self,
        repo: &RepoId,
        cache: &FileCache,
        comment: Comment,
        viewing: &Patchset,
        draft: bool,
    ) -> ServiceResult<CommentView> {
        let jj = self.jj_for(repo)?;
        let anchor = match (&comment.file, comment.lines, comment.side) {
            (Some(path), Some(range), Some(side)) => {
                let current = match side {
                    Side::Tip => &viewing.tip_commit,
                    Side::Base => &viewing.base_commit,
                };
                match resolve_anchor(
                    &**jj,
                    cache,
                    path,
                    &comment.anchor_commit_id,
                    range,
                    current,
                )
                .await?
                {
                    AnchorResolution::Valid => AnchorView::Valid,
                    AnchorResolution::Moved { new_range } => {
                        AnchorView::Moved { new_lines: new_range }
                    }
                    AnchorResolution::Drifted { new_range, similarity } => {
                        AnchorView::Drifted {
                            new_lines: new_range,
                            similarity,
                        }
                    }
                    AnchorResolution::Outdated { original_content } => {
                        AnchorView::Outdated { original_content }
                    }
                }
            }
            // Whole-file or whole-review comments have nothing to re-anchor.
            _ => AnchorView::Valid,
        };
        Ok(CommentView { comment, anchor, draft })
    }

    /// Hunks for one file in a review. Used by the UI to lazy-load a
    /// file's diff as it scrolls into view — open_review ships only
    /// the file list, then the client requests this for each visible
    /// `FileSlot`. `patchset` follows the same shape as `open_review`:
    /// `None` = the manifest's current patchset. `compare` — same
    /// semantics as in `open_review` — swaps the base for the named
    /// patchset's tip so the hunks describe the patchset→patchset
    /// delta rather than base..tip.
    pub async fn file_diff(
        &self,
        repo: &RepoId,
        review: &ReviewId,
        path: &str,
        patchset: Option<u32>,
        compare: Option<u32>,
    ) -> ServiceResult<kata_core::FileChange> {
        let jj = self.jj_for(repo)?;
        let manifest = self.storage.open_review(repo, review).await?;
        let selected_n = patchset.unwrap_or(manifest.current_patchset);
        let selected = manifest
            .patchset(selected_n)
            .ok_or_else(|| ServiceError::NotFound(format!("patchset {selected_n}")))?;
        let compare_base = match compare {
            None => None,
            Some(n) if n == selected_n => {
                return Err(ServiceError::NotFound(format!(
                    "cannot compare patchset {n} with itself"
                )));
            }
            Some(n) => Some(
                manifest
                    .patchset(n)
                    .ok_or_else(|| ServiceError::NotFound(format!("patchset {n}")))?
                    .tip_commit
                    .clone(),
            ),
        };
        let base = compare_base.as_ref().unwrap_or(&selected.base_commit);
        // Look up the file's metadata (status, rename info) — needed so
        // we know which side(s) to read. One `jj diff -T template` call,
        // ~50 ms; could be cached if it becomes a hot path.
        let files = jj.changed_files(base, &selected.tip_commit).await?;
        let target = files
            .into_iter()
            .find(|f| f.path == path)
            .ok_or_else(|| ServiceError::NotFound(format!("file {path:?} in review")))?;
        let updated =
            compute_one_file_hunks(&**jj, base, &selected.tip_commit, target).await?;
        Ok(updated)
    }

    /// Read a file at a specific commit as text. Returns NotFound if the
    /// file doesn't exist at that commit.
    pub async fn read_file_text(
        &self,
        repo: &RepoId,
        commit: &CommitId,
        path: &str,
    ) -> ServiceResult<String> {
        let jj = self.jj_for(repo)?;
        match jj.read_file(commit, path).await? {
            Some(bytes) => Ok(String::from_utf8_lossy(&bytes).into_owned()),
            None => Err(ServiceError::NotFound(format!("{path} at {commit}"))),
        }
    }

    /// Build the diff for a single commit (parent-of-change..change). Used
    /// when the UI scopes a review view down to one commit. Returns both
    /// change ids alongside the diff so the UI can read each side's
    /// file content (for syntax highlighting and anchor resolution) at
    /// the right commit — not at the whole-review patchset's tip, which
    /// can have completely different line numbers when later commits in
    /// the stack touch the same file.
    pub async fn commit_diff(
        &self,
        repo: &RepoId,
        change: &ChangeId,
    ) -> ServiceResult<CommitDiffView> {
        let jj = self.jj_for(repo)?;
        let tip_commit = jj
            .change_to_commit(change)
            .await?
            .ok_or_else(|| ServiceError::NotFound(format!("change {change}")))?;
        // Drive the parent lookup from the resolved commit ID, not the
        // change ID — commit IDs are immutable and can't be divergent,
        // so this stays correct even when the change has multiple
        // visible siblings (and `change_to_commit` already picked one
        // for us).
        let parent = jj
            .resolve_endpoint(&format!("{tip_commit}-"))
            .await?
            .ok_or_else(|| ServiceError::NotFound(format!("parent of change {change}")))?;
        let diff = build_diff(&**jj, &parent.commit_id, &tip_commit).await?;
        Ok(CommitDiffView {
            base_change: parent.change_id,
            base_commit: parent.commit_id,
            tip_change: change.clone(),
            tip_commit,
            files: diff.files,
        })
    }

    /// Build the patchset-compare v2 view: the cumulative tree-vs-tree
    /// diff between two patchsets (same shape as today's compare-mode
    /// in `open_review`) plus a per-change-id pair list that lets the
    /// UI attribute every diff to a specific commit.
    ///
    /// Pairing is by jj `change_id`: a `change_id` present in both
    /// patchsets is `Same` (matching commit-ids) or `Changed` (the
    /// author rewrote it). One-sided change-ids become
    /// `AddedInTo` / `RemovedFromFrom`. The UI uses these statuses to
    /// pick interaction (clickable vs inert) and to fetch the right
    /// per-commit interdiff on demand.
    ///
    /// Per-commit interdiff *content* is **not** included here — the
    /// pair list ships only commit-ids + first-line descriptions. The
    /// frontend fetches the actual file diff for a `Changed` row via
    /// [`Self::diff_commits`].
    pub async fn compare_patchsets(
        &self,
        repo: &RepoId,
        review: &ReviewId,
        from_n: u32,
        to_n: u32,
    ) -> ServiceResult<PatchsetCompareView> {
        if from_n == to_n {
            return Err(ServiceError::BadRequest(format!(
                "cannot compare patchset {from_n} with itself"
            )));
        }
        let jj = self.jj_for(repo)?;
        let manifest = self.storage.open_review(repo, review).await?;
        let from_ps = manifest
            .patchset(from_n)
            .ok_or_else(|| ServiceError::NotFound(format!("patchset {from_n}")))?
            .clone();
        let to_ps = manifest
            .patchset(to_n)
            .ok_or_else(|| ServiceError::NotFound(format!("patchset {to_n}")))?
            .clone();

        // List both patchsets' commits and compute the cumulative diff
        // metadata in parallel — three independent jj calls, one
        // round-trip cost.
        let from_revset =
            RevSet::new(format!("{}..{}", from_ps.base_commit, from_ps.tip_commit));
        let to_revset =
            RevSet::new(format!("{}..{}", to_ps.base_commit, to_ps.tip_commit));
        let (from_commits_res, to_commits_res, cumulative_res) = tokio::join!(
            jj.list_commits(&from_revset),
            jj.list_commits(&to_revset),
            build_diff_metadata(&**jj, &from_ps.tip_commit, &to_ps.tip_commit),
        );
        let from_commits = from_commits_res?;
        let to_commits = to_commits_res?;
        let cumulative = cumulative_res?;

        let mut pairs = pair_patchset_commits(&from_commits, &to_commits);
        // For AddedInTo / RemovedFromFrom rows: resolve the parent of
        // the present-side commit so the UI can render the commit's
        // own parent..commit diff when clicked. Two-sided rows
        // (Same / Changed) skip this — they already carry both
        // endpoints. Failures leave parent_commit=None; the row falls
        // back to inert rather than the whole response erroring.
        resolve_parents_for_one_sided(&**jj, &mut pairs).await;
        // Then compute per-pair diff counts in parallel so the side
        // panel can show "3 files +7 −15" inline. Sequential after
        // parent resolution because added/removed pairs use the
        // resolved parent as one endpoint.
        let workspace_path = std::path::PathBuf::from(&self.entry(repo)?.summary.canonical_path)
            .parent()
            .and_then(|p| p.parent())
            .map(|p| p.to_path_buf());
        compute_pair_diff_counts(&**jj, workspace_path.as_deref(), &mut pairs).await;

        let compare_base_mismatch = from_ps.base_commit != to_ps.base_commit;
        Ok(PatchsetCompareView {
            from: PatchsetEndpoints {
                n: from_n,
                base_commit: from_ps.base_commit,
                tip_commit: from_ps.tip_commit,
            },
            to: PatchsetEndpoints {
                n: to_n,
                base_commit: to_ps.base_commit,
                tip_commit: to_ps.tip_commit,
            },
            compare_base_mismatch,
            cumulative,
            pairs,
        })
    }

    /// Rebase-based interdiff between two commits (libjj path).
    /// Computes `diff(rebase(from_commit onto to_commit-), to_commit)`
    /// in-memory without touching the user's workspace. Use this for
    /// `Changed` pair rows in the patchset-compare v2 view; the
    /// naive [`Self::diff_commits`] gives wrong results when the
    /// stack has been rewritten because it bakes inherited downstream
    /// changes into every commit's reported diff.
    ///
    /// Runs inside `spawn_blocking` because jj-lib is synchronous and
    /// the operation involves file I/O against the jj store.
    pub async fn interdiff_commits(
        &self,
        repo: &RepoId,
        from: &CommitId,
        to: &CommitId,
        path: Option<&str>,
    ) -> ServiceResult<DiffCommitsResult> {
        let entry = self.entry(repo)?;
        // The canonical path stored at registration is `.jj/repo`;
        // the workspace root jj-lib expects is the directory two
        // levels up. Computed each call — cheap and avoids
        // long-lived cached state that could go stale across jj
        // operations.
        let workspace_path = std::path::PathBuf::from(&entry.summary.canonical_path)
            .parent()
            .and_then(|p| p.parent())
            .ok_or_else(|| {
                ServiceError::BadRequest(format!(
                    "cannot derive workspace path from {}",
                    entry.summary.canonical_path
                ))
            })?
            .to_path_buf();
        let from = from.clone();
        let to = to.clone();
        let path = path.map(|s| s.to_owned());
        tokio::task::spawn_blocking(move || -> kata_jj::Result<DiffCommitsResult> {
            let handle = kata_jj::libjj::open_repo(&workspace_path)?;
            match path {
                None => {
                    let diff = handle.compute_rebased_diff(&from, &to)?;
                    Ok(DiffCommitsResult::Diff(diff))
                }
                Some(p) => {
                    let file = handle.compute_rebased_file_hunks(&from, &to, &p)?;
                    Ok(DiffCommitsResult::File(file))
                }
            }
        })
        .await
        .map_err(|e| ServiceError::Internal(format!("interdiff task join: {e}")))?
        .map_err(ServiceError::from)
    }

    /// Generic commit-pair diff. Without `path`: file-level metadata
    /// for the entire diff. With `path`: full hunks for that single
    /// file (same shape as [`Self::file_diff`] but addressed by
    /// commit-id, not patchset-id).
    ///
    /// This is the per-commit interdiff source for the patchset-compare
    /// v2 view; it's also useful in any context where the UI already
    /// knows two commit-ids and wants the diff between them without
    /// dragging in patchset bookkeeping.
    pub async fn diff_commits(
        &self,
        repo: &RepoId,
        from: &CommitId,
        to: &CommitId,
        path: Option<&str>,
    ) -> ServiceResult<DiffCommitsResult> {
        let jj = self.jj_for(repo)?;
        match path {
            None => {
                let diff = build_diff_metadata(&**jj, from, to).await?;
                Ok(DiffCommitsResult::Diff(diff))
            }
            Some(p) => {
                let files = jj.changed_files(from, to).await?;
                let target = files
                    .into_iter()
                    .find(|f| f.path == p)
                    .ok_or_else(|| {
                        ServiceError::NotFound(format!(
                            "file {p:?} in diff {from}..{to}"
                        ))
                    })?;
                let updated = compute_one_file_hunks(&**jj, from, to, target).await?;
                Ok(DiffCommitsResult::File(updated))
            }
        }
    }

    /// Re-resolve the revset. If the tip has moved since the current
    /// patchset was recorded, append a new patchset and make it current.
    /// Optionally also replace the summary in the same call — only the
    /// review's `created_by` author may do so; non-creators passing a
    /// summary are rejected.
    pub async fn refresh_review(
        &self,
        repo: &RepoId,
        review: &ReviewId,
        actor: &Author,
        new_summary: Option<String>,
    ) -> ServiceResult<ReviewManifest> {
        let jj = self.jj_for(repo)?;
        let mut manifest = self.storage.open_review(repo, review).await?;
        if new_summary.is_some() && actor != &manifest.created_by {
            return Err(ServiceError::BadRequest(
                "only the review's creator can update its summary".into(),
            ));
        }
        let range = jj.resolve_range(&manifest.revset).await?;
        let current = manifest.current().clone();
        let tip_moved = range.tip.commit_id != current.tip_commit
            || range.base.commit_id != current.base_commit;
        if !tip_moved && new_summary.is_none() {
            return Ok(manifest);
        }
        if tip_moved {
            // A new patchset is a *continuation* of the previous one when
            // EITHER:
            //   * the new tip is a descendant of the old tip (normal
            //     fast-forward: new commits stacked on top), OR
            //   * the new tip's change_id matches the old tip's change_id
            //     (the author amended the tip in place — same change in
            //     jj's identity model, different commit_id under it).
            //
            // We used to check only the first. That conflated "the
            // author edited a commit" (the *common* case in jj) with
            // "the author abandoned the branch and started over" — both
            // showed up as `parent_patchset: None` and were labelled
            // "rewritten" in the UI. Now `parent_patchset` is None only
            // when neither signal holds, i.e. genuine history rewrite.
            let same_tip_change = range.tip.change_id == current.tip_change;
            let descends = jj
                .is_ancestor(&current.tip_commit, &range.tip.commit_id)
                .await?;
            let parent_patchset = if same_tip_change || descends {
                Some(current.n)
            } else {
                None
            };
            let next_n = manifest.patchsets.iter().map(|p| p.n).max().unwrap_or(0) + 1;
            manifest.patchsets.push(Patchset {
                n: next_n,
                base_change: range.base.change_id,
                base_commit: range.base.commit_id,
                tip_change: range.tip.change_id,
                tip_commit: range.tip.commit_id,
                recorded_at: Utc::now(),
                parent_patchset,
            });
            manifest.current_patchset = next_n;
        }
        if let Some(s) = new_summary {
            manifest.summary = Some(s).filter(|s| !s.is_empty());
        }
        self.storage.update_review(repo, &manifest).await?;
        let repo_name = self.repo_name(repo).unwrap_or_default();
        self.emit(Event::ReviewUpdated {
            repo: repo_name,
            review_id: manifest.review_id.clone(),
        });
        Ok(manifest)
    }

    /// Replace the review's free-text summary. Only the `created_by`
    /// author may call this. Passing `None` (or an empty string) clears
    /// the summary.
    pub async fn update_review_summary(
        &self,
        repo: &RepoId,
        review: &ReviewId,
        actor: &Author,
        summary: Option<String>,
    ) -> ServiceResult<ReviewManifest> {
        let mut manifest = self.storage.open_review(repo, review).await?;
        if actor != &manifest.created_by {
            return Err(ServiceError::BadRequest(
                "only the review's creator can update its summary".into(),
            ));
        }
        manifest.summary = summary.filter(|s| !s.is_empty());
        self.storage.update_review(repo, &manifest).await?;
        let repo_name = self.repo_name(repo).unwrap_or_default();
        self.emit(Event::ReviewUpdated {
            repo: repo_name,
            review_id: manifest.review_id.clone(),
        });
        Ok(manifest)
    }

    /// Flip the review's archived state. `archived = true` records the
    /// archive timestamp; `false` clears it. Only the review's creator
    /// may call this (the home-screen Archive button is hidden for
    /// other viewers). The new manifest is returned and a
    /// [`Event::ReviewUpdated`] is emitted so other tabs refresh.
    pub async fn set_review_archived(
        &self,
        repo: &RepoId,
        review: &ReviewId,
        actor: &Author,
        archived: bool,
    ) -> ServiceResult<ReviewManifest> {
        let mut manifest = self.storage.open_review(repo, review).await?;
        if actor != &manifest.created_by {
            return Err(ServiceError::BadRequest(
                "only the review's creator can archive or unarchive it".into(),
            ));
        }
        let already = manifest.archived_at.is_some();
        if already == archived {
            return Ok(manifest);
        }
        manifest.archived_at = if archived { Some(Utc::now()) } else { None };
        self.storage.update_review(repo, &manifest).await?;
        let repo_name = self.repo_name(repo).unwrap_or_default();
        self.emit(Event::ReviewUpdated {
            repo: repo_name,
            review_id: manifest.review_id.clone(),
        });
        Ok(manifest)
    }

    // ---- sessions ------------------------------------------------------

    pub async fn start_session(
        &self,
        repo: &RepoId,
        review: &ReviewId,
        author: &Author,
    ) -> ServiceResult<Session> {
        // Archived reviews are read-only — block at start_session so the
        // downstream draft-comment / draft-response paths can't be hit.
        // Authors with an already-open draft are unaffected; only the
        // creator can archive, and they presumably know they shouldn't.
        let manifest = self.storage.open_review(repo, review).await?;
        if manifest.archived_at.is_some() {
            return Err(ServiceError::BadRequest(
                "review is archived; unarchive before adding new comments".into(),
            ));
        }
        Ok(self
            .storage
            .open_or_create_session(repo, review, author)
            .await?)
    }

    pub async fn publish_session(
        &self,
        repo: &RepoId,
        review: &ReviewId,
        session: &SessionId,
    ) -> ServiceResult<()> {
        self.storage
            .publish_session(repo, review, session)
            .await?;
        let repo_name = self.repo_name(repo).unwrap_or_default();
        self.emit(Event::SessionPublished {
            repo: repo_name,
            review_id: review.clone(),
            session_id: session.clone(),
        });
        Ok(())
    }

    pub async fn discard_session(
        &self,
        repo: &RepoId,
        review: &ReviewId,
        session: &SessionId,
    ) -> ServiceResult<()> {
        self.storage
            .discard_session(repo, review, session)
            .await?;
        let repo_name = self.repo_name(repo).unwrap_or_default();
        self.emit(Event::SessionDiscarded {
            repo: repo_name,
            review_id: review.clone(),
            session_id: session.clone(),
        });
        Ok(())
    }

    // ---- comments ------------------------------------------------------

    pub async fn upsert_draft_comment(
        &self,
        repo: &RepoId,
        review: &ReviewId,
        session: &SessionId,
        author: &Author,
        comment_id: Option<CommentId>,
        input: DraftCommentInput,
    ) -> ServiceResult<Comment> {
        let comment_id = comment_id.unwrap_or_else(kata_storage::ids::new_comment_id);
        validate_anchor(&input)?;
        let manifest = self.storage.open_review(repo, review).await?;
        let comment = Comment {
            schema_version: SCHEMA_VERSION,
            comment_id,
            session_id: session.clone(),
            review_id: review.clone(),
            author: author.clone(),
            created_at: Utc::now(),
            patchset: manifest.current_patchset,
            anchor_change_id: input.anchor_change_id,
            anchor_commit_id: input.anchor_commit_id,
            file: input.file,
            side: input.side,
            lines: input.lines,
            review_wide: input.review_wide,
            flag: input.flag,
            body: input.body,
        };
        self.storage.upsert_draft_comment(repo, &comment).await?;
        Ok(comment)
    }

    /// Edit the body / flag of an existing draft comment without making
    /// the caller re-supply the anchor. Looks up the draft in the
    /// author's open session, rebuilds the input with the new fields,
    /// and delegates to [`Self::upsert_draft_comment`].
    pub async fn update_draft_comment(
        &self,
        repo: &RepoId,
        review: &ReviewId,
        author: &Author,
        comment_id: &CommentId,
        body: String,
        flag: Flag,
    ) -> ServiceResult<Comment> {
        let drafts = self.storage.list_drafts_for(repo, review, author).await?;
        let existing = drafts
            .comments
            .iter()
            .find(|c| &c.comment_id == comment_id)
            .ok_or_else(|| {
                ServiceError::NotFound(format!("draft comment {comment_id} for {author}"))
            })?;
        let session = existing.session_id.clone();
        let input = DraftCommentInput {
            anchor_change_id: existing.anchor_change_id.clone(),
            anchor_commit_id: existing.anchor_commit_id.clone(),
            file: existing.file.clone(),
            side: existing.side.clone(),
            lines: existing.lines.clone(),
            review_wide: existing.review_wide,
            flag,
            body,
        };
        self.upsert_draft_comment(repo, review, &session, author, Some(comment_id.clone()), input)
            .await
    }

    pub async fn discard_draft_comment(
        &self,
        repo: &RepoId,
        review: &ReviewId,
        session: &SessionId,
        comment: &CommentId,
    ) -> ServiceResult<()> {
        Ok(self
            .storage
            .discard_draft_comment(repo, review, session, comment)
            .await?)
    }

    // ---- responses -----------------------------------------------------

    pub async fn upsert_draft_response(
        &self,
        repo: &RepoId,
        session: &SessionId,
        author: &Author,
        response_id: Option<ResponseId>,
        input: DraftResponseInput,
    ) -> ServiceResult<Response> {
        let response_id = response_id.unwrap_or_else(kata_storage::ids::new_response_id);
        let response = Response {
            schema_version: SCHEMA_VERSION,
            response_id,
            in_reply_to: input.in_reply_to,
            session_id: session.clone(),
            author: author.clone(),
            created_at: Utc::now(),
            action: input.action,
            body: input.body,
        };
        self.storage
            .upsert_draft_response(repo, &response)
            .await?;
        Ok(response)
    }

    pub async fn discard_draft_response(
        &self,
        repo: &RepoId,
        review: &ReviewId,
        session: &SessionId,
        response: &ResponseId,
    ) -> ServiceResult<()> {
        Ok(self
            .storage
            .discard_draft_response(repo, review, session, response)
            .await?)
    }
}

// ---- request shapes ----------------------------------------------------

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CreateReviewParams {
    /// Human-readable name. Stored on the manifest as
    /// [`ReviewManifest::name`]; the internal id is generated
    /// server-side as a UUID v7 so two reviews can share the same name
    /// (e.g. a bookmark reused for a follow-up round).
    pub name: String,
    pub revset: RevSet,
    #[serde(default)]
    pub bookmark: Option<String>,
    pub created_by: Author,
    /// Optional author-written summary (markdown). Stored verbatim on
    /// the manifest and displayed at the top of the review.
    #[serde(default)]
    pub summary: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DraftCommentInput {
    pub anchor_change_id: ChangeId,
    pub anchor_commit_id: CommitId,
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub side: Option<Side>,
    #[serde(default)]
    pub lines: Option<LineRange>,
    /// `true` for review-wide comments (no specific file or commit
    /// scope). Must be `false` when `file` or `lines` is set.
    #[serde(default)]
    pub review_wide: bool,
    pub flag: Flag,
    #[serde(default)]
    pub body: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DraftResponseInput {
    pub in_reply_to: CommentId,
    pub action: ResolutionAction,
    #[serde(default)]
    pub body: String,
}

// ---- view shapes -------------------------------------------------------

/// Result of [`ReviewService::commit_diff`]: the diff for one commit
/// alongside both endpoints' change ids. The UI uses the change ids to
/// synthesize a patchset that scopes file reads, syntax highlighting,
/// and new-comment anchoring to the clicked commit instead of the
/// whole-review patchset's tip.
#[derive(Clone, Debug, Serialize)]
pub struct CommitDiffView {
    pub base_change: ChangeId,
    pub base_commit: CommitId,
    pub tip_change: ChangeId,
    pub tip_commit: CommitId,
    pub files: Vec<kata_core::FileChange>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ReviewView {
    pub manifest: ReviewManifest,
    pub diff: Diff,
    pub commits: Vec<CommitInfo>,
    pub comments: Vec<CommentView>,
    pub responses: Vec<ResponseView>,
    pub drafts: DraftsView,
    /// True when re-resolving the manifest's revset would advance the
    /// current patchset — i.e., the live tip or base of the branch has
    /// moved since the latest patchset was recorded. The UI uses this
    /// to decide whether the "Refresh" affordance is even worth showing.
    pub is_stale: bool,
    /// The user-facing jj error from re-resolving the manifest's revset,
    /// if it failed. Present when the revset has stopped resolving (e.g.
    /// a referenced change ID has gone divergent) — the UI surfaces it
    /// as a banner so the reader knows why `is_stale`, commits-panel
    /// liveness, and similar features have degraded. `None` when the
    /// revset resolves cleanly.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revset_error: Option<RevsetError>,
    /// Non-snapshot jj operations that landed in the repo between the
    /// viewer's previous open of this review and the current one,
    /// oldest first. Empty on the viewer's first ever open (no
    /// baseline yet) and when nothing relevant happened. The UI shows
    /// a compact "since you were here" summary when non-empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ops_since: Vec<OpSummary>,
    /// Wall-clock timestamp the viewer last opened this review at, or
    /// `None` on their first ever open. The UI compares it against each
    /// comment's responses to flag threads with new replies since the
    /// last visit. The recorded baseline advances on every open, so
    /// "unread" is naturally relative to the *previous* open, not the
    /// current one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_visit_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Structured information about a failure to resolve a review's
/// revset. The UI uses this to render a warning banner that explains
/// what went wrong and — for the divergent-change-ID case — lists
/// the commit IDs the reader has to `jj abandon` to disambiguate.
#[derive(Clone, Debug, Serialize)]
pub struct RevsetError {
    /// jj's stderr, with the leading `Error: ` framing stripped.
    /// First line is the headline; the rest is jj's hint output and
    /// renders as supplemental context.
    pub message: String,
    /// When the failure is a divergent change ID, the commit IDs of
    /// the conflicting visible commits. Empty for other revset
    /// errors (or when we couldn't enumerate the siblings).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub divergent_commit_ids: Vec<CommitId>,
}

#[derive(Clone, Debug, Serialize)]
pub struct CommentView {
    #[serde(flatten)]
    pub comment: Comment,
    pub anchor: AnchorView,
    pub draft: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct ResponseView {
    #[serde(flatten)]
    pub response: Response,
    pub draft: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum AnchorView {
    Valid,
    Moved { new_lines: LineRange },
    Drifted { new_lines: LineRange, similarity: f32 },
    Outdated { original_content: String },
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct DraftsView {
    pub session: Option<Session>,
    pub comments: Vec<CommentView>,
    pub responses: Vec<ResponseView>,
}

fn validate_anchor(input: &DraftCommentInput) -> ServiceResult<()> {
    if input.lines.is_some() && input.file.is_none() {
        return Err(ServiceError::BadRequest("lines provided without file".into()));
    }
    if input.lines.is_some() && input.side.is_none() {
        return Err(ServiceError::BadRequest("lines provided without side".into()));
    }
    Ok(())
}

/// Strip jj's stderr framing so the message reads like user-facing
/// guidance instead of a CLI dump. jj always prefixes its first line
/// with `Error: `; the rest (Caused by, hints) is left intact since
/// those carry useful context for parse failures.
fn clean_jj_stderr(stderr: &str) -> String {
    let trimmed = stderr.trim();
    trimmed.strip_prefix("Error: ").unwrap_or(trimmed).to_string()
}

/// Pull the change ID out of jj's stderr when the failure is a
/// divergent-change error (`Error: Change ID `X` is divergent`).
/// Returns `None` for any other shape so the caller can fall back
/// to a plain message-only error.
fn extract_divergent_change_id(stderr: &str) -> Option<&str> {
    if !stderr.contains("is divergent") {
        return None;
    }
    let after = stderr.split_once("Change ID `")?.1;
    after.split('`').next()
}

/// Build the [`RevsetError`] surfaced through `ReviewView` when the
/// live revset fails to resolve. For divergent change IDs we also
/// list the conflicting commit IDs so the UI can show the reader
/// exactly which commits to `jj abandon`.
async fn build_revset_error(jj: &dyn JjBackend, err: &kata_jj::Error) -> RevsetError {
    let stderr = match err {
        kata_jj::Error::JjFailed { stderr, .. } => stderr.as_str(),
        _ => {
            return RevsetError {
                message: err.to_string(),
                divergent_commit_ids: Vec::new(),
            };
        }
    };
    let divergent_commit_ids = match extract_divergent_change_id(stderr) {
        Some(change_id) => {
            let revset = kata_core::RevSet::new(format!("change_id({change_id})"));
            jj.list_commits(&revset)
                .await
                .map(|cs| cs.into_iter().map(|c| c.commit_id).collect())
                .unwrap_or_default()
        }
        None => Vec::new(),
    };
    RevsetError {
        message: clean_jj_stderr(stderr),
        divergent_commit_ids,
    }
}

#[cfg(test)]
mod compare_tests {
    use super::*;

    fn ci(change: &str, commit: &str, desc: &str) -> CommitInfo {
        CommitInfo {
            change_id: ChangeId::new(change),
            commit_id: CommitId::new(commit),
            author_email: "a@example.com".into(),
            author_timestamp: "2026-05-16T00:00:00Z".into(),
            description_first_line: desc.into(),
            description: desc.into(),
            changed_files: Vec::new(),
        }
    }

    #[test]
    fn pairs_same_when_change_and_commit_ids_both_match() {
        let from = vec![ci("ch1", "co1", "tweak the thing")];
        let to = vec![ci("ch1", "co1", "tweak the thing")];
        let pairs = pair_patchset_commits(&from, &to);
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].status, ChangeStatus::Same);
        assert_eq!(pairs[0].from_commit.as_ref().unwrap().as_str(), "co1");
        assert_eq!(pairs[0].to_commit.as_ref().unwrap().as_str(), "co1");
    }

    #[test]
    fn pairs_changed_when_change_matches_but_commit_differs() {
        // Same change-id, different commit-id == the author rewrote it.
        let from = vec![ci("ch1", "co-old", "tweak v1")];
        let to = vec![ci("ch1", "co-new", "tweak v2")];
        let pairs = pair_patchset_commits(&from, &to);
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].status, ChangeStatus::Changed);
        assert_eq!(pairs[0].from_commit.as_ref().unwrap().as_str(), "co-old");
        assert_eq!(pairs[0].to_commit.as_ref().unwrap().as_str(), "co-new");
        // Descriptions populated from both sides.
        assert_eq!(pairs[0].from_description.as_deref(), Some("tweak v1"));
        assert_eq!(pairs[0].to_description.as_deref(), Some("tweak v2"));
    }

    #[test]
    fn pairs_added_in_to_when_change_id_only_in_to_patchset() {
        let from: Vec<CommitInfo> = vec![];
        let to = vec![ci("ch1", "co1", "brand new commit")];
        let pairs = pair_patchset_commits(&from, &to);
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].status, ChangeStatus::AddedInTo);
        assert!(pairs[0].from_commit.is_none());
        assert!(pairs[0].from_description.is_none());
        assert_eq!(pairs[0].to_commit.as_ref().unwrap().as_str(), "co1");
    }

    #[test]
    fn pairs_removed_from_from_when_change_id_only_in_from_patchset() {
        let from = vec![ci("ch1", "co1", "dropped")];
        let to: Vec<CommitInfo> = vec![];
        let pairs = pair_patchset_commits(&from, &to);
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].status, ChangeStatus::RemovedFromFrom);
        assert_eq!(pairs[0].from_commit.as_ref().unwrap().as_str(), "co1");
        assert!(pairs[0].to_commit.is_none());
        assert!(pairs[0].to_description.is_none());
    }

    #[test]
    fn output_orders_to_side_first_then_removed_at_end() {
        // The UI surface wants "what's in PS_b" up top (the typical
        // workflow), then the dropped-from-PS_a leftovers at the bottom.
        let from = vec![
            ci("ch-keep", "co1", "still there"),
            ci("ch-gone", "co-gone", "vanished"),
        ];
        let to = vec![
            ci("ch-new", "co-new", "fresh"),
            ci("ch-keep", "co1", "still there"),
        ];
        let pairs = pair_patchset_commits(&from, &to);
        let statuses: Vec<ChangeStatus> = pairs.iter().map(|p| p.status).collect();
        // to-list order, then the removed entry trailing.
        assert_eq!(
            statuses,
            vec![
                ChangeStatus::AddedInTo,
                ChangeStatus::Same,
                ChangeStatus::RemovedFromFrom,
            ],
        );
    }

    #[test]
    fn mixed_scenario_buckets_each_change_id_independently() {
        // Realistic case: PS_a has three commits, agent rewrites one
        // (changed), drops one (removed), keeps one as-is (same), then
        // adds a fourth (added).
        let from = vec![
            ci("ch-same", "co-same", "context unchanged"),
            ci("ch-rewrite", "co-rewrite-v1", "first try"),
            ci("ch-drop", "co-drop", "abandoned in v2"),
        ];
        let to = vec![
            ci("ch-rewrite", "co-rewrite-v2", "second try"),
            ci("ch-same", "co-same", "context unchanged"),
            ci("ch-new", "co-new", "agent added this"),
        ];
        let pairs = pair_patchset_commits(&from, &to);
        let by_change: std::collections::HashMap<&str, ChangeStatus> = pairs
            .iter()
            .map(|p| (p.change_id.as_str(), p.status))
            .collect();
        assert_eq!(by_change.get("ch-same").copied(), Some(ChangeStatus::Same));
        assert_eq!(
            by_change.get("ch-rewrite").copied(),
            Some(ChangeStatus::Changed)
        );
        assert_eq!(
            by_change.get("ch-new").copied(),
            Some(ChangeStatus::AddedInTo)
        );
        assert_eq!(
            by_change.get("ch-drop").copied(),
            Some(ChangeStatus::RemovedFromFrom)
        );
        assert_eq!(pairs.len(), 4);
    }
}
