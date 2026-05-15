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

## Measure the line-number gutter instead of hardcoding its width

The inline thread, the line-composer overlay, and the orphan-thread
block all indent past the gutter using `lnCols * 65` (where 65 =
48 declared `width` + 8 + 8 padding + 1 right border on one `.ln`
cell). That lines up with the `.row.commented .content` stripe
today because the line-number digits fit inside the cell's 32 px
content area at the 12.5 px monospace font. Once that breaks —
5-digit line numbers, font size change, padding tweak — the table
auto-grows the gutter column and the indents stop aligning again.

Robust fix: ResizeObserver the first `.content` cell of the hunk
table and publish its `offsetLeft` as a CSS variable on the
wrapper. Everything that currently uses `lnCols * 65` reads the
variable. One observer per visible file; no math, no coupling to
the cell's box-model values.

## Word-diff pairing across uneven remove/add blocks

`computeHunkWordDiff` (`web/src/lib/wordDiff.ts`) uses N:N pairing
— a run of N consecutive remove lines followed by exactly N add
lines pairs row-by-row, everything else is skipped. Covers the
common case ("edit a few lines"), bails on the rest: `3 removed,
4 added` (one genuinely inserted line in the middle of a refactor)
gets no word-diff at all, even though three of the pairings are
obvious.

An LCS-best-match approach would score every `(remove[i], add[j])`
candidate by token overlap and pick pairings that maximize total
similarity, leaving the leftovers as pure delete/insert. More code
and a bigger debugging surface, but covers real cases (renames
mixed with inserts, lines reordered inside a block). Worth a look
at what VSCode / GitHub actually do before reinventing.

## Compare beyond hunks across patchsets

The "compared to" selector re-resolves the file diff between two
patchsets, but leaves everything else (commits, descriptions, review
summary) rendering as if the viewer were on the target patchset
alone. A richer compare view would surface:

- commits in patchset B that aren't in A, and vice versa;
- per-commit description deltas across the matched commits;
- review-summary deltas across the two rounds.

Probably a dedicated panel on the compare landing rather than
annotations sprinkled on existing widgets.

## Tauri desktop shell

`kata serve` + a browser tab is fine for the dev workflow but heavy
for "yet another tool the team installed on their laptop". A Tauri
wrapper would bundle the binary, a webview, and per-platform
installers into one thing — native window, dock icon, system-tray
refresh affordance, optional `kata://` URL handler. The axum routes
don't change; it's mostly packaging.

## Other ideas

_(add new entries above this line as they come up)_
