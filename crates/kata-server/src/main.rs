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
use kata_storage::{FilesystemStorage, Storage, compute_repo_id, jj_repo_canonical_path};

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

    /// Identity used for MCP writes (agents). Defaults to the same value
    /// as `--author`; override to give the agent a distinct identity.
    #[arg(long, env = "KATA_MCP_AUTHOR")]
    mcp_author: Option<String>,
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

    let storage: Arc<dyn Storage> = Arc::new(FilesystemStorage::new(cfg.review_root.clone()));
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

    let mcp_author = Author::new(
        args.mcp_author
            .clone()
            .unwrap_or_else(|| cfg.author.to_string()),
    );
    tracing::info!(
        author = %mcp_author,
        repos = repo_count,
        "mounting MCP at /mcp",
    );
    app = app.nest_service(
        "/mcp",
        kata_mcp::mcp_service(service.clone(), mcp_author.clone()),
    );

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
