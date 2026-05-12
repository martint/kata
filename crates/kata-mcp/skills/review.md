---
name: kata-review
description: Use when reviewing code through the Kata MCP server — list reviews, open one, read the diff, leave draft comments anchored to lines/files/the whole review, then publish the batch.
---

# Kata code review

Kata is a code-review tool over jujutsu (jj). Reviewers accumulate draft
comments in a private session and publish the whole batch at once. The
MCP server is bound to one repository at startup — no `repo` argument is
needed on any tool call.

## Workflow

1. **Discover.** `list_reviews` returns open reviews; each has a
   `review_id` (the jj bookmark name) and counts of published comments.

2. **Open.** `get_review` with the `review_id` returns:
   - `manifest` — metadata and patchset history.
   - `diff.files[]` — per-file hunks. Each hunk line has a `side`
     (`base` or `tip`) and a 1-based line number.
   - `comments` / `responses` — already-published feedback.
   - `drafts` — your unpublished work in the open session.

3. **Comment.** Anchor every comment with the tip patchset's IDs:
   `manifest.patchsets[last].tip_change` and `tip_commit`. Pick the
   granularity that fits:
   - `draft_line_comment` — a specific line range on one side.
   - `draft_file_comment` — feedback about a whole file.
   - `draft_review_comment` — overall notes, not tied to any file.

   `flag` classifies severity: `must-do` (blocks merge), `suggestion`
   (worth considering), or `other` (notes / questions). The first draft
   call auto-opens a session; subsequent drafts reuse it until you
   publish or discard.

4. **Respond.** `respond` replies to an existing comment. The `action`
   field also changes resolution state: `comment` (no change),
   `resolve`, `unresolve`, `wont-fix`, `un-wont-fix`.

5. **Publish.** `publish_session` with the `session_id` from
   `drafts.session` makes the whole batch visible to the author. Use
   `discard_session` to throw the batch away instead.

## Tips

- Read the entire diff before writing any comments. Comments stamped on
  one patchset stay anchored to those line numbers even after the
  author rewrites the change.
- Mind `side` on line comments: `tip` lands on new code, `base` on what
  was removed.
- Reserve `must-do` for things you'd actually block on — overuse
  dilutes the signal.
