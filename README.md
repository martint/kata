# Kata

Code review for [jujutsu](https://jj-vcs.github.io/jj/) (`jj`) repositories.

Kata exposes a review of a `jj` revset through a web UI and an MCP server.
Reviewers — human or agent — accumulate draft comments in a private session
and publish the whole batch atomically. Comments are anchored to specific
patchsets so they stay attached even after the author rewrites the change.

## Workspace layout

| Crate          | Purpose                                                       |
| -------------- | ------------------------------------------------------------- |
| `kata-core`    | Domain types (`ReviewId`, `ChangeId`, `Flag`, …).             |
| `kata-jj`      | `jj` CLI driver: bookmarks, revsets, diffs.                   |
| `kata-storage` | On-disk manifest + comment store.                             |
| `kata-service` | Repo-agnostic review service used by both transports.         |
| `kata-server`  | `axum` HTTP server; serves the API and the embedded web app.  |
| `kata-mcp`     | MCP transport (`StreamableHttpService`) over the same service.|

The Svelte frontend lives in `web/`.

## Build

```sh
# Requires Rust 1.95+, jj on PATH, and pnpm for the bundled web build.
cargo build --release

# To skip the web build (e.g. CI for crates other than kata-server):
KATA_SKIP_WEB_BUILD=1 cargo build
```

The release binary embeds `web/dist`; pass `--web-dir <path>` at runtime to
serve a different bundle (useful during UI development with `pnpm run dev`).

## Run

```sh
kata \
  --workspace main=/path/to/repo \
  --workspace other=/path/to/other-repo \
  --root  /var/lib/kata \
  --author "Jane Doe <jane@example.com>" \
  --bind  0.0.0.0:7878
```

Each `--workspace` adds a repo under a URL slug. The slug appears in routes
(`/api/repos/<slug>/...`) and in the MCP mount (`/mcp/<slug>`). Bare paths
derive the slug from the directory name; use `name=path` to override.

Environment variables mirror every flag: `KATA_WORKSPACE`, `KATA_ROOT`,
`KATA_AUTHOR`, `KATA_BIND`, `KATA_WEB_DIR`, `KATA_MCP_AUTHOR`.

## Web UI

Browse to the bind address. Permalinks have the shape
`/r/<repo>/<review_id>[?ps=<n>]`. The top bar carries the publish / discard
controls so they stay accessible while scrolling a long diff.

## MCP

Each registered workspace gets an MCP endpoint at `/mcp/<slug>`. The
transport is Streamable HTTP (`rmcp` 0.8). The server advertises both
**tools** and **resources** capabilities.

### Tools

`list_bookmarks`, `list_reviews`, `get_review`, `create_review`,
`start_session`, `publish_session`, `discard_session`,
`draft_line_comment`, `draft_file_comment`, `draft_review_comment`,
`respond`. The server is bound to one repo per endpoint, so no `repo`
argument is needed.

### Skill resource

The server publishes a `kata-review` skill (markdown + YAML frontmatter)
as an MCP resource:

```
URI:       skill://kata/review
mime_type: text/markdown
```

This follows the working group's resource-based path for skill
distribution (the `skill://` scheme proposed in SEP-2640 after SEP-2076
was closed). Today, Claude Code clients can reference it manually at the
prompt as `@<server>:skill://kata/review`. Auto-registration as a
native skill is tracked in
[anthropics/claude-code#38253](https://github.com/anthropics/claude-code/issues/38253).

The body is embedded into the binary at build time via `include_str!`,
so deploying a new server is enough to ship a new skill revision.

## License

Apache-2.0.
