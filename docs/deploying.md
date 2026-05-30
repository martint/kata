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
