use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::{Parser, Subcommand};
use kata_core::{Author, RepoManifest, SCHEMA_VERSION};
use kata_jj::JjLib;
use kata_server::{
    AppState, ServerConfig, router_with_assets, router_with_embedded_assets,
};
use kata_service::ReviewService;
use kata_storage::sqlite::SqliteStorage;
use kata_storage::{Storage, archive, compute_repo_id, jj_repo_canonical_path};

#[derive(Debug, Parser)]
#[command(name = "kata", about = "Code-review tool: server + archive tooling")]
struct Cli {
    /// Storage directory. `kata.db` lives here; `kata export` and
    /// `kata import` use sibling directories under it.
    #[arg(long, env = "KATA_DATA", global = true)]
    data: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the HTTP server (this is the long-lived process the web UI
    /// and MCP clients connect to).
    Serve(ServeArgs),
    /// Snapshot the SQLite database into a directory of TOML + Markdown
    /// files. The output format is intentionally stable across schema
    /// changes so it survives migrations and is friendly to other tools
    /// (grep, rsync, version control).
    Export {
        /// Destination directory. Created if missing. Files inside are
        /// overwritten atomically.
        dir: PathBuf,
    },
    /// Load a previously-exported directory into a fresh SQLite
    /// database. Errors if the database already contains overlapping
    /// rows — point `import` at an empty `--data` (the typical use is
    /// the one-shot migration from the old filesystem-only store).
    Import {
        /// Source directory written by a previous `kata export`.
        dir: PathBuf,
        /// Skip the interactive confirmation that triggers when the
        /// target database already has rows. Use in scripts or when
        /// you've already accepted that the import may error mid-way
        /// on ID conflicts.
        #[arg(long)]
        force: bool,
    },
    /// Seed a self-contained demo workspace + database and start the
    /// server pointed at it. The frontend's `?demo=1` overlay
    /// narrates a guided tour through the seeded data. Seeding
    /// shells out to `jj` (the only `kata` subcommand that needs
    /// the binary at all — `serve` itself does not).
    Demo(DemoArgs),
}

#[derive(Debug, Parser)]
struct DemoArgs {
    /// `host:port` to bind on. Same default as `serve`.
    #[arg(long, env = "KATA_BIND", default_value = "127.0.0.1:7878")]
    bind: SocketAddr,
    /// Identity used for writes from the running browser session.
    /// Defaults to the demo's seeded author so the UI doesn't
    /// look like a stranger walked in.
    #[arg(long, env = "KATA_AUTHOR", default_value = "alice@example.com")]
    author: String,
}

#[derive(Debug, Parser)]
struct ServeArgs {
    /// jj working copies to serve. Pass multiple times. Each value is either
    /// a bare path (the slug is derived from the directory name) or the
    /// explicit form `name=path`.
    #[arg(long = "workspace", env = "KATA_WORKSPACE", required = true, num_args = 1..)]
    workspaces: Vec<String>,

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

/// Seed the demo workspace + database under `data`, then start the
/// regular HTTP server pointed at it. `data` is whatever the global
/// `--data` flag resolved to; in the typical demo flow that's a
/// tempdir the user wants thrown away on exit, but we honour an
/// explicit `--data` too so the same invocation can rebuild a
/// reproducible demo state in a known location for screenshotting,
/// bug repro, etc.
async fn run_demo(
    data: PathBuf,
    args: DemoArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    let seeded = kata_demo::seed_demo(&data).await?;
    tracing::info!(
        repo = %seeded.repo_name,
        workspace = %seeded.workspace_path.display(),
        bind = %args.bind,
        "demo seeded; starting server",
    );
    // The seeded workspace is just one of the workspaces the
    // server registers — reuse `serve` instead of duplicating the
    // build path. The seeded review is at /r/<repo>/1 once the
    // browser hits the bind address.
    let workspace_arg = format!(
        "{}={}",
        seeded.repo_name,
        seeded.workspace_path.display()
    );
    let serve_args = ServeArgs {
        workspaces: vec![workspace_arg],
        author: args.author,
        bind: args.bind,
        web_dir: None,
        mcp_author: None,
        // Demo runs locally; no point polling jj for branch
        // movement on a workspace nobody else touches.
        branch_poll_secs: 0,
        mcp_cors_origins: Vec::new(),
    };
    serve(data.clone(), seeded.db_path, serve_args).await
}

/// Print a warning that the target DB has data and read a y/N answer
/// from stdin. Anything other than "y" / "yes" is taken as no.
///
/// Lives on the import path specifically because that's the only
/// command where running on top of existing data is plausibly a
/// mistake — `serve` is meant to run on a populated DB, and `export`
/// is read-only.
fn confirm_proceed(db_path: &Path) -> std::io::Result<bool> {
    use std::io::{BufRead, Write};
    eprintln!(
        "Database {} already contains data.\n\
         Importing on top will error on any ID overlap, and the import is\n\
         row-by-row with no global rollback — a conflict mid-stream leaves\n\
         a partial state. For a clean retry, delete `kata.db` first.\n",
        db_path.display()
    );
    eprint!("Continue? [y/N] ");
    std::io::stderr().flush()?;
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    Ok(matches!(
        line.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
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

    let cli = Cli::parse();
    let data = cli.data.ok_or("--data (or KATA_DATA) is required")?;
    let db_path = data.join("kata.db");
    // Every subcommand wants to open `<data>/kata.db`. Create the
    // parent first so SQLite doesn't fail with "unable to open
    // database file" on a fresh `--data`.
    if !data.exists() {
        std::fs::create_dir_all(&data)?;
    }

    match cli.command {
        Command::Serve(args) => serve(data, db_path, args).await,
        Command::Demo(demo_args) => run_demo(data, demo_args).await,
        Command::Export { dir } => {
            // Open the existing DB read-only conceptually — we don't
            // touch it, but the SqliteStorage abstraction always opens
            // r/w and runs pending migrations. That's the right call:
            // an export from a schema-newer DB into a directory readable
            // by a schema-older importer is exactly the workflow we
            // want to keep working.
            let storage = SqliteStorage::open(&db_path).await?;
            archive::export(&storage, &dir).await?;
            tracing::info!(dest = ?dir, "export complete");
            Ok(())
        }
        Command::Import { dir, force } => {
            let storage = SqliteStorage::open(&db_path).await?;
            // Importing on top of an already-populated database is
            // almost always a mistake (forgot to wipe, pointed at the
            // wrong --data). Surface it loudly. On confirmation we
            // proceed — the import is row-by-row with no global
            // rollback, so an ID overlap mid-stream leaves a partial
            // state. The prompt message says so.
            if !force && !storage.list_all_repos().await?.is_empty() {
                if !confirm_proceed(&db_path)? {
                    return Err("import aborted by user".into());
                }
            }
            archive::import(&dir, &storage).await?;
            tracing::info!(src = ?dir, "import complete");
            Ok(())
        }
    }
}

async fn serve(
    data: PathBuf,
    db_path: PathBuf,
    args: ServeArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    let workspaces = args
        .workspaces
        .iter()
        .map(|raw| parse_workspace(raw))
        .collect::<Result<Vec<_>, _>>()?;
    let cfg = ServerConfig {
        review_root: data.clone(),
        author: Author::new(args.author.clone()),
        bind_addr: args.bind,
    };

    // `kata.db` lives at `--data/kata.db`. WAL journal mode + a partial
    // UNIQUE index on draft sessions make this safe with the
    // multi-writer pattern we run (user + coding agent + reviewer
    // agents touching the same review at once).
    let storage: Arc<dyn Storage> = Arc::new(SqliteStorage::open(&db_path).await?);
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
        let jj = Arc::new(JjLib::new(path)?);
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
