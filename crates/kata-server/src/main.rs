use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::{Parser, Subcommand};
use kata_core::{Author, RepoManifest, SCHEMA_VERSION};
use kata_jj::JjLib;
use kata_server::auth::{AuthConfig, AuthMode, parse_upstream_cidr, validate_bind_safety};
use kata_server::config::TlsConfig;
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
    /// Manage per-author API tokens — long-lived bearer credentials
    /// for MCP agents and CI integrations that can't authenticate
    /// interactively. Tokens substitute for whatever per-request
    /// identity `--auth-mode` would otherwise determine.
    Token {
        #[command(subcommand)]
        action: TokenAction,
    },
}

#[derive(Debug, Subcommand)]
enum TokenAction {
    /// Mint a new token. The plaintext is printed once to stdout —
    /// save it, it's never shown again. Only the SHA-256 hash lands
    /// in the database.
    Create {
        /// Author identity the token authenticates as. The token's
        /// holder will appear as this author on every write.
        #[arg(long)]
        author: String,
        /// Free-form label for the token, e.g. `"ci-agent"` or
        /// `"laptop-mcp"`. Shown in `kata token list` so the
        /// operator can identify which token is which.
        #[arg(long)]
        name: String,
    },
    /// List all tokens (active and revoked), newest first.
    List,
    /// Revoke a token by its public id. The row is kept so the id
    /// can still be looked up in audit logs.
    Revoke {
        /// `token_id` as printed by `kata token list`.
        token_id: String,
    },
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
    /// Only consulted in `--auth-mode trust-client`; in `trust-
    /// forwarded-header` mode this default is never used (a request
    /// without the trusted header is a 401, not a fall-through).
    #[arg(long, env = "KATA_AUTHOR")]
    author: String,

    /// `host:port` to bind on. Default is loopback so a fresh install
    /// is safe by default. Override with `0.0.0.0:<port>` to expose
    /// on a network interface — also the right moment to think about
    /// `--auth-mode` (see below).
    #[arg(long, env = "KATA_BIND", default_value = "127.0.0.1:7878")]
    bind: SocketAddr,

    /// How the server decides who is acting on a request.
    /// `trust-client` (default) honours `X-Review-Author` on HTTP and
    /// `?as=` on MCP — safe on a localhost / single-user setup,
    /// unsafe for anything shared. `trust-forwarded-header` reads
    /// the actor from a header an upstream proxy is responsible for
    /// setting (`--auth-trusted-header`); use this when sitting
    /// behind oauth2-proxy / Authelia / Pomerium / similar.
    #[arg(long, env = "KATA_AUTH_MODE", value_enum, default_value_t = AuthMode::TrustClient)]
    auth_mode: AuthMode,

    /// Header to read the actor from in `--auth-mode trust-
    /// forwarded-header`. Defaults to `X-Forwarded-Email`, which is
    /// what `oauth2-proxy` sets by default. Header names are case-
    /// insensitive.
    #[arg(long, env = "KATA_AUTH_TRUSTED_HEADER", default_value = "X-Forwarded-Email")]
    auth_trusted_header: String,

    /// CIDR range allowed to set the trusted header. Pass multiple
    /// times for multiple ranges, or `0.0.0.0/0`/`::/0` to allow
    /// any source (only sensible inside an isolated network).
    /// Required when `--auth-mode trust-forwarded-header` is paired
    /// with a non-loopback bind — without it, the server refuses to
    /// start. Loopback binds skip the check entirely.
    #[arg(long = "auth-trust-upstream", env = "KATA_AUTH_TRUST_UPSTREAM", value_parser = parse_upstream_cidr)]
    auth_trust_upstream: Vec<ipnet::IpNet>,

    /// Path to a PEM-encoded TLS certificate chain. Pair with
    /// `--tls-key`. When both are set, the listener terminates TLS
    /// in-process via rustls. Omit both to serve plain HTTP and
    /// terminate TLS upstream. Mutually exclusive with `--tls-acme`.
    #[arg(long, env = "KATA_TLS_CERT", conflicts_with = "tls_acme")]
    tls_cert: Option<PathBuf>,

    /// Path to a PEM-encoded TLS private key matching `--tls-cert`.
    #[arg(long, env = "KATA_TLS_KEY", conflicts_with = "tls_acme")]
    tls_key: Option<PathBuf>,

    /// Domain to auto-issue a TLS certificate for via ACME (Let's
    /// Encrypt by default). When set, the listener terminates TLS
    /// using a certificate the server obtained itself; no
    /// `--tls-cert`/`--tls-key` are needed. The TLS-ALPN-01
    /// challenge runs on the same `--bind` listener, so no extra
    /// port is required. The domain must already resolve to this
    /// server by the time the first request lands.
    #[arg(long, env = "KATA_TLS_ACME")]
    tls_acme: Option<String>,

    /// Directory holding the ACME account key and the issued cert
    /// chain. Required whenever `--tls-acme` is set. Persisted
    /// across restarts — losing it forces a re-issuance and counts
    /// against Let's Encrypt's per-week rate limit, so back it up
    /// like any other server credential.
    #[arg(long, env = "KATA_TLS_ACME_CACHE", requires = "tls_acme")]
    tls_acme_cache: Option<PathBuf>,

    /// `mailto:` contact passed to the ACME CA when registering the
    /// account. Let's Encrypt uses it for expiry warnings. Strongly
    /// recommended but not required.
    #[arg(long, env = "KATA_TLS_ACME_CONTACT", requires = "tls_acme")]
    tls_acme_contact: Option<String>,

    /// Use Let's Encrypt's *staging* endpoint instead of production.
    /// Staging issues untrusted certificates (browsers will refuse
    /// without an override) but has no weekly rate limit, so it's
    /// the right pick while you're testing the deployment.
    #[arg(long, env = "KATA_TLS_ACME_STAGING", requires = "tls_acme")]
    tls_acme_staging: bool,

    /// OIDC issuer URL. When set with the OIDC client flags below
    /// and `--auth-mode oidc`, Kata speaks the OIDC authorization-
    /// code flow itself: a `/auth/login` route 302s to the issuer,
    /// `/auth/callback` validates the ID token, and a signed
    /// session cookie carries the email claim onto subsequent
    /// requests. Required when `--auth-mode oidc` is selected.
    #[arg(long, env = "KATA_OIDC_ISSUER")]
    oidc_issuer: Option<String>,

    /// OIDC client ID registered with the issuer.
    #[arg(long, env = "KATA_OIDC_CLIENT_ID", requires = "oidc_issuer")]
    oidc_client_id: Option<String>,

    /// OIDC client secret. Read from env in production to keep it
    /// out of process listings.
    #[arg(long, env = "KATA_OIDC_CLIENT_SECRET", requires = "oidc_issuer")]
    oidc_client_secret: Option<String>,

    /// Absolute redirect URI for the callback route. Must match what
    /// the IdP has registered for this client and match the
    /// scheme+host+port the SPA browses to (e.g.
    /// `https://kata.example.com/auth/callback`).
    #[arg(long, env = "KATA_OIDC_REDIRECT_URI", requires = "oidc_issuer")]
    oidc_redirect_uri: Option<String>,

    /// Secret bytes (as a UTF-8 string) used to sign session
    /// cookies. Rotating this invalidates every outstanding
    /// session. Generate via e.g. `openssl rand -base64 32`.
    #[arg(long, env = "KATA_OIDC_SESSION_SECRET", requires = "oidc_issuer")]
    oidc_session_secret: Option<String>,

    /// Session lifetime in seconds. The cookie's `Max-Age` matches
    /// and the embedded `exp` is enforced server-side, so a client
    /// can't extend its own session by lying about Max-Age.
    /// Defaults to 24 hours.
    #[arg(
        long,
        env = "KATA_OIDC_SESSION_SECONDS",
        default_value = "86400",
        requires = "oidc_issuer"
    )]
    oidc_session_seconds: i64,

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
        // The demo is single-user on the same host; the historical
        // client-supplied identity model is the right default.
        auth_mode: AuthMode::TrustClient,
        auth_trusted_header: "X-Forwarded-Email".into(),
        auth_trust_upstream: Vec::new(),
        tls_cert: None,
        tls_key: None,
        tls_acme: None,
        tls_acme_cache: None,
        tls_acme_contact: None,
        tls_acme_staging: false,
        oidc_issuer: None,
        oidc_client_id: None,
        oidc_client_secret: None,
        oidc_redirect_uri: None,
        oidc_session_secret: None,
        oidc_session_seconds: 86400,
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

/// Drive a `kata token` subcommand. All three operations open the
/// SQLite database directly via `ReviewService` (no jj backend, no
/// HTTP listener) — token management is pure storage work.
async fn run_token(
    db_path: PathBuf,
    action: TokenAction,
) -> Result<(), Box<dyn std::error::Error>> {
    let storage = SqliteStorage::open(&db_path).await?;
    let service = ReviewService::builder(Arc::new(storage)).build();
    match action {
        TokenAction::Create { author, name } => {
            let minted = kata_server::tokens::mint(Author::new(author), name);
            let stored = service.store_api_token(minted.token.clone()).await?;
            // Print the plaintext FIRST so a partial failure between
            // here and storage (impossible — we just persisted) can
            // never leak a usable token without telling the operator.
            // The audit metadata follows on stderr so plain
            // `kata token create | clip` captures only the secret.
            println!("{}", minted.plaintext);
            eprintln!();
            eprintln!("Save this token — it won't be shown again.");
            eprintln!();
            eprintln!("  token_id : {}", stored.token_id);
            eprintln!("  author   : {}", stored.author);
            eprintln!("  name     : {}", stored.name);
            eprintln!("  prefix   : {}", stored.prefix);
            eprintln!("  created  : {}", stored.created_at);
        }
        TokenAction::List => {
            let rows = service.list_api_tokens().await?;
            if rows.is_empty() {
                println!("No API tokens issued.");
                return Ok(());
            }
            println!(
                "{:<38}  {:<28}  {:<16}  {:<18}  {:<25}  {}",
                "token_id", "author", "name", "prefix", "created_at", "status",
            );
            for t in rows {
                let status = match &t.revoked_at {
                    Some(rev) => format!("revoked {}", rev),
                    None => match &t.last_used_at {
                        Some(used) => format!("active (last used {})", used),
                        None => "active (unused)".to_string(),
                    },
                };
                println!(
                    "{:<38}  {:<28}  {:<16}  {:<18}  {:<25}  {}",
                    t.token_id.as_str(),
                    t.author.as_str(),
                    t.name,
                    t.prefix,
                    t.created_at,
                    status,
                );
            }
        }
        TokenAction::Revoke { token_id } => {
            service
                .revoke_api_token(&kata_core::ApiTokenId::new(token_id.clone()))
                .await?;
            eprintln!("revoked token {token_id}");
        }
    }
    Ok(())
}

/// State the `/mcp` handler reads each request. The auth config is
/// alongside the dispatcher because the actor lookup is mode-aware:
/// in `trust-client` mode the historical `?as=` query param wins
/// (with the dispatcher's default as fallback); in `trust-
/// forwarded-header` mode the configured trusted header is the only
/// source, and a missing header is a 401.
#[derive(Clone)]
struct McpState {
    dispatcher: kata_mcp::McpDispatcher,
    auth: AuthConfig,
    /// Service handle for API-token lookups. The HTTP path's
    /// `ViewerAuthor` extractor reads from `AppState.service`; MCP
    /// has its own state struct, so a separate Arc rides along here.
    service: Arc<kata_service::ReviewService>,
}

async fn mcp_handler(
    axum::extract::State(state): axum::extract::State<McpState>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
    req: axum::extract::Request,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    // API token wins over the mode-specific identity, same as on
    // HTTP. Token presence in `Authorization: Bearer` or `?token=`
    // short-circuits the rest of the lookup. A presented-but-bad
    // token is a 401 — falling through silently would mask the
    // misconfiguration.
    let bearer = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer ").or_else(|| s.strip_prefix("bearer ")))
        .map(|s| s.trim().to_owned());
    let query_token = params.get("token").map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
    let presented = bearer.or(query_token);
    if let Some(plaintext) = presented {
        if kata_server::tokens::looks_like_token(&plaintext) {
            let hash = kata_server::tokens::hash(&plaintext);
            match state.service.authenticate_api_token(&hash).await {
                Ok(Some(t)) => {
                    return state
                        .dispatcher
                        .for_author(t.author.as_str())
                        .handle(req)
                        .await
                        .map(axum::body::Body::new);
                }
                Ok(None) => {
                    return (
                        axum::http::StatusCode::UNAUTHORIZED,
                        "api token unknown or revoked\n",
                    )
                        .into_response();
                }
                Err(e) => {
                    tracing::error!(error = ?e, "api token lookup failed");
                    return (
                        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                        "token lookup failed\n",
                    )
                        .into_response();
                }
            }
        }
    }
    let author = match state.auth.mode {
        AuthMode::TrustClient => params
            .get(kata_mcp::AUTHOR_QUERY_PARAM)
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| state.dispatcher.default_author().to_string()),
        AuthMode::TrustForwardedHeader => {
            let header = &state.auth.trusted_header;
            let value = req.headers().get(header.as_str()).and_then(|v| v.to_str().ok());
            match value.map(str::trim).filter(|s| !s.is_empty()) {
                Some(s) => s.to_owned(),
                None => {
                    tracing::warn!(
                        header = header.as_str(),
                        "MCP request missing trusted header (auth-mode=trust-forwarded-header)",
                    );
                    return (
                        axum::http::StatusCode::UNAUTHORIZED,
                        format!("Missing {header} header\n"),
                    )
                        .into_response();
                }
            }
        }
        AuthMode::Oidc => {
            // The OIDC session cookie is set by the browser-side
            // login flow and isn't meaningful for MCP agents.
            // Agents authenticate via the API-token path checked
            // above; falling through here means no token was
            // presented (or it was bad), and the right answer is
            // 401 — never the default author, since OIDC mode
            // implies "no client-trusted identities".
            return (
                axum::http::StatusCode::UNAUTHORIZED,
                "MCP under --auth-mode oidc requires an API token \
                 (Authorization: Bearer or ?token=); see `kata token create`\n",
            )
                .into_response();
        }
    };
    state
        .dispatcher
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
        Command::Token { action } => run_token(db_path, action).await,
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

    // Build the auth config, then run the bind/mode safety checks
    // before any I/O. We want a misconfigured deployment to die at
    // startup with a clear message, not silently begin serving in a
    // shape the operator didn't intend.
    //
    // OIDC mode requires the full four-tuple of OIDC flags (issuer,
    // client id/secret, redirect URI, session secret). Anything less
    // is a startup failure rather than a runtime 500 on the first
    // login attempt.
    let oidc_cfg = if args.auth_mode == AuthMode::Oidc {
        let issuer = args
            .oidc_issuer
            .clone()
            .ok_or("--auth-mode oidc requires --oidc-issuer")?;
        let client_id = args
            .oidc_client_id
            .clone()
            .ok_or("--auth-mode oidc requires --oidc-client-id")?;
        let client_secret = args
            .oidc_client_secret
            .clone()
            .ok_or("--auth-mode oidc requires --oidc-client-secret")?;
        let redirect_uri = args
            .oidc_redirect_uri
            .clone()
            .ok_or("--auth-mode oidc requires --oidc-redirect-uri")?;
        let session_secret = args
            .oidc_session_secret
            .clone()
            .ok_or("--auth-mode oidc requires --oidc-session-secret")?;
        if session_secret.len() < 16 {
            return Err(
                "--oidc-session-secret should be at least 16 bytes; \
                 generate via `openssl rand -base64 32`"
                    .into(),
            );
        }
        Some(kata_server::auth::OidcConfig {
            issuer,
            client_id,
            client_secret,
            redirect_uri,
            session_secret: session_secret.into_bytes(),
            session_seconds: args.oidc_session_seconds,
        })
    } else {
        if args.oidc_issuer.is_some()
            || args.oidc_client_id.is_some()
            || args.oidc_client_secret.is_some()
            || args.oidc_redirect_uri.is_some()
            || args.oidc_session_secret.is_some()
        {
            tracing::warn!(
                "OIDC flags are set but --auth-mode is {:?}; the OIDC settings are ignored",
                args.auth_mode,
            );
        }
        None
    };
    let auth = AuthConfig {
        mode: args.auth_mode,
        trusted_header: args.auth_trusted_header.clone(),
        upstream_allowlist: args.auth_trust_upstream.clone(),
        oidc: oidc_cfg,
    };
    // Three TLS shapes, mutually exclusive: PEM file pair, ACME
    // auto-issuance, or plain HTTP (terminate TLS upstream). clap's
    // `conflicts_with` catches the pem-vs-acme combination at parse
    // time; we re-check the pem-pair invariant by hand because a
    // half-set pair (only --tls-cert or only --tls-key) is a
    // configuration mistake clap can't express directly.
    let tls = match (args.tls_cert.as_ref(), args.tls_key.as_ref(), args.tls_acme.as_ref()) {
        (Some(cert), Some(key), None) => Some(TlsConfig::Pem {
            cert_path: cert.clone(),
            key_path: key.clone(),
        }),
        (Some(_), None, None) | (None, Some(_), None) => {
            return Err(
                "--tls-cert and --tls-key must be set together (or both omitted)".into(),
            );
        }
        (None, None, Some(domain)) => {
            let cache_dir = args
                .tls_acme_cache
                .clone()
                .ok_or("--tls-acme-cache is required when --tls-acme is set")?;
            Some(TlsConfig::Acme {
                domain: domain.clone(),
                cache_dir,
                contact: args.tls_acme_contact.clone(),
                staging: args.tls_acme_staging,
            })
        }
        (None, None, None) => None,
        _ => unreachable!("clap's conflicts_with rejects pem+acme combinations"),
    };
    if let Err(e) = validate_bind_safety(args.bind, &auth) {
        return Err(e.to_string().into());
    }

    let cfg = ServerConfig {
        review_root: data.clone(),
        author: Author::new(args.author.clone()),
        bind_addr: args.bind,
        auth: auth.clone(),
        tls: tls.clone(),
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
    // OIDC discovery + client build. Runs async because the
    // discovery document is fetched over HTTP from the IdP; a
    // failure here is fatal (we can't serve `/auth/login` without
    // a working client and we'd 500 every request in OIDC mode
    // anyway). Skipped entirely in non-OIDC modes.
    let oidc_runtime = match cfg.auth.oidc.clone() {
        Some(oidc_cfg) => {
            tracing::info!(issuer = %oidc_cfg.issuer, "discovering OIDC provider");
            Some(kata_server::oidc::build_runtime(oidc_cfg).await?)
        }
        None => None,
    };
    let state = AppState {
        service: service.clone(),
        default_author: cfg.author.clone(),
        auth: cfg.auth.clone(),
        bind_addr: cfg.bind_addr,
        oidc: oidc_runtime,
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
    let mcp_state = McpState {
        dispatcher,
        auth: cfg.auth.clone(),
        service: service.clone(),
    };
    let mut mcp_router = axum::Router::new()
        .route("/", axum::routing::any(mcp_handler))
        .with_state(mcp_state);
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

    // The upstream-IP guard layer reads `ConnectInfo<SocketAddr>` per
    // request, so the make-service has to be built with connect-info
    // for both the plain and TLS paths. The layer itself is a thin
    // wrapper that only does work in trust-forwarded-header mode on a
    // non-loopback bind.
    let guard_state = GuardState {
        auth: cfg.auth.clone(),
        bind: cfg.bind_addr,
    };
    let app = app.layer(axum::middleware::from_fn_with_state(
        guard_state,
        upstream_guard,
    ));
    let make_service = app.into_make_service_with_connect_info::<SocketAddr>();

    // axum-server replaces axum::serve so we can wrap the same listener
    // in rustls for the TLS branch without spinning up two different
    // serve futures. The graceful-shutdown caveat from before still
    // holds — we never call it for the SSE / MCP reasons noted in the
    // old code — so a ctrl-c just drops the future.
    let scheme = if cfg.tls.is_some() { "https" } else { "http" };
    tracing::info!(addr = %cfg.bind_addr, scheme, auth_mode = ?cfg.auth.mode, "kata listening");
    if cfg.auth.mode == AuthMode::TrustClient && !cfg.bind_addr.ip().is_loopback() {
        // Loud at startup because the combination is silently
        // catastrophic — any client on the network can claim any
        // identity. We accept the deployment but make sure the
        // operator sees it in their logs.
        tracing::warn!(
            "auth-mode=trust-client on a non-loopback bind ({}). Any caller can claim any identity. \
             Move auth to a fronting proxy and switch to --auth-mode trust-forwarded-header.",
            cfg.bind_addr,
        );
    }
    let serve = async move {
        match &cfg.tls {
            Some(TlsConfig::Pem { cert_path, key_path }) => {
                let tls_cfg = axum_server::tls_rustls::RustlsConfig::from_pem_file(
                    cert_path, key_path,
                )
                .await
                .map_err(|e| {
                    format!(
                        "loading TLS cert/key from {:?} / {:?}: {e}",
                        cert_path, key_path,
                    )
                })?;
                axum_server::bind_rustls(cfg.bind_addr, tls_cfg)
                    .serve(make_service)
                    .await
                    .map_err(|e| -> Box<dyn std::error::Error> { Box::new(e) })
            }
            Some(TlsConfig::Acme { domain, cache_dir, contact, staging }) => {
                serve_acme(
                    cfg.bind_addr,
                    domain.clone(),
                    cache_dir.clone(),
                    contact.clone(),
                    *staging,
                    make_service,
                )
                .await
            }
            None => axum_server::bind(cfg.bind_addr)
                .serve(make_service)
                .await
                .map_err(|e| -> Box<dyn std::error::Error> { Box::new(e) }),
        }
    };
    tokio::select! {
        res = serve => res?,
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("shutting down");
        }
    }
    Ok(())
}

/// Run the application behind an ACME-issued certificate. Spawns a
/// background task that polls the ACME state stream so renewals
/// happen ahead of expiry; the acceptor itself hot-swaps the cert
/// as it gets renewed, so no restart is needed at the 60-day mark.
async fn serve_acme(
    bind_addr: SocketAddr,
    domain: String,
    cache_dir: PathBuf,
    contact: Option<String>,
    staging: bool,
    make_service: axum::extract::connect_info::IntoMakeServiceWithConnectInfo<
        axum::Router,
        SocketAddr,
    >,
) -> Result<(), Box<dyn std::error::Error>> {
    use futures::StreamExt;
    use rustls_acme::AcmeConfig;
    use rustls_acme::caches::DirCache;

    std::fs::create_dir_all(&cache_dir).map_err(|e| {
        format!("creating ACME cache directory {:?}: {e}", cache_dir)
    })?;

    let mut state = {
        let mut cfg = AcmeConfig::new([domain.clone()])
            .cache(DirCache::new(cache_dir.clone()));
        if let Some(c) = contact.clone() {
            // rustls-acme expects each contact pre-formatted as a URI
            // (e.g. `mailto:admin@example.com`). We accept the bare
            // email too, and add the `mailto:` for the operator if
            // it's missing.
            let normalised = if c.contains(':') { c } else { format!("mailto:{c}") };
            cfg = cfg.contact([normalised]);
        }
        cfg.directory_lets_encrypt(!staging).state()
    };
    let acceptor = state.axum_acceptor(state.default_rustls_config());

    // Drain the ACME state stream so renewals proceed. Without
    // someone polling `state.next()`, the background driver never
    // ticks and the cert never refreshes. Logged at info-level for
    // visibility; failures don't abort the server (we still have a
    // valid cert until it expires).
    let dir_label = if staging { "staging" } else { "production" };
    tracing::info!(
        domain = %domain,
        cache = %cache_dir.display(),
        directory = dir_label,
        "ACME enabled",
    );
    tokio::spawn(async move {
        while let Some(event) = state.next().await {
            match event {
                Ok(ok) => tracing::info!(?ok, "ACME state event"),
                Err(e) => tracing::error!(error = %e, "ACME error"),
            }
        }
    });

    axum_server::bind(bind_addr)
        .acceptor(acceptor)
        .serve(make_service)
        .await
        .map_err(|e| -> Box<dyn std::error::Error> { Box::new(e) })
}

/// State bundle the upstream-guard middleware needs. Kept separate
/// from `AppState` so the layer doesn't pull in the whole service
/// just to read two fields.
#[derive(Clone)]
struct GuardState {
    auth: AuthConfig,
    bind: SocketAddr,
}

/// Per-request middleware: in trust-forwarded-header mode on a non-
/// loopback bind, reject requests whose remote address isn't in the
/// configured upstream allowlist. The check is cheap (a slice scan
/// over a handful of CIDRs) and short-circuits entirely for the
/// other branches.
async fn upstream_guard(
    axum::extract::State(state): axum::extract::State<GuardState>,
    axum::extract::ConnectInfo(remote): axum::extract::ConnectInfo<SocketAddr>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    if state.auth.enforce_allowlist(state.bind) && !state.auth.upstream_allowed(remote.ip()) {
        tracing::warn!(
            remote = %remote.ip(),
            "rejecting request: source not in --auth-trust-upstream allowlist",
        );
        return (
            axum::http::StatusCode::FORBIDDEN,
            "Forbidden: source not in upstream allowlist\n",
        )
            .into_response();
    }
    next.run(req).await
}
