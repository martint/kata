# Ideas

Things worth considering but not yet picked up. Lower the bar for adding —
write the context now so the rationale doesn't get lost; we can prune later.

## Auto-refresh a review when the underlying branch moves

`service::refresh_review` already knows how to re-resolve a review's revset
and append a new patchset (fast-forward vs rewritten is recorded via the
`parent_patchset` field on `Patchset`). Today it's only invoked through
explicit user action (the refresh button) or an MCP `refresh_review` tool
call. Two pieces of automation are reasonable but each has trade-offs:

- **Refresh implicitly inside `open_review`.** Every pageload would pick
  up new commits. Cost: one extra `jj log` per view. Bigger concern: a
  passive viewer's mere act of opening a review would create new
  patchsets, which can race with the author still rebasing or amending.
  Probably acceptable if we debounce ("only if no refresh in the last
  N seconds") and skip when the review is anchored to an explicit
  patchset via the URL.

- **Background watcher per repo.** Poll the jj op log on a timer (or
  watch `.jj/repo/op_heads` for changes) and run `refresh_review` for
  every review whose revset endpoints have moved. Most "magic" UX but
  the most moving parts: keeps a tokio task alive per repo, has to
  serialize against in-flight refreshes started by the user/agent, and
  needs to know which reviews are "alive" (or just iterate the manifest
  list each tick).

Either way: the SSE event flow (`Event::ReviewUpdated`) already pushes
the updated manifest to open viewers, so the UI cost of these is zero.

## Other ideas

_(add new entries above this line as they come up)_
