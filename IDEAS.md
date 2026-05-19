# Ideas

Things worth considering but not yet picked up. Lower the bar for adding —
write the context now so the rationale doesn't get lost; we can prune later.

## Auto-refresh a review when the underlying branch moves

`service::refresh_review` re-resolves a review's revset and appends a
new patchset (fast-forward vs rewritten is recorded via the
`parent_patchset` field on `Patchset`). It's invoked through explicit
user action (the refresh button) or the MCP `refresh_review` tool, and
the background watcher pings the UI via `Event::ReviewBranchMoved`
when the branch has moved — but only the human/agent decides whether
to actually advance the manifest. A more "magic" alternative:

- **Refresh implicitly inside `open_review`.** Every pageload would pick
  up new commits. Cost: one extra `jj log` per view. Bigger concern: a
  passive viewer's mere act of opening a review would create new
  patchsets, which can race with the author still rebasing or amending.
  Probably acceptable if we debounce ("only if no refresh in the last
  N seconds") and skip when the review is anchored to an explicit
  patchset via the URL.

## Smarter scoping for the branch watcher

The current watcher (`spawn_branch_watcher`) re-resolves every review's
revset every tick. That's one `jj log` per review per tick, which is
fine for a handful of reviews and silly for a hundred. Options:

- **Scope to reviews with active SSE subscribers.** The event bus
  already knows who's listening; the watcher could iterate only the
  subset that anyone actually cares about right now.
- **Dedup by revset.** Multiple reviews on `trunk()..feature-X` for
  different bookmarks all resolve the same expression — one call could
  serve all of them.
- **Watch `.jj/repo/op_heads`** instead of polling. React to jj
  operations directly; on each op-id change, do one full pass.

Also: the watcher's in-memory state map (`(RepoId, ReviewId) →
(tip, base)`) never prunes entries for reviews that were deleted from
storage. Minor memory leak, easy fix when we add a real review-delete
path.

## Dedup in-flight file-diff fetches

`FileSlot` caches resolved per-file diffs in `fileDiffCache`, lifted
to `ReviewViewer` so cached entries survive the slot virtualizing
itself out of the DOM. The cache stores the *resolved* `FileChange`,
not the in-flight promise. If a slot kicks off a fetch, the user
scrolls past quickly (the slot unmounts before the response arrives),
then scrolls back (the slot remounts), the second mount sees an
empty cache plus a fresh local `loadingHunks` (component state, lost
on unmount) and starts a second fetch for the same
`(patchset, compare, path)`.

`api.readFile` already handles this by caching `Promise<string>`
directly so concurrent reads share the round-trip. Same pattern
would work for the file-diff cache: store `Promise<FileChange>`,
delete the entry on rejection so retries still have a path. Worth
picking up once we have a workflow that mounts/unmounts the same
slot quickly (rapid scrolling on a slow connection, automated
navigation tests, etc.).

## Review-summary deltas in patchset-compare

Compare v2 surfaces per-change-id status (`same` / `changed` /
`added` / `removed`), per-commit description deltas, and per-pair
interdiffs. The one thing it still doesn't surface is the
review-summary delta across the two rounds — the manifest's
`summary` field can change when an author updates the description
to track scope drift, but a reader comparing PS_a → PS_b never
sees that. Probably a small section above the pair list on the
compare landing.

## Tauri desktop shell

`kata serve` + a browser tab is fine for the dev workflow but heavy
for "yet another tool the team installed on their laptop". A Tauri
wrapper would bundle the binary, a webview, and per-platform
installers into one thing — native window, dock icon, system-tray
refresh affordance, optional `kata://` URL handler. The axum routes
don't change; it's mostly packaging.

## Surface broken revsets on the review-list row

The review-detail page renders a banner when the manifest's revset
can't be resolved (`RevsetError` on `ReviewView`), but the same
problem is invisible from `ReviewList` — the reader has to open
each review to discover it's broken. Add a pill on the list row
that mirrors the banner's headline, gated on the same
`revset_error` shape so users can spot stuck reviews from the
home screen.

## Conflicts as first-class diff content

jj keeps conflicted commits as live objects with structured conflict
regions, not as the broken working-copy state git leaves you in. A
review could lean on that directly:

- Badge commits in the commits panel that landed conflicted ("⚠
  conflict in `foo.rs`"), so the reviewer doesn't have to check
  out anything to spot them.
- Render the conflict regions inline as a special hunk kind,
  showing the three sides (base, left, right) the way `jj resolve`
  would.

The data is already in the underlying commit — the diff machinery
needs to recognize "this side of the diff comes from a conflicted
region" and emit a different `HunkLine` origin (`Conflict { base,
left, right }`) that the renderer knows about.

## Reviewer suggestions via `jj absorb`

PR tools force a "thanks, fixing in PS3" round-trip on every
reviewer suggestion. jj's `absorb` knows how to push a working-copy
diff back into the right commit in the stack. A "suggested change"
in Kata could:

- Generate the diff from the reviewer's edit (against the same
  patchset they're looking at).
- Send it to the author's workspace as a patch they can run
  `jj absorb` against (low-trust path), or
- Apply it directly via `absorb` if the author has opted in
  (high-trust path, presumably self-review or trusted-team
  scenarios).

Permission model is the open question — most reviewers don't have
write access to the author's working copy. Probably ships as the
patch-handoff variant first, with absorb-directly as an opt-in.

## Richer divergence panel

The divergence banner already lists `divergent_commit_ids` (12-char
prefixes), but that's still just a bag of IDs — the reader can't
tell which version is which without dropping to a shell. Two adds
would close the loop:

- For each sibling, fetch commit metadata (author, timestamp,
  description first line) via one extra `list_commits` call against
  `change_id(X)` and render the row inline.
- A copy-button per row that yields `jj abandon <commit_id>` so
  the reader doesn't have to retype anything.

The panel only renders when `revset_error` is set, so the cost is
gated.

## Edit a review's revset after creation

Today only `refresh_review` and `update_review_summary` mutate a
review. There's no way to rewrite `manifest.revset` itself, which
is exactly what a reader wants to do when divergence is genuine
("both versions are real, the review should track only the new
one") or when the original revset stopped meaning what they
intended. The shape is straightforward:

- `POST /api/repos/<slug>/reviews/<n>/revset` taking `{ revset:
  string }`.
- Service-side: validate the new revset resolves to a single tip
  + base, append a new patchset, record the previous one's tip
  as `parent_patchset` if it descends.

Wait for the demand signal before building — most reviews probably
don't hit this, and the divergence banner + `jj abandon` workflow
covers the common case.

## Two-phase comment resolution: claim vs. acknowledgement

The current model treats "resolved" as a single-actor decision: a
responder marks the thread done and the UI immediately folds it.
The unread-replies marker (committed) softens this — threads with
responses newer than the viewer's last visit stay expanded even
when resolved — but it doesn't model the actual handshake: the
responder *claims* the work is done, and the comment author then
either *accepts* or *reopens*. Once the viewer reloads, the
unread marker clears whether they actually read the response or
just scrolled past it.

Real fix: split the resolution state into two fields.

- `resolved` stays where it is — a response action set by anyone
  who thinks the issue is addressed.
- `acknowledged_at` (or similar) is set by the *comment author*
  when they sign off on the resolution. Until then, the thread
  stays expanded with a "needs your review" badge regardless of
  resolution state. The author can either acknowledge (folds) or
  reopen (resolution clears, thread stays expanded).

Once this lands, the existing "next unread" comment-nav can
upgrade from a derived timestamp predicate to a real persistent
queue: anything `resolved && !acknowledged` is in your inbox
until you act on it.

One new response action (`acknowledge`), one schema field, and a
small storage migration. The UI surface mostly already exists —
the unread-marker rendering paths just key off
`!acknowledged && state !== 'open'` instead of the timestamp
comparison.

## A "review responses" view-mode toggle

A top-bar chip that puts the viewer into a focused "go through
what changed since I was last here" mode: expand every comment
with responses newer than `last_visit_at`, hide the rest, and
gate the comment-nav `< >` buttons to walk only that subset.
Click again to return to the normal view.

Doesn't change the data model — it's a derived filter layered on
top of the unread-replies signal that already exists. Suits the
specific workflow of "I asked an agent to address a batch, now
I'm reviewing what it did". The current always-on visible badge
+ auto-expand handles the steady-state case; this mode is for
when the viewer is deliberately sweeping a backlog.

Worth picking up if the steady-state markers turn out to be
noisy on long-running reviews, or if the two-phase-acknowledgement
work above lands and we want a quick "show me what I haven't
acknowledged yet" affordance.

## Base-aware reprojection for patchset-compare

The v2 compare view detects `compare_base_mismatch` (the two patchsets
descend from different base commits) and surfaces a banner. The
cumulative diff and each per-commit interdiff still reflect upstream
movement on top of author edits, which is misleading when the reader
asks "what did the agent change."

A real fix reprojects the *from* side onto the *to* base before
diffing: rebase `from.tip` onto `to.base`, then diff the result
against `to.tip`. Same operation `compute_rebased_diff` in
`kata-jj::libjj` already does at the per-commit level — extending it
to "rebase a whole patchset's tip" is a few lines on top of the
existing `merge_trees` helper. The per-pair diff_counts and the
cumulative diff would then both be reprojection-clean.

Today every review in the corpus has stable bases, so the banner is
enough. Lands when someone actually hits the case.

## Authenticated identity

Today the HTTP layer trusts an `X-Review-Author` header (falling back
to the server's `--author` flag). That's fine for a single-tenant
local box; it's a soft hole on anything shared, because any caller
can claim to be anyone. The MCP transport has the same shape via
`?author=` on the streamable-HTTP endpoint.

Options:

- **OIDC behind a fronting proxy.** Run `oauth2-proxy` / Authelia /
  Pomerium in front, have it set a trusted header (e.g.
  `X-Forwarded-Email`) once the user is authenticated, and have
  Kata derive the actor from *that* header rather than the
  client-supplied one. The proxy carries the OIDC dance, session
  cookies, refresh, group claims, etc. Kata stays small.
  - Variation: accept `X-Forwarded-Email` only when the request
    comes from a configured set of upstream IPs / a unix socket,
    so a misconfigured deployment can't have the header spoofed.
- **Built-in OIDC client.** Adds an `--oidc-issuer` flag and an
  `/auth/callback` route; Kata mints session cookies itself. Heavier
  surface area but keeps the deployment to a single binary, which
  matters for the "drop on a laptop / VM" workflow the demo command
  targets.
- **Per-agent API tokens.** Agents (MCP clients) don't have humans
  at a keyboard; a long-lived token bound to a specific author
  identity is a better fit than OIDC. The author claim and the
  token issuer would be tied together (the token *is* the actor),
  so an agent can't claim to be someone else by editing a header.

Storage / data model implications are small — `Author` is already
the canonical identity throughout `kata-core`. The work is
plumbing on the HTTP and MCP transports plus a configuration
story for *which* mode (proxy vs. built-in vs. tokens) is active
on a given deployment.

Lands the first time someone wants to host Kata for a team rather
than a single user.

## TLS / HTTPS

`kata serve` only speaks plain HTTP. Same caveat as above: fine
for `localhost`, wrong for anything else. Browsers also nag
about features (clipboard, service workers, secure cookies) that
degrade or refuse to run on plain HTTP.

Two reasonable paths:

- **Terminate at a reverse proxy.** Nginx / Caddy / Traefik in
  front; Kata stays HTTP-only on a loopback socket. The proxy
  handles certs (often via ACME) and forwarding. This is the
  recommended production pattern *today*, but it's nowhere in the
  docs — the README and demo flow both leave the reader at plain
  HTTP. The cheap fix is a short "Deploying behind a proxy"
  section with a working Caddy / Nginx snippet, plus a note that
  the `X-Forwarded-*` headers Kata trusts (see the auth entry
  above) only apply behind such a proxy.
- **Native TLS in `kata serve`.** `--tls-cert <path>` /
  `--tls-key <path>` flags, rustls under the hood. Optional
  ACME / Let's Encrypt mode (`--tls-acme <domain>`) for the
  drop-on-a-VM workflow where adding a reverse proxy is friction.
  Keeps the single-binary story coherent and matches the demo
  command's "no setup" pitch.

Either way: once HTTPS is the path of least resistance, the
existing `bind: 127.0.0.1:7878` default needs a conditional
companion (`bind: 0.0.0.0:443` when TLS is configured) so users
don't accidentally serve a wide-open HTTPS listener on the loopback.

Pairs naturally with the auth work — both are "this is no longer
just a localhost dev tool" plumbing — and probably wants to land
in the same change so the deployment guide can cover both.

## Other ideas

_(add new entries above this line as they come up)_
