use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::Parser;
use kata_core::{Author, RepoManifest, SCHEMA_VERSION};
use kata_jj::JjCli;
use kata_server::{
    AppState, ServerConfig, router_with_assets, router_with_embedded_assets,
};
use kata_service::ReviewService;
use kata_storage::{Storage, compute_repo_id, jj_repo_canonical_path};

#[derive(Debug, Parser)]
#[command(name = "kata", about = "HTTP server for the kata code-review tool")]
struct Args {
    /// jj working copies to serve. Pass multiple times. Each value is either
    /// a bare path (the slug is derived from the directory name) or the
    /// explicit form `name=path`.
    #[arg(long = "workspace", env = "KATA_WORKSPACE", required = true, num_args = 1..)]
    workspaces: Vec<String>,

    /// Directory where comments and manifests are stored.
    #[arg(long, env = "KATA_ROOT")]
    root: PathBuf,

    /// Identity used for writes when the client doesn't override it.
    #[arg(long, env = "KATA_AUTHOR")]
    author: String,

    /// `host:port` to bind on.
    #[arg(long, env = "KATA_BIND", default_value = "0.0.0.0:7878")]
    bind: SocketAddr,

    /// Override the embedded Svelte bundle with one served from disk
    /// (e.g. `web/dist` during local UI work). Omit to use the bundle
    /// compiled into the binary.
    #[arg(long, env = "KATA_WEB_DIR")]
    web_dir: Option<PathBuf>,

    /// Fallback identity used for MCP writes when a request doesn't pass
    /// `?as=<name>` on the URL. Defaults to `--author`. Per-request
    /// overrides via the query param let multiple agents (e.g. Claude
    /// vs. the human user) write distinct attribution — this is a
    /// stopgap until there's a real auth story.
    #[arg(long, env = "KATA_MCP_AUTHOR")]
    mcp_author: Option<String>,

    /// How often (in seconds) to poll each repo for branch movement so
    /// the UI can surface a "Refresh" affordance without the user
    /// reloading. Set to 0 to disable the background watcher entirely.
    #[arg(long, env = "KATA_BRANCH_POLL_SECS", default_value = "10")]
    branch_poll_secs: u64,

    /// Origin to allow on `/mcp` for browser-based MCP clients (e.g. the
    /// MCP inspector). Pass multiple times to allow several origins.
    /// Without this flag, `/mcp` returns no CORS headers and browsers
    /// refuse the cross-origin request — which is the safe default since
    /// the MCP endpoint is unauthenticated.
    #[arg(long = "mcp-cors-origin", env = "KATA_MCP_CORS_ORIGIN")]
    mcp_cors_origins: Vec<String>,
}

struct WorkspaceSpec {
    name: String,
    path: PathBuf,
}

fn parse_workspace(raw: &str) -> Result<WorkspaceSpec, String> {
    let (name, path) = match raw.split_once('=') {
        Some((n, p)) => (n.trim().to_string(), PathBuf::from(p)),
        None => {
            let path = PathBuf::from(raw);
            let name = derive_name(&path)
                .ok_or_else(|| format!("cannot derive slug from {raw:?}; use `name=path`"))?;
            (name, path)
        }
    };
    if name.is_empty() || !name.chars().all(is_slug_char) {
        return Err(format!(
            "workspace name {name:?} is not a valid url slug (use a-z, 0-9, -, _)",
        ));
    }
    Ok(WorkspaceSpec { name, path })
}

fn derive_name(path: &Path) -> Option<String> {
    path.file_name()
        .and_then(|s| s.to_str())
        .map(str::to_ascii_lowercase)
}

fn is_slug_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_'
}

async fn mcp_handler(
    axum::extract::State(dispatcher): axum::extract::State<kata_mcp::McpDispatcher>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
    req: axum::extract::Request,
) -> axum::response::Response {
    let author = params
        .get(kata_mcp::AUTHOR_QUERY_PARAM)
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| dispatcher.default_author().to_string());
    dispatcher
        .for_author(&author)
        .handle(req)
        .await
        .map(axum::body::Body::new)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                "info,\
                 kata=debug,kata_server=debug,\
                 kata_service=debug,kata_jj=debug,kata_storage=debug,\
                 tower_http=info"
                    .into()
            }),
        )
        .init();

    let args = Args::parse();
    let workspaces = args
        .workspaces
        .iter()
        .map(|raw| parse_workspace(raw))
        .collect::<Result<Vec<_>, _>>()?;
    let cfg = ServerConfig {
        review_root: args.root,
        author: Author::new(args.author),
        bind_addr: args.bind,
    };

    // `<review_root>/kata.db` keeps the SQLite file beside the per-repo
    // export directories. WAL journal mode + a partial UNIQUE index on
    // draft sessions make this safe with the multiple-writer pattern we
    // run (the user, the coding agent, and reviewer agents all touching
    // the same review). The filesystem-backed `Storage` impl is still
    // around but only via `kata export` / `kata import` — see the
    // storage-swap commits.
    if !cfg.review_root.exists() {
        std::fs::create_dir_all(&cfg.review_root)?;
    }
    let db_path = cfg.review_root.join("kata.db");
    let storage: Arc<dyn Storage> =
        Arc::new(kata_storage::sqlite::SqliteStorage::open(&db_path).await?);
    let mut builder = ReviewService::builder(storage.clone());
    let repo_count = workspaces.len();

    for WorkspaceSpec { name, path } in workspaces {
        let canonical = jj_repo_canonical_path(&path)?;
        let repo_id = compute_repo_id(&canonical);
        let canonical_str = canonical.to_string_lossy().into_owned();
        tracing::info!(repo = %name, repo_id = %repo_id, path = %canonical_str, "registering repo");
        storage
            .ensure_repo(&RepoManifest {
                schema_version: SCHEMA_VERSION,
                repo_id: repo_id.clone(),
                canonical_path: canonical_str.clone(),
            })
            .await?;
        let jj = Arc::new(JjCli::new(path));
        builder.add_repo(name, repo_id, canonical_str, jj)?;
    }

    let service = Arc::new(builder.build());

    if args.branch_poll_secs > 0 {
        let interval = std::time::Duration::from_secs(args.branch_poll_secs);
        tracing::info!(?interval, "starting branch watcher");
        service.clone().spawn_branch_watcher(interval);
    } else {
        tracing::info!("branch watcher disabled (--branch-poll-secs=0)");
    }
    let state = AppState {
        service: service.clone(),
        default_author: cfg.author.clone(),
    };

    let mut app = match &args.web_dir {
        Some(dir) => {
            tracing::info!(dir = ?dir, "serving web bundle from disk");
            router_with_assets(state, dir)
        }
        None => {
            tracing::info!("serving embedded web bundle");
            router_with_embedded_assets(state)
        }
    };

    let default_mcp_author = Author::new(
        args.mcp_author
            .clone()
            .unwrap_or_else(|| cfg.author.to_string()),
    );
    tracing::info!(
        default_author = %default_mcp_author,
        repos = repo_count,
        "mounting MCP at /mcp",
    );
    let dispatcher = kata_mcp::McpDispatcher::new(service.clone(), default_mcp_author);
    let mut mcp_router = axum::Router::new()
        .route("/", axum::routing::any(mcp_handler))
        .with_state(dispatcher);
    if !args.mcp_cors_origins.is_empty() {
        let origins = args
            .mcp_cors_origins
            .iter()
            .map(|o| {
                axum::http::HeaderValue::from_str(o)
                    .map_err(|e| format!("invalid --mcp-cors-origin {o:?}: {e}"))
            })
            .collect::<Result<Vec<_>, String>>()?;
        tracing::info!(origins = ?args.mcp_cors_origins, "enabling CORS on /mcp");
        let cors = tower_http::cors::CorsLayer::new()
            .allow_origin(tower_http::cors::AllowOrigin::list(origins))
            .allow_methods([
                axum::http::Method::GET,
                axum::http::Method::POST,
                axum::http::Method::DELETE,
                axum::http::Method::OPTIONS,
            ])
            .allow_headers([
                axum::http::header::CONTENT_TYPE,
                axum::http::header::ACCEPT,
                axum::http::HeaderName::from_static("mcp-session-id"),
                axum::http::HeaderName::from_static("mcp-protocol-version"),
            ])
            // Streamable HTTP threads a session id back to the client in
            // the initialize response; the browser only exposes it to JS
            // if we list it here.
            .expose_headers([axum::http::HeaderName::from_static("mcp-session-id")]);
        mcp_router = mcp_router.layer(cors);
    }
    app = app.nest("/mcp", mcp_router);

    let listener = tokio::net::TcpListener::bind(cfg.bind_addr).await?;
    tracing::info!(addr = %cfg.bind_addr, "kata listening");

    // We intentionally do not use `with_graceful_shutdown`: the SSE event
    // stream and the MCP `StreamableHttpService` are designed to stay open
    // forever, so a graceful drain never completes. Dropping the serve
    // future closes the listener and lets the process exit immediately.
    tokio::select! {
        res = axum::serve(listener, app) => res?,
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("shutting down");
        }
    }
    Ok(())
}
