use std::sync::Arc;

use kata_core::{
    Author, ChangeId, CommentId, CommitId, Flag, LineRange, RepoId, ResolutionAction,
    ReviewId, RevSet, SessionId, Side,
};
use kata_service::{
    CreateReviewParams, DraftCommentInput, DraftResponseInput, ReviewService, ServiceError,
};
use rmcp::{
    ErrorData as McpError, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::*,
    schemars,
    service::RequestContext,
    tool, tool_handler, tool_router,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};
use serde::{Deserialize, Serialize};

/// MCP service. Bound to one repository at construction so agents don't
/// need to pass a repo argument on every call.
#[derive(Clone)]
pub struct ReviewMcp {
    service: Arc<ReviewService>,
    repo: RepoId,
    author: Author,
    tool_router: ToolRouter<ReviewMcp>,
}

#[tool_router]
impl ReviewMcp {
    pub fn new(service: Arc<ReviewService>, repo: RepoId, author: Author) -> Self {
        Self {
            service,
            repo,
            author,
            tool_router: Self::tool_router(),
        }
    }

    // ---- discovery -----------------------------------------------------

    #[tool(description = "List bookmarks in the underlying jj repo.")]
    async fn list_bookmarks(&self) -> Result<CallToolResult, McpError> {
        let bookmarks = self.service.list_bookmarks(&self.repo).await.map_err(into_mcp)?;
        Ok(text_json(&bookmarks))
    }

    #[tool(description = "List existing reviews in this repo.")]
    async fn list_reviews(&self) -> Result<CallToolResult, McpError> {
        let reviews = self.service.list_reviews(&self.repo).await.map_err(into_mcp)?;
        Ok(text_json(&reviews))
    }

    #[tool(
        description = "Open a review and return its manifest, diff, published comments/responses, and the agent's own drafts. Pass `patchset` to view an earlier round; omit for the latest."
    )]
    async fn get_review(
        &self,
        Parameters(args): Parameters<GetReviewArgs>,
    ) -> Result<CallToolResult, McpError> {
        let view = self
            .service
            .open_review(&self.repo, &args.review_id, &self.author, args.patchset)
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
            review_id,
            revset,
            bookmark,
        } = args;
        let revset = revset.unwrap_or_else(|| {
            let name = bookmark.as_deref().unwrap_or(review_id.as_str());
            RevSet::trunk_to(name)
        });
        let manifest = self
            .service
            .create_review(
                &self.repo,
                CreateReviewParams {
                    review_id,
                    revset,
                    bookmark,
                    created_by: self.author.clone(),
                },
            )
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
        let session = self
            .service
            .start_session(&self.repo, &args.review_id, &self.author)
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
        self.service
            .publish_session(&self.repo, &args.review_id, &args.session_id)
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
        self.service
            .discard_session(&self.repo, &args.review_id, &args.session_id)
            .await
            .map_err(into_mcp)?;
        Ok(ok_text("discarded"))
    }

    // ---- comments ------------------------------------------------------

    #[tool(
        description = "Draft a line-level comment. Auto-starts a session if none is open. Use `flag` to mark severity: must-do, suggestion, or other."
    )]
    async fn draft_line_comment(
        &self,
        Parameters(args): Parameters<DraftLineCommentArgs>,
    ) -> Result<CallToolResult, McpError> {
        let DraftLineCommentArgs {
            review_id,
            anchor_change_id,
            anchor_commit_id,
            file,
            side,
            lines,
            flag,
            body,
        } = args;
        let session = self
            .service
            .start_session(&self.repo, &review_id, &self.author)
            .await
            .map_err(into_mcp)?;
        let input = DraftCommentInput {
            anchor_change_id,
            anchor_commit_id,
            file: Some(file),
            side: Some(side),
            lines: Some(lines),
            flag,
            body: body.unwrap_or_default(),
        };
        let comment = self
            .service
            .upsert_draft_comment(
                &self.repo,
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
            review_id,
            anchor_change_id,
            anchor_commit_id,
            file,
            flag,
            body,
        } = args;
        let session = self
            .service
            .start_session(&self.repo, &review_id, &self.author)
            .await
            .map_err(into_mcp)?;
        let input = DraftCommentInput {
            anchor_change_id,
            anchor_commit_id,
            file: Some(file),
            side: None,
            lines: None,
            flag,
            body: body.unwrap_or_default(),
        };
        let comment = self
            .service
            .upsert_draft_comment(
                &self.repo,
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

    #[tool(description = "Draft a whole-review comment. Auto-starts a session.")]
    async fn draft_review_comment(
        &self,
        Parameters(args): Parameters<DraftReviewCommentArgs>,
    ) -> Result<CallToolResult, McpError> {
        let DraftReviewCommentArgs {
            review_id,
            anchor_change_id,
            anchor_commit_id,
            flag,
            body,
        } = args;
        let session = self
            .service
            .start_session(&self.repo, &review_id, &self.author)
            .await
            .map_err(into_mcp)?;
        let input = DraftCommentInput {
            anchor_change_id,
            anchor_commit_id,
            file: None,
            side: None,
            lines: None,
            flag,
            body: body.unwrap_or_default(),
        };
        let comment = self
            .service
            .upsert_draft_comment(
                &self.repo,
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
        description = "Reply to a comment. `action` controls resolution: comment (no state change), resolve, unresolve, wont-fix, un-wont-fix. Auto-starts a session."
    )]
    async fn respond(
        &self,
        Parameters(args): Parameters<RespondArgs>,
    ) -> Result<CallToolResult, McpError> {
        let RespondArgs {
            review_id,
            in_reply_to,
            action,
            body,
        } = args;
        let session = self
            .service
            .start_session(&self.repo, &review_id, &self.author)
            .await
            .map_err(into_mcp)?;
        let response = self
            .service
            .upsert_draft_response(
                &self.repo,
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
                "Code review tool. Use `list_reviews` and `get_review` to inspect changes; \
                 `draft_line_comment` / `draft_file_comment` / `draft_review_comment` to leave \
                 feedback (starts a draft session on first use); `respond` to reply or change \
                 resolution; `publish_session` once the round is complete."
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

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct GetReviewArgs {
    pub review_id: ReviewId,
    #[serde(default)]
    pub patchset: Option<u32>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct CreateReviewArgs {
    pub review_id: ReviewId,
    #[serde(default)]
    pub revset: Option<RevSet>,
    #[serde(default)]
    pub bookmark: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct StartSessionArgs {
    pub review_id: ReviewId,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct SessionArgs {
    pub review_id: ReviewId,
    pub session_id: SessionId,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct DraftLineCommentArgs {
    pub review_id: ReviewId,
    pub anchor_change_id: ChangeId,
    pub anchor_commit_id: CommitId,
    pub file: String,
    pub side: Side,
    pub lines: LineRange,
    pub flag: Flag,
    #[serde(default)]
    pub body: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct DraftFileCommentArgs {
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
    pub review_id: ReviewId,
    pub anchor_change_id: ChangeId,
    pub anchor_commit_id: CommitId,
    pub flag: Flag,
    #[serde(default)]
    pub body: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct RespondArgs {
    pub review_id: ReviewId,
    pub in_reply_to: CommentId,
    pub action: ResolutionAction,
    #[serde(default)]
    pub body: Option<String>,
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

/// Build an axum-mountable MCP service for a single repo. Mount one per repo
/// with `Router::new().nest_service("/mcp/<repo>", kata_mcp::mcp_service(...))`.
pub fn mcp_service(
    service: Arc<ReviewService>,
    repo: RepoId,
    author: Author,
) -> StreamableHttpService<ReviewMcp, LocalSessionManager> {
    let kata_mcp = ReviewMcp::new(service, repo, author);
    StreamableHttpService::new(
        move || Ok(kata_mcp.clone()),
        LocalSessionManager::default().into(),
        StreamableHttpServerConfig::default(),
    )
}
