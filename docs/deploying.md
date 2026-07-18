# Deploying Kata for a team

The defaults — `--bind 127.0.0.1:7878` and `--auth-mode trust-client`
— are safe on a single-user laptop and **wrong** for anything shared.
`trust-client` reads the actor from a client-supplied header
(`X-Review-Author`) or `?as=` on MCP, so any caller on the network
can claim any identity. Anything beyond a single user needs both TLS
and a real auth source.

There are three production-ready setups. Pick the one that fits the
operations model you already have.

1. **Behind a reverse proxy that handles auth.** Recommended for
   most deployments — TLS, identity, and observability all live in
   one place upstream of Kata.
2. **Built-in OIDC.** Same end-to-end story without a fronting
   proxy. Suitable for a single-VM deployment where adding
   `oauth2-proxy` (or similar) is more friction than it's worth.
3. **Operator-supplied TLS cert** or **ACME / Let's Encrypt** —
   covers TLS termination when you want a single-binary deployment.
   Pair with one of the auth modes above; TLS alone is not auth.

## Container images

Every `vX.Y.Z` tag on `main` publishes a multi-arch (linux/amd64 +
linux/arm64) image to `ghcr.io/martint/kata:<version>` and
`ghcr.io/martint/kata:latest`. The binary's only runtime dep is
glibc (sqlite is bundled, TLS uses rustls, the frontend is
embedded); the release image is debian-slim + the binary, ~80 MB.

Use [`docker-compose.yml.example`](../docker-compose.yml.example)
at the repo root as the deployment template — same file works
whether you pull the published image or rebuild from your working
copy:

```sh
# Copy the templates and fill in your config.
cp docker-compose.yml.example docker-compose.yml
cp .env.example .env       # then edit: set KATA_WORKSPACES_DIR

# Pull + run the published image.
docker compose pull
docker compose up -d

# Build from your working copy instead (useful while iterating on
# kata's code, or to run a fork).
docker compose up --build
```

Both `docker-compose.yml` and `.env` are gitignored so per-host
customisation (extra bind mounts, OIDC secrets in `.env`, etc.)
stays out of the upstream repo.

The compose file requires one piece of operator config —
`KATA_WORKSPACES_DIR`, a host directory holding your jj repos — and
ships sensible defaults for everything else (listens on
`127.0.0.1:7878`, writes to a named volume at `/data`, scans
`/workspaces` live so dropping a repo into the host dir
registers it without a restart).

By default the container runs as root, which writes root-owned
files into the `/data` and `/workspaces` bind mounts. Tell
compose to run as your host user instead by exporting
`KATA_UID` / `KATA_GID` — either once on the command line:

```sh
KATA_UID=$(id -u) KATA_GID=$(id -g) docker compose up -d
```

or by adding the resolved numbers to `.env`:

```sh
echo "KATA_UID=$(id -u)" >> .env
echo "KATA_GID=$(id -g)" >> .env
```

The image makes `/data` and the container's `HOME` world-
writable so any UID can run it without a chown step inside the
container.

The defaults are tuned for **local development with agents** —
compose publishes the port on `127.0.0.1` only + trust-client auth
so Claude Code via MCP works on the same host without
token-minting ceremony. For any shared deployment, set
`KATA_PUBLISH_ADDR=0.0.0.0` **and** a real `KATA_AUTH_MODE` (plus
TLS / OIDC params) in `.env`. (`KATA_PUBLISH_ADDR` is the host
interface compose forwards the port on; the container itself
always binds `0.0.0.0:7878` internally — `KATA_BIND` in the
Dockerfile.) The auth modes are covered below; every CLI flag has
a `KATA_*` env var, so the recipes that follow translate directly
to lines in `.env`.

A `--profile demo` service in the same compose ships the seeded
guided tour for a zero-config kata test drive:

```sh
docker compose --profile demo up
```

## 1. Behind a reverse proxy (recommended)

Terminate TLS and authenticate the user upstream
(`oauth2-proxy` / Authelia / Pomerium / Caddy with the
[`forward_auth`](https://caddyserver.com/docs/caddyfile/directives/forward_auth)
directive). Configure the proxy to forward the authenticated user's
email to Kata in a header.

Then run Kata in `trust-forwarded-header` mode, with an upstream
allowlist that names the proxy's source IP/CIDR:

```sh
kata serve \
  --workspace main=/path/to/repo \
  --data /var/lib/kata \
  --author "system@example.com" \
  --bind 0.0.0.0:7878 \
  --auth-mode trust-forwarded-header \
  --auth-trusted-header X-Forwarded-Email \
  --auth-trust-upstream 10.0.0.5/32
```

In this mode client-supplied identity (`X-Review-Author` / `?as=`)
is ignored. A request that reaches Kata without the trusted header,
or from outside the allowlist, gets a 401 / 403. Loopback binds
skip the allowlist check (only same-host processes can connect).

## 2. Built-in OIDC (single-binary)

Kata can speak the OIDC authorization-code flow itself, which gets
you a single-binary deployment without a fronting auth proxy:

```sh
kata serve \
  --workspace main=/path/to/repo \
  --data /var/lib/kata \
  --author "system@example.com" \
  --bind 0.0.0.0:443 \
  --tls-acme review.example.com \
  --tls-acme-cache /var/lib/kata/acme \
  --auth-mode oidc \
  --oidc-issuer https://accounts.google.com \
  --oidc-client-id "$OIDC_CLIENT_ID" \
  --oidc-client-secret "$OIDC_CLIENT_SECRET" \
  --oidc-redirect-uri https://review.example.com/auth/callback \
  --oidc-session-secret "$OIDC_SESSION_SECRET"
```

Generate the session secret with `openssl rand -base64 32` and keep
it stable across restarts (rotating it invalidates every outstanding
session). On first visit the SPA bounces through `/auth/login` →
the IdP → `/auth/callback`, the email claim becomes the author
identity, and a 24-hour HMAC-signed cookie carries it on every
subsequent request.

MCP agents in OIDC mode authenticate via API tokens (see below);
session cookies are browser-only.

## Admins (global)

By default the only person who can edit a review's revset or summary,
archive/delete it, or write author annotations is its **creator**. A
global **admin** passes every one of those gates on *every* review —
exactly as if they were its creator. Actions stay attributed to the
admin's own identity (an admin-written annotation shows the admin, not
the original creator); this is a permissions grant, not impersonation.

Admin status is decided from the identity Kata already resolved, so it
works in **every auth mode** (and for API-token callers). There are two
sources, OR'd together:

**By email** — a static allowlist, valid in all modes. Repeat the flag
or pass a comma-separated env var; matched case-insensitively after
trimming:

```sh
kata serve … \
  --admin-email alice@example.com \
  --admin-email ops@example.com
# or: KATA_ADMIN_EMAILS="alice@example.com,ops@example.com"
```

**By proxy group** — in `trust-forwarded-header` mode, membership in a
named group from a proxy-supplied groups header (e.g. Authelia's
`Remote-Groups`). The groups header is trusted on the same basis as the
email header — i.e. only because the request came through the proxy your
`--auth-trust-upstream` allowlist vouches for:

```sh
kata serve … \
  --auth-mode trust-forwarded-header \
  --auth-trusted-header Remote-Email \
  --auth-trust-upstream 10.0.0.5/32 \
  --admin-group kata-admins \
  --auth-groups-header Remote-Groups   # this is the default
```

Group names are matched exactly (groups are case-sensitive on most
IdPs). The group source is consulted only in `trust-forwarded-header`
mode; in OIDC and `trust-client` modes use the email allowlist. MCP
admins are recognised by the email allowlist (so an API token whose
author is listed acts as an admin); the per-request group header is
HTTP-only.

With no `--admin-email` / `--admin-group` configured there are no
admins and every gate behaves exactly as before.

## TLS termination

The reverse-proxy recipe above already covers TLS upstream. If
you're going proxy-less, Kata terminates TLS in-process via rustls.
Two flavours:

**Operator-supplied cert** — renewal lives outside Kata:

```sh
kata serve --tls-cert /etc/kata/cert.pem --tls-key /etc/kata/key.pem ...
```

Refreshing the cert is the operator's job (cert-bot, ACME script,
etc.) plus a server restart.

**ACME / Let's Encrypt** — Kata talks ACME itself:

```sh
kata serve \
  --tls-acme review.example.com \
  --tls-acme-cache /var/lib/kata/acme \
  --tls-acme-contact mailto:ops@example.com \
  --bind 0.0.0.0:443 ...
```

The TLS-ALPN-01 challenge runs on the same `--bind` listener — no
extra port. The cache directory holds the ACME account key and
issued cert across restarts; back it up like any other server
credential (losing it forces re-issuance against Let's Encrypt's
weekly rate limit). Add `--tls-acme-staging` while you're testing —
staging issues untrusted certs but has no rate limit. Mutually
exclusive with `--tls-cert` / `--tls-key`.

## API tokens for agents

MCP agents and CI integrations can't go through an interactive
auth flow. Mint a long-lived bearer credential bound to an author
identity:

```sh
kata token create --author ci-agent@example.com --name "github-actions"
# Plaintext is printed once. Save it; only the SHA-256 hash is stored.
```

The agent presents the token in either spot:

- `Authorization: Bearer <token>` on HTTP.
- `?token=<token>` on the URL — primarily for MCP clients that can
  only set query parameters.

A valid token authenticates as its bound author regardless of
`--auth-mode`, so tokens work in all three modes. In OIDC mode
they're the only way to authenticate non-browser clients, since
session cookies don't apply. Manage with `kata token list` and
`kata token revoke <token_id>`.

## Commit retention (garbage collection)

A review diffs against the commits its patchsets pinned, not the
live bookmark — so those commits have to outlive the branch that
introduced them. Kata protects them by writing a git ref under
`refs/kata/<review-id>/` in the backing repo for each patchset
endpoint. Because these are ordinary refs, both `jj util gc` and a
plain `git gc` treat the pinned commits (and everything between a
patchset's base and tip) as reachable and never collect them.
Reviews are re-pinned at every startup, so upgrading an existing
deployment retroactively protects reviews created before this
behaviour existed; the protection drops automatically when a review
is deleted.

Operational implications:

- **Don't prune `refs/kata/*`.** If you run repo maintenance that
  deletes refs (mirroring scripts, aggressive `git gc --prune` with
  custom ref filtering, etc.), exclude the `refs/kata/` namespace.
  Removing those refs re-exposes reviewed commits to collection.
- **Kata needs write access to the repo's git store** to create and
  delete these refs. The bind-mounted repo must be writable by the
  user the container runs as (see the host-user note in the
  compose template).
- If a commit is collected anyway (a pre-upgrade review, or pruned
  refs), the review degrades gracefully rather than failing — it
  loads with comments intact and a "diff unavailable" banner
  instead of erroring out. See `docs/SPEC.md` §10.4.

## GitHub PR integration

Kata can drive the full review cycle on a github.com pull request:
paste the PR URL, kata imports the discussion, you draft in kata,
publish back as a GitHub review. Full spec in `docs/SPEC.md` §21;
deployment-specific notes follow.

### How auth works (and why this is single-user)

Kata delegates *every* GitHub API call to the
[`gh`](https://cli.github.com/) CLI on the host. There is no
in-process OAuth client, no callback URL, and kata stores zero
GitHub credentials. The operator runs `gh auth login` once on the
machine kata runs on; kata reuses that authenticated session for
every read and write it makes against github.com.

The trade kata makes:

- **Pre-approved OAuth app.** `gh` ships under GitHub's own
  pre-approved OAuth App, so it sidesteps the per-org policy
  gauntlet a custom OAuth app would otherwise face (SAML SSO
  authorize step, OAuth-App access restrictions). Practically:
  if `gh api repos/<your-org>/<your-repo>/pulls/<n>` works at the
  command line, kata's import will work too. If it doesn't, fix
  it at the `gh` level (`gh auth login --hostname github.com`,
  `gh auth refresh -s repo`, click the SAML authorize link `gh`
  prints, ask an org owner to approve the app, etc.) — kata
  inherits whatever you've authorized for `gh`.

- **Per-machine, not per-user.** Every action kata posts to
  github.com attributes to whoever ran `gh auth login` on the
  kata host. There is no per-user identity layer inside kata
  for the GitHub side — the kata HTTP `--auth-mode` controls
  who can drive *kata*, but every github.com action is signed
  by the single configured `gh` identity. This is fine for a
  single-user, locally-hosted kata. **Do not deploy this mode
  for a shared kata server**: multiple kata users would
  collapse onto one github identity, attributing everyone's
  reviews to the gh-auth account.

Shared-team deployments would need per-user GitHub OAuth, which
isn't currently wired up (an earlier draft of this integration
went there and was deliberately scoped out — see the §21.6
out-of-scope list in `docs/SPEC.md`).

### Operator checklist

1. Install `gh`:
   `brew install gh` / `apt install gh` / etc., or see
   [cli.github.com](https://cli.github.com/).
2. Authenticate: `gh auth login`. Choose `github.com`, HTTPS,
   "login with a web browser" is the standard path. After it
   completes, `gh auth status` should report you're logged in,
   and `gh api user` should return your profile JSON.
3. Ensure the workspaces kata reviews live in have a git remote
   pointing at the github.com repo whose PRs you intend to
   import. `git remote -v` from the workspace should list
   `https://github.com/<owner>/<repo>(.git)?` or
   `git@github.com:<owner>/<repo>(.git)?`. Either form works.
4. Restart kata (no new flags — kata probes `gh` lazily). The
   home screen's "Import from GitHub PR" card appears when
   `gh api user` succeeds; if it doesn't, the card shows a
   precise reason ("install gh" vs "run `gh auth login`").

### In Docker

The compose image — the default `docker compose up --build` target
(`runtime-with-gh`) — ships `gh` and `git`. Kata stores no
credentials of its own, so reuse the host's `gh auth login` by
mounting its config dir read-only and pointing `gh` at it:

```yaml
    environment:
      GH_CONFIG_DIR: /gh
    volumes:
      - "${HOME}/.config/gh:/gh:ro"
```

`gh auth login` on the host once is the whole setup — every API
call the container makes inherits that session (read-only, so the
container can't disturb it). Fetching a PR head from a **private**
repo over HTTPS reuses the same login: the image sets a system git
credential helper (`!gh auth git-credential`), so `git fetch` pulls
the token from the mounted `gh` config. **SSH** remotes instead need
a key mounted (e.g. `- "${HOME}/.ssh:/home/kata/.ssh:ro"`), since
`gh` credentials don't cover SSH. The published release image is the
lean `runtime` target and ships neither `gh` nor `git` — build the
default target (or `--target runtime-with-gh`) for the PR
integration.

### When import fails

Common failure modes and what to check, in roughly the order
you'll hit them:

- **"no registered workspace has a git remote pointing at
  github.com/&lt;o&gt;/&lt;r&gt;"** — kata won't auto-clone unknown
  repos. Register the workspace with
  `--workspace <name>=<path>` at startup; the workspace must be
  a jj/git workspace whose `git remote -v` lists that repo.
- **"the `gh` CLI is not authenticated"** — run
  `gh auth login`.
- **"GitHub returned 403 ... organization has enabled OAuth App
  access restrictions"** — your org restricts third-party OAuth
  Apps and `gh` hasn't been approved. Almost always already-
  approved in practice; if not, an org owner needs to approve
  the GitHub CLI app at
  `https://github.com/orgs/<your-org>/policies/applications`.
- **"SAML SSO authorization required"** — your org enforces
  SAML SSO and your `gh` token hasn't been SSO-authorized for
  it. Run `gh auth refresh -s <scope>` or click the per-org
  authorize URL `gh` prints in the error. After re-running,
  retry the kata import.
- **"the PR head moved since import"** on publish — the PR was
  force-pushed or rebased after you imported. Re-import the
  PR (paste the same URL again) to anchor inline comments
  against the new head, then publish.

### Workspace housekeeping

Each imported PR creates two remote-tracking bookmarks per PR
on the matching workspace — `kata-pr/<n>/head@<remote>` and
`kata-pr/<n>/base@<remote>`. Deleting the kata review drops
both (scoped to the recorded remote so same-numbered PRs from
different remotes don't collide). Abandoning a review without
deleting leaves them in place. To clean up manually:

```sh
jj bookmark forget --include-remotes 'glob:kata-pr/*'
```

Or via git directly:

```sh
git -C <workspace> for-each-ref --format='%(refname)' '**/kata-pr/*' \
  | xargs -r -n1 git -C <workspace> update-ref -d
jj git import   # so jj's view drops the removed bookmarks
```
