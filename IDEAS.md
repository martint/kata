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

**Status:** all three modes plus per-agent tokens have shipped.
Nothing pending in this section.

`kata serve` supports three identity sources:

- **`--auth-mode trust-client`** (default) — `X-Review-Author` on
  HTTP, `?as=` on MCP. Safe on localhost, unsafe shared.
- **`--auth-mode trust-forwarded-header`** — reads an upstream-set
  header (default `X-Forwarded-Email`), gated by `--auth-trust-
  upstream <cidr>` so the header is only honoured from configured
  ingress points. The proxy (oauth2-proxy / Authelia / Pomerium /
  Caddy) carries the OIDC dance.
- **`--auth-mode oidc`** — Kata speaks the OIDC authorization-
  code flow itself. `/auth/login` 302s to the issuer;
  `/auth/callback` validates the ID token and mints a
  HMAC-signed session cookie carrying the email claim. Suits the
  drop-on-a-VM single-binary workflow where adding `oauth2-proxy`
  upstream is friction.

Plus **API tokens** — `kata token create/list/revoke` mints long-
lived bearer credentials bound to an author. Presented as
`Authorization: Bearer <token>` (HTTP) or `?token=` (MCP). Token
auth wins over the mode-specific lookup so MCP agents and CI
integrations don't have to round-trip through an interactive flow.
In OIDC mode the session cookie is browser-only; agents MUST
present a token.

## TLS / HTTPS

**Status:** all three modes have shipped. Nothing pending in this
section.

Working paths today:

- **Reverse proxy out front.** Nginx / Caddy / Traefik does TLS +
  OIDC; Kata stays HTTP-only on a loopback socket. Pair with
  `--auth-mode trust-forwarded-header`.
- **Native rustls with operator-supplied cert.** `--tls-cert <path>`
  / `--tls-key <path>` wrap the listener in rustls via
  `axum-server`. Refresh is the operator's job (cert-bot, external
  ACME script, etc.) plus a server restart.
- **Native ACME / Let's Encrypt auto-issuance.** `--tls-acme
  <domain>` + `--tls-acme-cache <dir>` (`+ --tls-acme-staging` for
  development, `+ --tls-acme-contact mailto:...` for renewal
  warnings). Uses TLS-ALPN-01 challenge so no extra port is
  needed; cert lives on the same listener as the app. The cert
  hot-swaps in place at renewal time — no restart at the 60-day
  mark.

## Repository browser

**Status:** shipped. Read-only, four pieces — log graph, commit
detail, file viewer, file history — plus a "Create review from
this commit" handoff. Mutations (rebase / squash / abandon /
push / fetch) deliberately remain out of scope: the review
tool's job stays "look at code", not "rewrite history".

Lives at `/r/<repo>/browse` with a "Browse" link in the app
header. Default revset is `bookmarks() | @ | latest(@-.. |
..@, 50)`; the search box at the top of the log pane takes any
free-form expression.

Wire shape:

- **Log graph.** Column-stem layout (port of jjuicy's
  Sapling-style algorithm, `kata-jj::log_graph`) ships per-row
  `(col, row)` coordinates and tagged edge shapes to the client.
  Frontend renders SVG paths — no DAG knowledge required.
- **Commit detail.** Description, refs, changed-files list
  (clickable), conflict list, "Create review from this commit"
  button.
- **File viewer.** Read-only file content at a `(commit, path)`,
  fed through the diff viewer's existing Shiki pipeline. Binary
  files render a placeholder.
- **Per-file history.** "History" button on the file viewer
  switches the log pane to `files("<path>")` so the user can
  see which commits touched it.
- **Create-review handoff.** Navigates to the new-review form
  with `?prefill_revset=<commit>-..<commit>` — reviewer
  confirms or edits before submitting.

Possible follow-ups (no current demand signal):

- **Pagination beyond the row cap.** The layout algorithm
  already supports it via `has_more`; the client doesn't issue
  follow-up pages yet. Worth adding when reviewers hit the cap.
- **Per-line "blame" view.** Walking history per line is a
  different shape from per-file history; expensive, and most of
  the use case is covered by per-file history + the diff
  viewer. Land if asked.

## In-app search across the diff

Browser find (Ctrl/Cmd+F) doesn't work usefully against a Kata
review. `FileSlot` virtualises files outside the viewport — their
hunks aren't in the DOM at all — and file-fold collapses hide
content even for the file the user is currently looking at. So
the native search reaches a fraction of what's actually in the
review, with no signal about what it's missing.

A proper fix is an in-app search box that knows the structure:

- **Scope**: live against the *whole* review, not just what's
  rendered. The review's full diff is already in
  `current.diff.files[*].hunks[*].lines` on the client (modulo
  lazy-loaded files — those can be force-loaded on first search,
  cached afterward).
- **Surface**: a `/` keyboard shortcut + a search field in the
  top header (next to the comment-nav cluster). Pressing Esc
  closes; `n` / `Shift-n` walk results.
- **Matches**: per-file count + a jump list. Clicking a result
  scrolls the file into view, expanding it if folded and
  scrolling to the matching line. The matched substring is
  highlighted in place.
- **Scope filters**: at minimum "search base+tip" / "tip only"
  / "comments and annotations". A second pass could add file-
  path filtering — useful on big reviews.

Two implementation calls to make before building:

- **Virtualisation strategy.** Today `FileSlot` mounts/unmounts
  via IntersectionObserver, so off-screen files are *not in the
  DOM at all*. Switching to `content-visibility: auto` would
  keep content in the DOM (and in the find/accessibility tree)
  while deferring paint — but it changes the height-estimation
  story and the SSR-ish placeholder model `FileSlot` currently
  relies on. Worth seeing if an in-app search can sidestep this
  by reading the in-memory `FileChange.hunks` directly without
  needing the DOM to be live.
- **Comment / annotation bodies as search targets.** Reviewers
  searching for a word in a thread expect Cmd+F to find it.
  Whether comment bodies live in the same index as diff text or
  in a separate "discussions" tab is a UX call — probably one
  index with a chip filter, but worth confirming.

Lands when reviewers report missing search results, or once
reviews routinely exceed a screen of diff (the demo's three-file
review masks the problem because everything fits in the
viewport).

## Other ideas

_(add new entries above this line as they come up)_
