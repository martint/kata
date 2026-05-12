//! Application service — the layer between transports (HTTP, MCP) and the
//! storage + jj backends. Pure async functions; transports adapt requests
//! and responses around them.

pub mod error;
pub mod events;

use std::collections::HashMap;
use std::sync::Arc;

use chrono::Utc;
use kata_core::{
    Author, Bookmark, ChangeId, Comment, CommentId, CommitId, CommitInfo, Diff, Flag,
    LineRange, Patchset, RepoId, RepoSummary, ResolutionAction, Response, ResponseId,
    ReviewId, ReviewManifest, RevSet, SCHEMA_VERSION, Session, SessionId, Side,
};
use kata_jj::{AnchorResolution, JjBackend, build_diff, resolve_anchor};
use kata_storage::{ReviewSummary, Storage};
use serde::{Deserialize, Serialize};

pub use crate::error::{ServiceError, ServiceResult};
pub use crate::events::{Event, EventBus};

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

    pub async fn list_reviews(&self, repo: &RepoId) -> ServiceResult<Vec<ReviewSummary>> {
        Ok(self.storage.list_reviews(repo).await?)
    }

    // ---- review lifecycle ----------------------------------------------

    pub async fn create_review(
        &self,
        repo: &RepoId,
        params: CreateReviewParams,
    ) -> ServiceResult<ReviewManifest> {
        let jj = self.jj_for(repo)?;
        let CreateReviewParams {
            review_id,
            revset,
            bookmark,
            created_by,
        } = params;
        let range = jj.resolve_range(&revset).await?;
        let now = Utc::now();
        let manifest = ReviewManifest {
            schema_version: SCHEMA_VERSION,
            review_id,
            revset,
            created_at: now,
            created_by,
            bookmark,
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
        };
        self.storage.create_review(repo, &manifest).await?;
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
    pub async fn open_review(
        &self,
        repo: &RepoId,
        review: &ReviewId,
        viewer: &Author,
        patchset: Option<u32>,
    ) -> ServiceResult<ReviewView> {
        let jj = self.jj_for(repo)?;
        let total = std::time::Instant::now();

        let t = std::time::Instant::now();
        let manifest = self.storage.open_review(repo, review).await?;
        tracing::debug!(elapsed_ms = t.elapsed().as_millis() as u64, "open_review: manifest");

        let selected_n = patchset.unwrap_or(manifest.current_patchset);
        let selected = manifest
            .patchset(selected_n)
            .ok_or_else(|| ServiceError::NotFound(format!("patchset {selected_n}")))?
            .clone();

        let t = std::time::Instant::now();
        let (diff, commits) = tokio::try_join!(
            build_diff(&**jj, &selected.base_commit, &selected.tip_commit),
            jj.list_commits(&manifest.revset),
        )?;
        tracing::debug!(
            elapsed_ms = t.elapsed().as_millis() as u64,
            files = diff.files.len(),
            commits = commits.len(),
            "open_review: diff + commits",
        );

        let t = std::time::Instant::now();
        let (published, responses, drafts) = tokio::try_join!(
            self.storage.list_published_comments(repo, review),
            self.storage.list_published_responses(repo, review),
            self.storage.list_drafts_for(repo, review, viewer),
        )?;
        tracing::debug!(
            elapsed_ms = t.elapsed().as_millis() as u64,
            published = published.len(),
            responses = responses.len(),
            drafts = drafts.comments.len(),
            "open_review: storage",
        );

        let t = std::time::Instant::now();
        let mut comments = Vec::with_capacity(published.len());
        for c in published {
            if c.patchset > selected_n {
                continue;
            }
            comments.push(self.build_comment_view(repo, c, &selected, false).await?);
        }
        let mut draft_comments = Vec::with_capacity(drafts.comments.len());
        for c in drafts.comments {
            if c.patchset > selected_n {
                continue;
            }
            draft_comments.push(self.build_comment_view(repo, c, &selected, true).await?);
        }
        tracing::debug!(
            elapsed_ms = t.elapsed().as_millis() as u64,
            comments = comments.len() + draft_comments.len(),
            "open_review: comment views",
        );

        tracing::info!(
            review = %review,
            elapsed_ms = total.elapsed().as_millis() as u64,
            "open_review",
        );

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
        })
    }

    async fn build_comment_view(
        &self,
        repo: &RepoId,
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
    /// when the UI scopes a review view down to one commit.
    pub async fn commit_diff(
        &self,
        repo: &RepoId,
        change: &ChangeId,
    ) -> ServiceResult<Diff> {
        let jj = self.jj_for(repo)?;
        let tip = jj
            .change_to_commit(change)
            .await?
            .ok_or_else(|| ServiceError::NotFound(format!("change {change}")))?;
        let parent_expr = ChangeId::new(format!("{change}-"));
        let base = jj
            .change_to_commit(&parent_expr)
            .await?
            .ok_or_else(|| ServiceError::NotFound(format!("parent of change {change}")))?;
        Ok(build_diff(&**jj, &base, &tip).await?)
    }

    /// Re-resolve the revset. If the tip has moved since the current
    /// patchset was recorded, append a new patchset and make it current.
    /// Otherwise leave the manifest untouched.
    pub async fn refresh_review(
        &self,
        repo: &RepoId,
        review: &ReviewId,
    ) -> ServiceResult<ReviewManifest> {
        let jj = self.jj_for(repo)?;
        let mut manifest = self.storage.open_review(repo, review).await?;
        let range = jj.resolve_range(&manifest.revset).await?;
        let current = manifest.current().clone();
        if range.tip.commit_id == current.tip_commit
            && range.base.commit_id == current.base_commit
        {
            return Ok(manifest);
        }
        let parent_patchset = if jj
            .is_ancestor(&current.tip_commit, &range.tip.commit_id)
            .await?
        {
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
            flag: input.flag,
            body: input.body,
        };
        self.storage.upsert_draft_comment(repo, &comment).await?;
        Ok(comment)
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
    pub review_id: ReviewId,
    pub revset: RevSet,
    #[serde(default)]
    pub bookmark: Option<String>,
    pub created_by: Author,
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

#[derive(Clone, Debug, Serialize)]
pub struct ReviewView {
    pub manifest: ReviewManifest,
    pub diff: Diff,
    pub commits: Vec<CommitInfo>,
    pub comments: Vec<CommentView>,
    pub responses: Vec<ResponseView>,
    pub drafts: DraftsView,
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
