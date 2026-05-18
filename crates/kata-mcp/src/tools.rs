use std::sync::Arc;

use kata_core::{
    AnnotationId, Author, ChangeId, ColumnRange, CommentId, CommitId, Flag, LineRange, RepoId,
    ResolutionAction, ReviewId, RevSet, SessionId, Side,
};
use kata_service::{
    AnnotationInput, CreateReviewParams, DraftCommentInput, DraftResponseInput, ReviewService,
    ServiceError,
};
use rmcp::{
    ErrorData as McpError, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::*,
    service::RequestContext,
    tool, tool_handler, tool_router,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};
use serde::{Deserialize, Serialize};

/// MCP service. A single instance fronts every repo registered with the
/// underlying [`ReviewService`]; tool callers select one by passing `repo`
/// (the workspace slug) on each call.
#[derive(Clone)]
pub struct ReviewMcp {
    service: Arc<ReviewService>,
    author: Author,
    tool_router: ToolRouter<ReviewMcp>,
}

#[tool_router]
impl ReviewMcp {
    pub fn new(service: Arc<ReviewService>, author: Author) -> Self {
        Self {
            service,
            author,
            tool_router: Self::tool_router(),
        }
    }

    fn resolve(&self, repo: &str) -> Result<RepoId, McpError> {
        self.service.resolve_repo(repo).map_err(into_mcp)
    }

    // ---- discovery -----------------------------------------------------

    #[tool(
        description = "List the repositories this MCP server can act on. Returns each repo's `name` (the slug to pass as `repo` to every other tool) and its canonical path."
    )]
    async fn list_repos(&self) -> Result<CallToolResult, McpError> {
        Ok(text_json(&self.service.list_repos()))
    }

    #[tool(description = "List bookmarks in the underlying jj repo.")]
    async fn list_bookmarks(
        &self,
        Parameters(args): Parameters<RepoArgs>,
    ) -> Result<CallToolResult, McpError> {
        let repo = self.resolve(&args.repo)?;
        let bookmarks = self.service.list_bookmarks(&repo).await.map_err(into_mcp)?;
        Ok(text_json(&bookmarks))
    }

    #[tool(description = "List existing reviews in this repo.")]
    async fn list_reviews(
        &self,
        Parameters(args): Parameters<RepoArgs>,
    ) -> Result<CallToolResult, McpError> {
        let repo = self.resolve(&args.repo)?;
        let reviews = self.service.list_reviews(&repo).await.map_err(into_mcp)?;
        Ok(text_json(&reviews))
    }

    #[tool(
        description = "Open a review and return its manifest, diff, published comments/responses, and the agent's own drafts. Pass `patchset` to view an earlier round; omit for the latest."
    )]
    async fn get_review(
        &self,
        Parameters(args): Parameters<GetReviewArgs>,
    ) -> Result<CallToolResult, McpError> {
        let repo = self.resolve(&args.repo)?;
        let view = self
            .service
            .open_review(&repo, &args.review_id, &self.author, args.patchset, None)
            .await
            .map_err(into_mcp)?;
        Ok(text_json(&view))
    }

    // ---- review lifecycle ----------------------------------------------

    #[tool(
        description = "Create a new review against a revset (defaults to `trunk()..<bookmark>` when only `bookmark` is given)."
    )]
    async fn create_review(
        &self,
        Parameters(args): Parameters<CreateReviewArgs>,
    ) -> Result<CallToolResult, McpError> {
        let CreateReviewArgs {
            repo,
            name,
            revset,
            bookmark,
            summary,
        } = args;
        let repo = self.resolve(&repo)?;
        let revset = revset.unwrap_or_else(|| {
            let slug = bookmark.as_deref().unwrap_or(name.as_str());
            RevSet::trunk_to(slug)
        });
        let manifest = self
            .service
            .create_review(
                &repo,
                CreateReviewParams {
                    name,
                    revset,
                    bookmark,
                    created_by: self.author.clone(),
                    summary,
                },
            )
            .await
            .map_err(into_mcp)?;
        Ok(text_json(&manifest))
    }

    #[tool(
        description = "Re-resolve the review's revset against the underlying jj repo. If the tip or base has moved since the last patchset was recorded, append a new patchset and make it current. Call after pushing additional commits or rewriting the branch so reviewers see the new round. Optionally pass `summary` to also update the review summary in the same call — only the review's creator may do that."
    )]
    async fn refresh_review(
        &self,
        Parameters(args): Parameters<RefreshReviewArgs>,
    ) -> Result<CallToolResult, McpError> {
        let repo = self.resolve(&args.repo)?;
        let manifest = self
            .service
            .refresh_review(&repo, &args.review_id, &self.author, args.summary)
            .await
            .map_err(into_mcp)?;
        Ok(text_json(&manifest))
    }

    #[tool(
        description = "Replace the free-text summary on a review. Only the review's creator may call this. Pass `summary: null` (or an empty string) to clear it."
    )]
    async fn update_review_summary(
        &self,
        Parameters(args): Parameters<UpdateSummaryArgs>,
    ) -> Result<CallToolResult, McpError> {
        let repo = self.resolve(&args.repo)?;
        let manifest = self
            .service
            .update_review_summary(&repo, &args.review_id, &self.author, args.summary)
            .await
            .map_err(into_mcp)?;
        Ok(text_json(&manifest))
    }

    #[tool(
        description = "Start or reuse the agent's open draft session for a review. Idempotent — same session is returned until it's published or discarded."
    )]
    async fn start_session(
        &self,
        Parameters(args): Parameters<StartSessionArgs>,
    ) -> Result<CallToolResult, McpError> {
        let repo = self.resolve(&args.repo)?;
        let session = self
            .service
            .start_session(&repo, &args.review_id, &self.author)
            .await
            .map_err(into_mcp)?;
        Ok(text_json(&session))
    }

    #[tool(
        description = "Publish the agent's draft session. All draft comments and responses in it become visible to other reviewers."
    )]
    async fn publish_session(
        &self,
        Parameters(args): Parameters<SessionArgs>,
    ) -> Result<CallToolResult, McpError> {
        let repo = self.resolve(&args.repo)?;
        self.service
            .publish_session(&repo, &args.review_id, &args.session_id)
            .await
            .map_err(into_mcp)?;
        Ok(ok_text("published"))
    }

    #[tool(
        description = "Discard the agent's draft session. Drafts in it are not deleted from disk but become invisible to readers."
    )]
    async fn discard_session(
        &self,
        Parameters(args): Parameters<SessionArgs>,
    ) -> Result<CallToolResult, McpError> {
        let repo = self.resolve(&args.repo)?;
        self.service
            .discard_session(&repo, &args.review_id, &args.session_id)
            .await
            .map_err(into_mcp)?;
        Ok(ok_text("discarded"))
    }

    // ---- comments ------------------------------------------------------

    #[tool(
        description = "Draft a line-level comment. Auto-starts a session if none is open. Use `flag` to mark severity: must-do, suggestion, or question. Pass `columns` to scope to a sub-region of the line range: for a single line `lines.start == lines.end`, `columns` is the half-open UTF-16 range `[start, end)` within that line; for a multi-line range, `columns.start` is the offset on the FIRST line and `columns.end` is the offset on the LAST line (no relation required between the two values)."
    )]
    async fn draft_line_comment(
        &self,
        Parameters(args): Parameters<DraftLineCommentArgs>,
    ) -> Result<CallToolResult, McpError> {
        let DraftLineCommentArgs {
            repo,
            review_id,
            anchor_change_id,
            anchor_commit_id,
            file,
            side,
            lines,
            columns,
            flag,
            body,
        } = args;
        let repo = self.resolve(&repo)?;
        let session = self
            .service
            .start_session(&repo, &review_id, &self.author)
            .await
            .map_err(into_mcp)?;
        let input = DraftCommentInput {
            anchor_change_id,
            anchor_commit_id,
            file: Some(file),
            side: Some(side),
            lines: Some(lines),
            columns,
            review_wide: false,
            flag,
            body: body.unwrap_or_default(),
        };
        let comment = self
            .service
            .upsert_draft_comment(
                &repo,
                &review_id,
                &session.session_id,
                &self.author,
                None,
                input,
            )
            .await
            .map_err(into_mcp)?;
        Ok(text_json(&comment))
    }

    #[tool(description = "Draft a whole-file comment. Auto-starts a session.")]
    async fn draft_file_comment(
        &self,
        Parameters(args): Parameters<DraftFileCommentArgs>,
    ) -> Result<CallToolResult, McpError> {
        let DraftFileCommentArgs {
            repo,
            review_id,
            anchor_change_id,
            anchor_commit_id,
            file,
            flag,
            body,
        } = args;
        let repo = self.resolve(&repo)?;
        let session = self
            .service
            .start_session(&repo, &review_id, &self.author)
            .await
            .map_err(into_mcp)?;
        let input = DraftCommentInput {
            anchor_change_id,
            anchor_commit_id,
            file: Some(file),
            side: None,
            lines: None,
            columns: None,
            review_wide: false,
            flag,
            body: body.unwrap_or_default(),
        };
        let comment = self
            .service
            .upsert_draft_comment(
                &repo,
                &review_id,
                &session.session_id,
                &self.author,
                None,
                input,
            )
            .await
            .map_err(into_mcp)?;
        Ok(text_json(&comment))
    }

    #[tool(
        description = "Edit the body and flag of an existing draft comment authored by the current MCP user. The anchor (file / lines / side) and review-id are kept; pass the new `body` and `flag`. Fails if the comment is already published or doesn't belong to the caller's open session."
    )]
    async fn update_draft_comment(
        &self,
        Parameters(args): Parameters<UpdateDraftCommentArgs>,
    ) -> Result<CallToolResult, McpError> {
        let UpdateDraftCommentArgs {
            repo,
            review_id,
            comment_id,
            flag,
            body,
        } = args;
        let repo = self.resolve(&repo)?;
        let comment = self
            .service
            .update_draft_comment(
                &repo,
                &review_id,
                &self.author,
                &comment_id,
                body.unwrap_or_default(),
                flag,
            )
            .await
            .map_err(into_mcp)?;
        Ok(text_json(&comment))
    }

    #[tool(description = "Draft a whole-review comment. Auto-starts a session.")]
    async fn draft_review_comment(
        &self,
        Parameters(args): Parameters<DraftReviewCommentArgs>,
    ) -> Result<CallToolResult, McpError> {
        let DraftReviewCommentArgs {
            repo,
            review_id,
            anchor_change_id,
            anchor_commit_id,
            flag,
            body,
        } = args;
        let repo = self.resolve(&repo)?;
        let session = self
            .service
            .start_session(&repo, &review_id, &self.author)
            .await
            .map_err(into_mcp)?;
        let input = DraftCommentInput {
            anchor_change_id,
            anchor_commit_id,
            file: None,
            side: None,
            lines: None,
            columns: None,
            review_wide: true,
            flag,
            body: body.unwrap_or_default(),
        };
        let comment = self
            .service
            .upsert_draft_comment(
                &repo,
                &review_id,
                &session.session_id,
                &self.author,
                None,
                input,
            )
            .await
            .map_err(into_mcp)?;
        Ok(text_json(&comment))
    }

    // ---- annotations ---------------------------------------------------
    //
    // Author-only context notes. Only the review's creator (the agent's
    // identity must equal the manifest's `created_by`) can write — the
    // service layer enforces this, so attempts from other identities
    // fail with BadRequest. No session/draft cycle and no flag.

    #[tool(
        description = "Attach a line-anchored author note (annotation) to a region of code. Annotations are one-way context for reviewers — no replies, no resolve state. Only the review's creator may author them. Publishes immediately; there's no draft cycle."
    )]
    async fn add_line_annotation(
        &self,
        Parameters(args): Parameters<AddLineAnnotationArgs>,
    ) -> Result<CallToolResult, McpError> {
        let AddLineAnnotationArgs {
            repo,
            review_id,
            anchor_change_id,
            anchor_commit_id,
            file,
            side,
            lines,
            body,
        } = args;
        let repo = self.resolve(&repo)?;
        let input = AnnotationInput {
            anchor_change_id,
            anchor_commit_id,
            file: Some(file),
            side: Some(side),
            lines: Some(lines),
            body: body.unwrap_or_default(),
        };
        let annotation = self
            .service
            .upsert_annotation(&repo, &review_id, &self.author, None, input)
            .await
            .map_err(into_mcp)?;
        Ok(text_json(&annotation))
    }

    #[tool(
        description = "Attach a whole-file author note (annotation). Same author-only / publishes-immediately semantics as add_line_annotation."
    )]
    async fn add_file_annotation(
        &self,
        Parameters(args): Parameters<AddFileAnnotationArgs>,
    ) -> Result<CallToolResult, McpError> {
        let AddFileAnnotationArgs {
            repo,
            review_id,
            anchor_change_id,
            anchor_commit_id,
            file,
            body,
        } = args;
        let repo = self.resolve(&repo)?;
        let input = AnnotationInput {
            anchor_change_id,
            anchor_commit_id,
            file: Some(file),
            side: None,
            lines: None,
            body: body.unwrap_or_default(),
        };
        let annotation = self
            .service
            .upsert_annotation(&repo, &review_id, &self.author, None, input)
            .await
            .map_err(into_mcp)?;
        Ok(text_json(&annotation))
    }

    #[tool(
        description = "Edit the body of an existing annotation. Anchor and id are kept; the new body replaces the old. Only the review's creator may edit."
    )]
    async fn update_annotation(
        &self,
        Parameters(args): Parameters<UpdateAnnotationArgs>,
    ) -> Result<CallToolResult, McpError> {
        let UpdateAnnotationArgs {
            repo,
            review_id,
            annotation_id,
            body,
        } = args;
        let repo = self.resolve(&repo)?;
        // Look up the existing annotation so we can re-supply its anchor
        // unchanged (the service's upsert needs a full input, not a
        // patch). Open-review is the cheap path that reuses
        // `list_annotations` underneath.
        let view = self
            .service
            .open_review(&repo, &review_id, &self.author, None, None)
            .await
            .map_err(into_mcp)?;
        let existing = view
            .annotations
            .iter()
            .find(|a| a.annotation.annotation_id == annotation_id)
            .ok_or_else(|| {
                McpError::invalid_params(
                    format!("annotation {annotation_id} not found in review {review_id}"),
                    None,
                )
            })?;
        let input = AnnotationInput {
            anchor_change_id: existing.annotation.anchor_change_id.clone(),
            anchor_commit_id: existing.annotation.anchor_commit_id.clone(),
            file: existing.annotation.file.clone(),
            side: existing.annotation.side,
            lines: existing.annotation.lines,
            body: body.unwrap_or_default(),
        };
        let annotation = self
            .service
            .upsert_annotation(&repo, &review_id, &self.author, Some(annotation_id), input)
            .await
            .map_err(into_mcp)?;
        Ok(text_json(&annotation))
    }

    #[tool(
        description = "Delete an annotation. Only the review's creator may delete."
    )]
    async fn delete_annotation(
        &self,
        Parameters(args): Parameters<DeleteAnnotationArgs>,
    ) -> Result<CallToolResult, McpError> {
        let DeleteAnnotationArgs {
            repo,
            review_id,
            annotation_id,
        } = args;
        let repo = self.resolve(&repo)?;
        self.service
            .delete_annotation(&repo, &review_id, &self.author, &annotation_id)
            .await
            .map_err(into_mcp)?;
        Ok(ok_text("deleted"))
    }

    #[tool(
        description = "Reply to a comment. `action` controls resolution: comment (no state change), resolve, unresolve, wont-fix, un-wont-fix. Auto-starts a session."
    )]
    async fn respond(
        &self,
        Parameters(args): Parameters<RespondArgs>,
    ) -> Result<CallToolResult, McpError> {
        let RespondArgs {
            repo,
            review_id,
            in_reply_to,
            action,
            body,
        } = args;
        let repo = self.resolve(&repo)?;
        let session = self
            .service
            .start_session(&repo, &review_id, &self.author)
            .await
            .map_err(into_mcp)?;
        let response = self
            .service
            .upsert_draft_response(
                &repo,
                &session.session_id,
                &self.author,
                None,
                DraftResponseInput {
                    in_reply_to,
                    action,
                    body: body.unwrap_or_default(),
                },
            )
            .await
            .map_err(into_mcp)?;
        Ok(text_json(&response))
    }
}

/// The review-workflow skill, served via MCP resources under
/// `skill://kata/review`. Embedded at build time so the server stays a
/// single self-contained binary. The Skills-over-MCP working group's
/// resource-based extension is the current direction for skill
/// distribution (SEP-2076 was closed; SEP-2640 is the live proposal).
const REVIEW_SKILL_URI: &str = "skill://kata/review";
const REVIEW_SKILL_BODY: &str = include_str!("../skills/review.md");

#[tool_handler]
impl ServerHandler for ReviewMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::V_2024_11_05,
            capabilities: ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
            server_info: Implementation::from_build_env(),
            instructions: Some(
                "Code review tool. One server can front multiple repositories; pass `repo` \
                 (a workspace slug from `list_repos`) on every tool call. Use `list_reviews` \
                 and `get_review` to inspect changes; `draft_line_comment` / \
                 `draft_file_comment` / `draft_review_comment` to leave feedback (starts a \
                 draft session on first use); `update_draft_comment` to revise a draft \
                 before publishing; `respond` to reply or change resolution; \
                 `publish_session` once the round is complete. Review creators may set \
                 a summary at `create_review` time and replace it later via \
                 `update_review_summary` (or alongside `refresh_review`). Before doing \
                 review work, read the resource `skill://kata/review` for the full workflow."
                    .into(),
            ),
        }
    }

    async fn initialize(
        &self,
        request: InitializeRequestParam,
        _ctx: RequestContext<rmcp::RoleServer>,
    ) -> Result<InitializeResult, McpError> {
        Ok(InitializeResult {
            protocol_version: request.protocol_version,
            capabilities: self.get_info().capabilities,
            server_info: self.get_info().server_info,
            instructions: self.get_info().instructions,
        })
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParam>,
        _ctx: RequestContext<rmcp::RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        let raw = RawResource {
            uri: REVIEW_SKILL_URI.into(),
            name: "kata-review".into(),
            title: Some("Kata code review skill".into()),
            description: Some(
                "Workflow for reviewing code via the Kata MCP server: list reviews, \
                 open one, leave anchored draft comments, then publish."
                    .into(),
            ),
            mime_type: Some("text/markdown".into()),
            size: Some(REVIEW_SKILL_BODY.len() as u32),
            icons: None,
        };
        Ok(ListResourcesResult {
            resources: vec![Annotated::new(raw, None)],
            next_cursor: None,
        })
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParam,
        _ctx: RequestContext<rmcp::RoleServer>,
    ) -> Result<ReadResourceResult, McpError> {
        match request.uri.as_str() {
            REVIEW_SKILL_URI => Ok(ReadResourceResult {
                contents: vec![ResourceContents::TextResourceContents {
                    uri: REVIEW_SKILL_URI.into(),
                    mime_type: Some("text/markdown".into()),
                    text: REVIEW_SKILL_BODY.into(),
                    meta: None,
                }],
            }),
            other => Err(McpError::resource_not_found(
                "resource not found",
                Some(serde_json::json!({ "uri": other })),
            )),
        }
    }
}

// ---- request schemas ---------------------------------------------------

/// Tools that act on a single repo but take no other arguments use this
/// shape. Every other Args struct embeds `repo` as its first field.
#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct RepoArgs {
    /// Workspace slug (from `list_repos`).
    pub repo: String,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct GetReviewArgs {
    pub repo: String,
    pub review_id: ReviewId,
    #[serde(default)]
    pub patchset: Option<u32>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct CreateReviewArgs {
    pub repo: String,
    /// Human-readable label for the review (e.g. the bookmark slug).
    /// The internal `review_id` is generated server-side; what the URL
    /// shows is the per-repo `number` the storage layer assigns.
    pub name: String,
    #[serde(default)]
    pub revset: Option<RevSet>,
    #[serde(default)]
    pub bookmark: Option<String>,
    /// Optional markdown summary shown at the top of the review.
    #[serde(default)]
    pub summary: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct RefreshReviewArgs {
    pub repo: String,
    pub review_id: ReviewId,
    /// When set, also replaces the review's summary. Only the creator
    /// may do this; non-creators get a BadRequest. Omit to refresh
    /// without touching the summary.
    #[serde(default)]
    pub summary: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct UpdateSummaryArgs {
    pub repo: String,
    pub review_id: ReviewId,
    /// New summary. `null` (or omitting) clears the existing one.
    #[serde(default)]
    pub summary: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct StartSessionArgs {
    pub repo: String,
    pub review_id: ReviewId,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct SessionArgs {
    pub repo: String,
    pub review_id: ReviewId,
    pub session_id: SessionId,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct DraftLineCommentArgs {
    pub repo: String,
    pub review_id: ReviewId,
    pub anchor_change_id: ChangeId,
    pub anchor_commit_id: CommitId,
    pub file: String,
    pub side: Side,
    pub lines: LineRange,
    /// Optional column-range anchor (UTF-16). For a single-line
    /// `lines`, this is `[start, end)` within the line. For a multi-
    /// line `lines`, `start` is the offset on the FIRST selected
    /// line and `end` is the offset on the LAST one. Omit for whole-
    /// line(s).
    #[serde(default)]
    pub columns: Option<ColumnRange>,
    pub flag: Flag,
    #[serde(default)]
    pub body: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct DraftFileCommentArgs {
    pub repo: String,
    pub review_id: ReviewId,
    pub anchor_change_id: ChangeId,
    pub anchor_commit_id: CommitId,
    pub file: String,
    pub flag: Flag,
    #[serde(default)]
    pub body: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct DraftReviewCommentArgs {
    pub repo: String,
    pub review_id: ReviewId,
    pub anchor_change_id: ChangeId,
    pub anchor_commit_id: CommitId,
    pub flag: Flag,
    #[serde(default)]
    pub body: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct UpdateDraftCommentArgs {
    pub repo: String,
    pub review_id: ReviewId,
    pub comment_id: CommentId,
    pub flag: Flag,
    #[serde(default)]
    pub body: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct RespondArgs {
    pub repo: String,
    pub review_id: ReviewId,
    pub in_reply_to: CommentId,
    pub action: ResolutionAction,
    #[serde(default)]
    pub body: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct AddLineAnnotationArgs {
    pub repo: String,
    pub review_id: ReviewId,
    pub anchor_change_id: ChangeId,
    pub anchor_commit_id: CommitId,
    pub file: String,
    pub side: Side,
    pub lines: LineRange,
    #[serde(default)]
    pub body: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct AddFileAnnotationArgs {
    pub repo: String,
    pub review_id: ReviewId,
    pub anchor_change_id: ChangeId,
    pub anchor_commit_id: CommitId,
    pub file: String,
    #[serde(default)]
    pub body: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct UpdateAnnotationArgs {
    pub repo: String,
    pub review_id: ReviewId,
    pub annotation_id: AnnotationId,
    #[serde(default)]
    pub body: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct DeleteAnnotationArgs {
    pub repo: String,
    pub review_id: ReviewId,
    pub annotation_id: AnnotationId,
}

// ---- helpers -----------------------------------------------------------

fn text_json<T: Serialize>(value: &T) -> CallToolResult {
    match serde_json::to_string_pretty(value) {
        Ok(s) => CallToolResult::success(vec![Content::text(s)]),
        Err(e) => CallToolResult::error(vec![Content::text(format!(
            "serialization failed: {e}"
        ))]),
    }
}

fn ok_text(s: &str) -> CallToolResult {
    CallToolResult::success(vec![Content::text(s.to_string())])
}

fn into_mcp(err: ServiceError) -> McpError {
    McpError::internal_error(err.to_string(), None)
}

/// Build an axum-mountable MCP service. A single instance fronts every
/// repo registered with the underlying [`ReviewService`]; mount once at
/// `/mcp` and let clients pick a repo per call.
pub fn mcp_service(
    service: Arc<ReviewService>,
    author: Author,
) -> StreamableHttpService<ReviewMcp, LocalSessionManager> {
    let kata_mcp = ReviewMcp::new(service, author);
    StreamableHttpService::new(
        move || Ok(kata_mcp.clone()),
        LocalSessionManager::default().into(),
        StreamableHttpServerConfig::default(),
    )
}
