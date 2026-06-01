---
name: kata-review
description: Use when working with code reviews through the Kata MCP server — as a reviewer (read diff, leave draft comments, publish) or as the review's author (attach context annotations for reviewers).
---

# Kata code review

Kata is a code-review tool over jujutsu (jj). Reviewers accumulate draft
comments in a private session and publish the whole batch at once. One
MCP server can front multiple repositories; every tool call takes a
`repo` argument naming which workspace to act on.

## Workflow

1. **Discover.** `list_repos` returns the workspaces this server hosts;
   each has a `name` (the slug to pass as `repo`). `list_reviews` then
   returns the open reviews in that repo — each has a `review_id` (the
   jj bookmark name) and counts of published comments.

2. **Open.** `get_review` with the `review_id` returns:
   - `manifest` — metadata and patchset history.
   - `diff.files[]` — per-file **metadata only**: `path`, `status`
     (added / modified / deleted / renamed), `added` / `removed`
     line counts, and `binary`. Hunks are **not** inlined, to keep
     the response small for big reviews.
   - `comments` / `responses` — already-published feedback.
   - `drafts` — your unpublished work in the open session.

   Call `read_file_diff` (with the same `review_id` plus the `path`
   from `diff.files[]`) to fetch the hunks for one file. Each hunk
   line carries a `side` (`base` or `tip`) and a 1-based line
   number on that side — the same coordinates you anchor comments
   with. Pull only the files you actually need to read; for a big
   refactor the full diff is often much larger than your context
   budget.

   For a small review where pulling every file is cheaper than the
   per-file round-trips, pass `include_hunks: true` to `get_review`
   to inline every file's hunks in one call. Same hunk shape as
   `read_file_diff` returns. Default `false` — opting in for a
   200-file refactor will blow your context budget.

3. **Comment.** Anchor every comment with the tip patchset's IDs:
   `manifest.patchsets[last].tip_change` and `tip_commit`. Pick the
   granularity that fits:
   - `draft_line_comment` — a specific line range on one side.
     Pass `columns` to scope to a sub-region: for a single line
     (`lines.start == lines.end`), `columns` is `{start, end}` UTF-
     16 within the line — useful for dense lines (long parameter
     lists, chained calls) where line-granularity hides which part
     you mean. For a multi-line range, `columns.start` is the
     offset on the FIRST line and `columns.end` is the offset on
     the LAST line — the typical shape when reviewers free-form
     drag-select prose that begins mid-line on one row and ends
     mid-line on another.
   - `draft_file_comment` — feedback about a whole file.
   - `draft_review_comment` — overall notes, not tied to any file.

   `flag` classifies severity: `must-do` (blocks merge), `suggestion`
   (worth considering), or `question` (you want the author to answer
   something). The first draft call auto-opens a session; subsequent
   drafts reuse it until you publish or discard. Revise a still-
   unpublished draft with `update_draft_comment` (pass the `comment_id`
   plus the new `body` and `flag`); the anchor stays put. Drop a draft
   you no longer want with `delete_draft_comment` — useful for clearing
   a typo'd or misfired comment before publish.

4. **Respond.** `respond` replies to an existing comment. The `action`
   field also changes resolution state: `comment` (no change),
   `resolve`, `unresolve`, `wont-fix`, `un-wont-fix`. **Never `resolve`
   a `question` you're answering** — whether your answer satisfies the
   author is the author's call, not yours. Use `action: comment` and
   let them resolve.

   Revise a still-unpublished reply with `update_response`, or drop
   one with `delete_draft_response` — handy for undoing a typo'd
   reply or a misclicked resolve / wont-fix / unresolve action
   before publishing the session.

5. **Publish.** `publish_session` with the `session_id` from
   `drafts.session` makes the whole batch visible to the author. Use
   `discard_session` to throw the batch away instead.

## Author annotations (creator-only)

Annotations are short context notes the **review's author** attaches
to specific code regions for the benefit of reviewers — "this looks
weird because we already tried Y", "yes the duplication is on
purpose, see ticket X". They're separate from review comments:

- One-way: reviewers read them but can't reply. (If a reviewer wants
  to discuss what an annotation says, they file a regular comment.)
- No severity / flag — annotations are context, not requests.
- No draft session — `add_*_annotation` publishes immediately.
- Creator-only: only the identity matching `manifest.created_by` may
  write. Non-creators get `BadRequest`.

Tools:

- `add_line_annotation` — anchor a note to a specific line range on
  one side, same anchor shape as `draft_line_comment`.
- `add_file_annotation` — note that applies to a whole file.
- `update_annotation` — change the body of an existing annotation
  (anchor stays put; pass `annotation_id` + new `body`).
- `delete_annotation` — remove an annotation.

Annotations are surfaced inline in `get_review` under
`annotations: []` (omitted when the review has none). Each entry
flattens an `Annotation` with an `anchor` field that mirrors
comments' Valid/Moved/Drifted/Outdated revival so an annotation
stays attached to its code across patchsets.

When to use which:

- You're the **reviewer**, and you have a critique or a question →
  `draft_*_comment`.
- You're the **author**, and you want to pre-explain something that
  would otherwise look wrong → `add_*_annotation`.

## Recording a new round

When the author pushes new commits or rewrites the branch under
review, call `refresh_review`. It re-resolves the manifest's revset
and, if the tip or base has moved, appends a new patchset and makes it
current. Comments stay anchored to their original patchset (they
re-anchor against the current view), so refreshing is safe and
non-destructive.

If you're the review's creator, pass `summary` to `refresh_review` (or
call `update_review_summary` separately) to set or replace the
free-text description shown at the top of the review. Non-creators
that try to update the summary are rejected.

If the original revset stopped meaning what you intended — `jj
abandon` on a divergent change, or wanting to track a different
branch tip — call `update_review_revset`. The new revset is
re-resolved and, if the endpoints moved, a new patchset is
recorded the same way `refresh_review` would (creator-only,
idempotent: same endpoints = no new patchset).

## Lifecycle

When a review is done with — landed, abandoned, or just no longer
relevant — clean it up:

- `archive_review` — hides the review from the home screen by
  default and rejects new draft sessions. Reversible with
  `unarchive_review`. Prefer this for reviews that "shouldn't show
  up day-to-day" but might still be referenced.
- `delete_review` — permanently removes the review and every
  dependent record (sessions, comments, responses, annotations,
  visit timestamps). Reach for this only when the review truly
  shouldn't exist; archive is the safer default.

Both are creator-only. Both are idempotent (calling twice doesn't
error).

## Tips

- Read the entire diff before writing any comments. Comments stamped on
  one patchset stay anchored to those line numbers even after the
  author rewrites the change.
- Mind `side` on line comments: `tip` lands on new code, `base` on what
  was removed.
- Reserve `must-do` for things you'd actually block on — overuse
  dilutes the signal.
- Use `question` when you want the author to answer something
  specific. The author decides whether your answer (or theirs)
  resolved it — never `action: resolve` on a question you didn't
  raise.
