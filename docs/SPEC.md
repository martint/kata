# Kata — Product Specification

This document describes how Kata behaves as a product: the problems
it solves, the concepts it asks users to learn, the workflows it
supports, and the affordances and idioms reviewers will encounter
on screen. It deliberately avoids implementation detail; for that,
see the README's architecture section and the per-crate sources.

---

## 1. What Kata is

Kata is a code-review tool for [Jujutsu (`jj`)](https://jj-vcs.github.io/jj/).

`jj` makes commits malleable on purpose. The same logical change
typically goes through many revisions before it lands — `jj amend`,
`jj squash`, `jj split`, `jj rebase` are everyday operations, not
escape hatches reached for once a quarter. Most code-review tools
were designed for a Git-style world where a commit, once pushed, is
treated as forever; they react badly when history is rewritten
underneath them. Comments either move to the wrong line, disappear
silently, or refuse to follow at all.

Kata's premise: review the *change*, not the commits. Reviewers and
authors should be free to rewrite history during a round of review,
and the conversation should survive that rewriting in a way that
makes drift visible rather than silently destructive.

Three audiences share the same UI:
- Authors, putting work up for review.
- Human reviewers, reading diffs and leaving comments.
- AI agents, reading the same diffs through MCP and leaving the
  same kinds of comments.

---

## 2. The vocabulary

A small number of concepts carry the whole product. Once a user
learns these, everything else is mechanical.

### 2.1 Repository (workspace)

A `jj` working copy that Kata has been pointed at. Each workspace
gets a URL slug (typically the directory name, optionally renamed
at startup). One Kata instance can serve any number of workspaces
side by side; the user picks one from the home page and everything
they do from there is scoped to it.

### 2.2 Review

A *review* is the unit of "I want feedback on this change."

It pins:
- a **revset** — any `jj` revset expression, not just a branch
  (e.g. `trunk()..feature`, `mine() & ~immutable_heads()`, a stack
  of three changes by `change_id`),
- a **bookmark** — optional, used so the system can notice when the
  underlying branch advances,
- a **summary** — Markdown prose that explains *why* the change
  exists; the author edits it as scope drifts during review,
- a **creator** — only this identity may write annotations and
  edit the summary,
- a sequence of **patchsets** (see below).

Reviews are numbered per repository (`#1`, `#2`, …) and are
addressable at `/r/<repo>/<number>`.

### 2.3 Patchset

A patchset is a snapshot of the revset's resolved endpoints —
`(base_change, base_commit, tip_change, tip_commit)` — at one
moment in time. Each one is just `(N, base, tip, recorded_at,
parent_patchset)`.

The first patchset is recorded when the review is created. Every
subsequent patchset is recorded by an explicit *refresh* action
that re-resolves the revset and notices the endpoints have moved.
The system also notices in the background that the underlying
bookmark has advanced and surfaces a hint ("the branch has moved
since the latest patchset was recorded"), but it never records a
new patchset on its own — recording is always under user control.

A patchset's relationship to its predecessor falls into one of
three buckets:
- **Fast-forward** — the new tip descends from the old tip. New
  commits stacked on top, no rewriting.
- **Amended in place** — the tip's `change_id` is the same; the
  same logical change has a new `commit_id` because the author
  amended it.
- **Rewritten** — neither of the above; the branch was abandoned
  and started over. Surfaced explicitly in the UI because comments
  may not transfer cleanly.

The UI labels a patchset accordingly. The first two get a quiet
"PSN" badge; rewritten patchsets get a stronger marker that warns
the reader that anchor drift is more likely.

### 2.4 Comment

A *comment* is feedback from a reviewer, anchored to a specific
place in the change. Comments can target:

- **A line range on a specific file/side** — the most common kind.
  "Line 42 of `src/foo.ts`, tip side."
- **A column range inside a line** — partial-selection comments,
  for when "line 42" is too coarse.
- **A commit** — feedback on a commit as a whole, with no file or
  line. ("This commit description should mention the migration.")
- **The review as a whole** — review-wide comments, with no commit
  binding. ("Overall direction is good — one structural concern
  below.")

Every comment carries a **flag** (severity):
- **must-do** — the reviewee must address this before the change
  is acceptable. This is the default, on the theory that "you're
  blocking my merge" should be the loudest choice the reviewer can
  reach for.
- **suggestion** — optional improvement.
- **question** — asking the author to explain. Whether their
  answer satisfies the question is the author's call to make;
  responders should not auto-resolve.

Comments resolve to a state: **open**, **resolved**, or
**won't-fix**. Resolution can change as the conversation
progresses.

### 2.5 Response

A *response* is a reply to a comment. Responses can carry a
*resolution action* alongside their body:
- **comment** — discussion only, no state change.
- **resolve** — closes the thread.
- **unresolve** — reopens a thread that was resolved or won't-fix.
- **won't-fix** — closes the thread acknowledging the issue but
  declining to address it. Communicates a different stance from
  *resolve*, which implies the issue was fixed.

The full history of responses on a comment is preserved in order
and rendered as a thread, with each response's body, author, and
timestamp.

### 2.6 Anchor (and what happens to it across patchsets)

When a comment is written, it captures the bytes of the lines it
references on the patchset it was written against. On every later
patchset, those bytes are re-located, and the comment is presented
with one of four **anchor states**:

- **valid** — the same content still exists at the same line
  numbers. The thread renders inline at its original location with
  no extra chrome.
- **moved** — the same content exists, but at different line
  numbers. The thread renders at the new location, with a quiet
  "moved to L42–43" badge so the reader knows the position changed.
- **drifted** — the content has changed but is still recognisable.
  The thread renders at the closest match, with a "drifted (87%
  similar)" badge that quantifies how confident the system is.
- **outdated** — the content has changed enough that the system
  can't place the comment confidently. The thread surfaces as
  *outdated* with the original snippet visible, alongside a link
  back to the patchset where it was originally written.

Anchor drift is visible at a glance; it never silently destroys a
comment, and it never silently relocates one to the wrong place.

### 2.7 Annotation

An *annotation* is the author's counterpart to a comment: a note
they leave on their own change. Annotations are author-only —
nobody else can write them — and have no thread, no replies, no
resolution. They are context the reviewer needs *before* reading
the diff:

> "Note: kept `formatName` as its own export so we can reuse it
> for `Person.displayName` in a follow-up PR."

Notes are colour-coded amber (vs comments' neutral palette) so the
reader can tell at a glance "this is from the author."

### 2.8 Session

A reviewer's drafts and responses live inside a *session* until
they're ready to share them.

A session is opened implicitly the first time a reviewer writes
anything against a review. Subsequent drafts accumulate in the
same session. **Publish** flushes the entire batch atomically:
collaborators see a coherent review with all the reviewer's
thoughts at once, rather than a drip-feed where threads appear in
isolation as each one is typed. **Discard** throws the batch away
without publishing.

Sessions are per-author. Two reviewers can be drafting feedback on
the same review at the same time without seeing each other's
drafts.

---

## 3. Application surface

Two screens cover everything: the review list and the review viewer.
A sticky top header is shared between them. Everything else is
chrome inside one of those two screens.

### 3.1 The header

The header has two rows. Row one is always present:
- Kata wordmark + icon, doubling as a home link.
- A workspace selector when multiple workspaces are configured.
- The signed-in author's identity.

Row two appears only on a review and carries review-scoped state:
- Review number + name, plus an **Archived** pill if archived.
- **Patchset** picker (left) and **compared to** picker (right) —
  present only when the review has more than one patchset.
- **Comment navigation**: `← position/total →` for stepping through
  every comment in reading order.
- **Filter chips**: status (draft / open / resolved) and severity
  (must-do / suggestion / question), each a separate chip that
  toggles inclusion. A "Filter hides N comments — show all"
  affordance appears when every chip in a row is off.
- **View toggle**: a three-segment control with **Both**,
  **Diffs**, **Comments** — the user picks how much chrome they
  want around the code.

Everything in row two is sticky; the user can scroll through the
diff and still reach any of these controls without scrolling back.

### 3.2 The review list

Shown at `/`. One row per review on the active workspace, sorted
newest-first. Each row carries:
- review number + name,
- the revset and bookmark,
- the creator,
- a quick comment-status summary (e.g. "3 open, 2 resolved"),
- an *archived* tag if archived.

Filters at the top distinguish *yours*, *yours-and-active*, and
*all*. Typing in the search box narrows by name / bookmark /
revset / creator simultaneously. A **+ Create review** button
takes the user through a single-form flow: pick a bookmark, type
a revset, optionally name the review and seed a summary.

### 3.3 The review viewer

This is where the actual work happens. Three panes:

- **File tree** (left). One node per changed file, with `+N / −M`
  counts. A filter input narrows the tree. The header carries the
  file-by-file navigator (`← position/total →`) that scrolls the
  page to the previous/next file. The tree pane can be collapsed
  to a thin toggle, which is remembered across sessions per device.
- **Commits panel** (centre, top). A list of every commit in the
  revset, newest-first. Each row shows the change-id, commit-id,
  one-line description, author, and a "N comments" pill when
  threads anchor inside this commit. Clicking a row scopes the
  diff below to just that commit. An "All commits" sentinel at
  the top of the list restores the cumulative diff.
- **Files panel** (centre, below the commits). The diff itself,
  file by file. Each file renders its header (status icon, path,
  add/remove counts), then its hunks, then any inline threads.

The right half of row two of the header (filter chips, view
toggle, comment-nav) is the global commentary control surface. The
commits panel is where the *new* comment affordances live for
non-line-anchored feedback.

---

## 4. Creating a review

The flow is one screen:

1. Pick a *bookmark* from the dropdown of local bookmarks.
2. Optionally type a *revset*. Kata pre-fills the revset to
   `trunk()..<bookmark>`. The author can replace this with any
   `jj` revset; the form previews the resolved commits live.
3. Optionally type a *name* and *summary*. The name defaults to
   the bookmark; the summary is plain Markdown.
4. **Create.**

The new review opens at `/r/<repo>/<number>`. The first patchset is
recorded automatically as part of creation.

If the revset doesn't resolve (typo, ambiguous symbol, divergent
change), the form surfaces the error inline with a quick-fix
suggestion ("run `jj abandon <id>` for the version you don't
want").

---

## 5. Reading a diff

### 5.1 Rendering modes

The diff renders side-by-side on wide viewports and unified on
narrow ones; the breakpoint is wide enough that two columns of
code stay readable.

For modified files, the system computes a per-line and per-word
diff:
- changed lines get a row tint (green for adds, red for removes),
- changed *words* within a line get a stronger inline highlight on
  top of the row tint, so the eye lands on the actual edits rather
  than scanning the whole row.

Added and removed files render as a single column with no pairing.
Renames are detected explicitly and rendered with both paths in
the header.

### 5.2 Whole-file vs. hunks

Diffs render as hunks-with-context by default. The file header
exposes a toggle that swaps to a continuous *whole file* view of
the tip side, while keeping the same word-level highlights inline
— useful when the surrounding code (not just the changed lines)
matters for understanding the edit. The toggle is per-file and
remembered across sessions for that path.

### 5.3 Syntax highlighting

Tokenised highlighting runs against the whole file (not just the
hunk), so multi-line constructs render correctly. Comments are
overlaid on the highlighted tokens, so a tinted line keeps its
syntax colours.

### 5.4 Side-by-side mechanics

The split between base and tip is shared across every SBS hunk on
the page; dragging the divider on one hunk rebalances all of
them. Snap-to-centre at 50% makes the common case effortless.
Each side scrolls horizontally on its own when the code is wider
than the column; *neither* side scrolls vertically (the page
scroll handles that). Holding `Shift` while scrolling horizontally
moves both sides together.

### 5.5 File-level fold

The file header carries a `▾` toggle that collapses the file's
diff (and all its inline threads) to a single header row.
Collapse state is per-path and remembered across sessions, so a
file the user collapsed mid-review stays collapsed when they come
back later — a noisy lockfile doesn't keep re-expanding every
visit.

---

## 6. Commenting

Comments are anchored to where the reviewer's attention was when
they wrote them. Kata exposes three entry points for that.

### 6.1 Inline (line / partial-selection)

The principal flow:

1. The reviewer drag-selects text in the diff. The selection
   highlights as usual.
2. A small popup appears just above the selection with three
   actions: **Comment**, **Copy**, and (for review authors) **N**
   (for *Note*, see §7).
3. Clicking **Comment** opens an inline composer pinned to the
   anchor row. The composer pre-fills with the selected text in
   the body's quote area.
4. The reviewer writes Markdown, picks a flag (must-do /
   suggestion / question), and submits.

Selection that covers a single line creates a line-anchored
comment. Selection that covers a sub-line range — e.g. just the
identifier inside a longer line — creates a partial-selection
comment whose anchor remembers the column range. A multi-line
selection anchors the whole range.

The reviewer can also *click the gutter* on any line to open a
fresh composer for that single line, with no selected text.

### 6.2 Per-commit

Each commit row in the commits panel carries a 💬 button on the
right edge. Clicking it opens a composer for a comment scoped to
the whole commit. These comments have no file or line; they
appear under the commit row in the panel itself, not in the diff.

### 6.3 Review-wide

The "All commits" sentinel at the top of the commits panel has the
same 💬 button. It opens a composer for a comment scoped to the
review as a whole. Review-wide comments live in a dedicated
"Review-wide" block above the commits list.

### 6.4 The composer

The composer is identical for all three entry points: a Markdown
text area, a severity-flag selector, **Cancel**, and **Save
draft**. Saving puts the draft in the reviewer's session;
publishing happens later, from the review-level toolbar.

A draft is editable until it's published. Clicking **Edit** on a
draft swaps in a composer pre-filled with the draft's body; the
original draft hides while the editor is open to avoid a confusing
"two of the same thread" appearance.

---

## 7. Annotations

Annotations are the author's counterpart to comments and use the
same UI plumbing (drag-select → popup → composer), with two
differences:

- The popup's **N** button is visible only to the author of the
  review, and only it opens an annotation composer (not a comment
  composer).
- Annotations have no severity, no thread, no resolution, no
  responses. The composer is the body field and a Save button.

Existing annotations render alongside threads at the same anchor.
They use an **amber** colour scheme (border, background, left
rule) versus the **neutral** scheme reviewer comments use, so the
two are unmistakable. The annotation block carries the same
permalink, copy-Markdown, edit, and delete affordances as a
comment thread.

When an anchor carries multiple things — say one comment plus one
annotation — each gets a per-item fold chevron so they can be
collapsed individually. Single-item anchors use the bulk gutter
marker (see §11) for the same purpose.

---

## 8. Drafts and sessions

The reviewer's flow is:

1. **Open a review.** A session opens implicitly the first time
   the reviewer writes anything. The session is per-(reviewer,
   review); two reviewers on the same review have independent
   sessions.
2. **Draft freely.** Every comment / response / status change
   accumulates in the session as a draft. Drafts are visible to
   the reviewer (clearly tagged) and invisible to everyone else.
3. **Publish.** A single **Publish (N)** button in the header
   flushes the whole batch atomically. Collaborators see the
   entire round of feedback appear at once, with a single
   timestamp.
4. **Or discard.** A **Discard drafts** button throws the entire
   in-progress batch away. The session ends.

After publishing, the published comments are addressable by anyone
viewing the review. The reviewer can open a new session at any
time and start another round.

### 8.1 Why batch?

Publishing piecemeal trains authors to react to every comment as it
arrives, which is exhausting and often wasted work — the reviewer
might be about to retract or rephrase the next thread. Atomic
publish gives the reviewer room to think, reorganise their
feedback, and only commit to it once. It also gives the author one
notification per round instead of one per comment.

### 8.2 Drafts survive

Drafts live on disk in the session record. Closing the browser
tab, reloading the page, or coming back tomorrow does not lose
them; the reviewer's drafts re-appear, exactly as left, when they
re-open the review.

---

## 9. Resolution

Every comment is *open*, *resolved*, or *won't-fix*. Resolution
state changes through responses (see §2.5). A response's
resolution action determines how the system treats the thread
going forward:

- **comment** keeps the state unchanged. Most replies use this
  action.
- **resolve** closes the thread. Resolved threads fold by default
  (see §11) to remove clutter once a discussion is done.
- **won't-fix** also closes the thread, but carries a different
  social meaning: the author is acknowledging the issue and
  declining to address it in this review. Reviewers and tooling
  can distinguish "fixed" from "won't fix" in queries.
- **unresolve** reopens a closed thread. Useful when a previous
  resolution turned out to be premature.

### 9.1 Unread replies

When a thread receives a response newer than the viewer's last
visit (and not authored by the viewer themselves), the thread is
flagged as having *unread replies*. Two visible consequences:

- The thread keeps an "open" appearance — it stays expanded even
  if it would otherwise auto-fold because of its resolution. A
  fresh response by a collaborator can't hide behind the
  resolver's fold.
- The comment-navigation counter and prev/next buttons treat
  unread threads as priority targets.

The viewer's "last visit" is the wall-clock timestamp of their
previous open of the review. First-time visitors don't see unread
markers (there's no past visit to compare against).

---

## 10. Multiple patchsets

The patchset picker (§3.1) is the primary control. The viewer
defaults to showing the latest patchset. Switching patchsets via
the dropdown changes:

- which version of the file content the diff is built against,
- which anchors are considered "current",
- which comments' anchor states (valid / moved / drifted /
  outdated) are recomputed.

The URL gets a `?ps=N` query parameter so the patchset choice is
shareable.

### 10.1 Refreshing

When the underlying bookmark advances after the latest patchset
was recorded, a quiet banner appears on the review summary:

> *The branch has moved since the latest patchset was recorded —
> refresh to capture it.*

Clicking **Refresh** re-resolves the revset and records a new
patchset if the endpoints have moved. The system never refreshes
automatically; mere passive viewing of a review doesn't create
patchsets.

### 10.2 Per-patchset comment authorship

Every comment remembers which patchset it was written against. The
header chip on a comment exposes this as "PSN" with two flavours:

- If PSN is the current view, it's a plain badge.
- If PSN is *not* the current view, the badge becomes a button:
  clicking it jumps the viewer to PSN and scrolls to the comment
  in that view. This is the canonical way to read a comment "in
  the context the reviewer wrote it in" when the anchor has
  drifted on the current view.

---

## 11. Patchset compare (interdiff)

The header's **compared to** dropdown picks a second patchset to
compare against the first.

When set to "base" (the default), the diff shows the cumulative
change from the patchset's base to its tip — i.e. everything in
the patchset.

When set to another patchset *M*, the diff switches to a
*patchset-to-patchset* view:

- **Cumulative interdiff** — by default, the file panel shows
  everything that changed in the review between PSm and PSn (file
  by file).
- **Per-pair (commit-level) interdiff** — the commits panel
  reshapes itself into a *pair list*, one row per change-id, with
  a status badge:
  - **changed** — content edits, descriptions edits, or both.
  - **added** — change is in the *to* patchset but not in *from*.
  - **removed** — change is in *from* but not in *to*.
  - **same** — present in both with identical content (used to
    confirm a commit was *not* touched).
  - **rebased-only** — content was identical but the commit-id
    changed because its parent did. Filterable as a separate
    bucket so noise-rebases don't clutter the list.

  Clicking a *changed* pair scopes the file panel to that pair's
  interdiff.

- **Bases-differ warning** — when the two patchsets' bases differ,
  a banner makes it explicit: the diff includes upstream movement,
  not just author edits, and the reader should adjust their mental
  model accordingly.

A breadcrumb at the top of the files panel keeps the reader's mode
explicit: *Showing: Cumulative · PSn → PSm (12 files)* or *Showing:
∼ <change> · <one-line description>*, with a one-click "← cumulative"
escape back to the all-files view.

---

## 12. Filtering, navigation, and view mode

The header's row-two controls collectively let the reader focus
the viewer on whatever they want to see right now without losing
the rest.

### 12.1 Filter chips

Two clusters of toggle chips:
- **Status**: Draft, Open, Resolved.
- **Severity**: Must-do, Suggestion, Question.

A chip is *on* when its background is filled; clicking toggles
inclusion. Threads are visible only if their status and severity
both pass the active chip set.

When every chip in a row is off (i.e. nothing matches), a one-line
banner appears just before the chips: *Filter hides 12 comments —
show all.* Clicking the banner restores every chip on.

Filter state lives in the URL and persists for the session.

### 12.2 Comment navigation

The `← N/M →` control walks through every comment that survives
the active filter, in reading order (file order, then line order
within a file, with review-wide and commit-level threads
prefixed). The position counter is always live; the keyboard
shortcuts `[` and `]` advance.

This is where the value of filtering compounds: filter out
*resolved*, and the comment navigator becomes a focused tour of
"things still asking for my attention."

### 12.3 View modes

The three-segment control swaps between:
- **Both** — diff and threads, side by side as usual.
- **Diffs** — diff only. Threads collapse into gutter markers; a
  click on the marker re-expands its thread for that one line.
- **Comments** — threads only, full-width, with their inline
  context. The diff hunks compress to a one-line summary so the
  reader still sees what each thread anchors to.

"Diffs" is the right mode for a quick technical read; "Comments"
is the right mode for catching up on conversation; "Both" is the
day-to-day default.

### 12.4 Fold model

Each line that has at least one thread or note carries a *gutter
marker* — a small chevron in the line-number column.

- ▼ = at least one thread at this anchor is expanded.
- ▶ = every thread at this anchor is folded.

Clicking the marker toggles the bulk state: if anything was
expanded, the click folds everything; if everything was folded,
the click expands everything.

When an anchor has 2+ items (e.g. one comment + one annotation, or
two comments on the same line range), each item also carries its
own per-item fold chevron in its header, so the user can collapse
just one of them while leaving the others expanded.

Fold state is per-anchor-id and persists across sessions, so a
thread the user collapsed yesterday stays collapsed when they come
back today.

A *resolved* thread folds by default in **Both** mode and stays
collapsed unless the user explicitly expands it or it receives
unread replies. An *unread* thread always expands, even if its
resolution would otherwise have it fold — the override is one of
the system's strongest defaults, on the theory that a fresh
response should never be hidden from the author of the original
comment.

---

## 13. Permalinks and deep links

Every comment, response, annotation, file, and line has a stable
URL. The URL preserves the patchset/compare/scope state the reader
was in when they copied it, so clicking a permalink in chat or
email lands the recipient in the same view the sender was looking
at.

Hash anchors:
- `#c-<comment_id>` scrolls to a comment thread (or response).
- `#n-<annotation_id>` scrolls to an author annotation.
- `#file-<encoded_path>` scrolls to a file's header.
- `#L:<n>` scrolls to a specific line (with the prevailing patchset
  and side).

Hash anchors are addressable both at first load and at navigation
time; landing on a comment that's currently in a collapsed state
auto-expands it.

---

## 14. Archival

A review can be archived once it's no longer worth attention —
shipped, abandoned, or stale. Archiving:
- adds an **Archived** pill in the title row,
- moves the review to a separate "Archived" bucket in the review
  list (off by default; toggle to show),
- locks editing: no new comments, drafts, responses, or summary
  edits. Existing content stays readable and addressable by
  permalink.

Unarchive reverses every effect. Archive is intentionally cheap
and reversible, not a delete.

---

## 15. Multiple workspaces

A Kata instance can serve many `jj` working copies side-by-side.
Each gets a URL slug; reviews are namespaced per workspace. The
header carries a workspace selector when more than one workspace
is configured, and the home page lists reviews per active
workspace.

There is no cross-workspace search or list. The product treats
workspaces as independent, partly because cross-workspace IDs
collide and partly because cross-workspace reading is an unusual
workflow.

---

## 16. Agent reviewers (MCP)

Kata exposes the same service over MCP — Model Context Protocol —
so an AI agent can read diffs and leave comments through the same
APIs as a human reviewer.

Two pieces:

- **Tools.** `list_repos`, `list_bookmarks`, `list_reviews`,
  `get_review`, `create_review`, `refresh_review`,
  `update_review_summary`, `start_session`, `publish_session`,
  `discard_session`, `draft_line_comment`, `draft_file_comment`,
  `draft_review_comment`, `update_draft_comment`, `respond`. Tools
  are scoped per workspace; `list_repos` returns the slugs the
  agent should pass as `repo`.
- **The `kata-review` skill** is exposed as an MCP resource at
  `skill://kata/review` — pre-built guidance for an agent doing a
  code review (how to structure feedback, when to use which
  severity, when to leave it alone). Reference it from Claude
  Code as `@<server>:skill://kata/review`.

The same review can have human and agent reviewers in flight at
the same time; each gets its own session and its own publish
button. From the author's perspective, the two are
indistinguishable except for the author identity on each thread.

---

## 17. Demo mode

Kata ships with a **demo** subcommand for zero-config evaluation:

```
kata demo --data /tmp/kata-demo
```

This seeds a self-contained `jj` workspace plus SQLite database
with a realistic review (three commits, two published comments by
a reviewer, one author annotation, a second patchset with a
follow-up fix) and starts the regular server pointed at it.

Opening the printed URL with `?demo=1` appended launches a guided
tour: a floating bubble narrates a sequence of steps, each
spotlighting the part of the UI it's describing. The tour covers:
the review list; opening a review; patchsets and "compared to";
the file tree; the commits panel; scoping the diff to a single
commit; inline threads with severity flags; the three ways to
comment (inline / per-commit / review-wide); author annotations;
per-thread and per-file folding; filter chips; comment navigation;
view modes. Sixteen steps total, with `← / →` keyboard navigation,
an Esc-to-leave escape hatch, and a "Skip tour" button that's
always visible.

The tour persists step index in `localStorage`, so a page reload
in the middle resumes where the user was. Closing the tour clears
the latched state, and reloading without `?demo=1` runs Kata
normally.

The seeded review and database are the user's to keep — the demo
is just real Kata data the seed wrote through the normal service
APIs, not a fake. Once the tour is done, the user can poke at
everything, leave their own comments, delete it, or run the demo
command again into a fresh path for a clean state.

---

## 18. URL structure summary

- `/` — review list (current workspace).
- `/r/<repo>/<number>` — review viewer, latest patchset, cumulative
  diff.
- Query string adds optional state:
  - `?ps=N` — current patchset.
  - `?cmp=M` — "compared to" patchset (interdiff mode).
  - `?commit=<change_id>` — pair list scoping in interdiff mode.
  - `?scope=<change_id>` — scope the diff to a single commit
    (outside interdiff mode).
  - `?debug` — opt-in debug affordances.
  - `?demo=1` — arm the guided tour.
- Hash anchors: `#c-<id>`, `#n-<id>`, `#file-<encoded path>`,
  `#L:<n>`.

Every meaningful piece of viewer state is encodable in the URL,
which makes deep-linking and sharing first-class.

---

## 19. Explicitly out of scope

A few capabilities are intentionally omitted, because they would
push the product into territory that conflicts with its model:

- **Force-publishing.** There is no admin override to publish or
  delete another author's draft. Sessions are personal.
- **Comments that "follow" a commit by id.** Comments anchor to
  content; they don't follow `commit_id` or `change_id` directly.
  Following identity would silently move comments onto rewritten
  history without surfacing drift.
- **A separate "approve" / "request changes" verdict.** The unit
  of feedback is the thread, with severity and resolution. There
  is no global thumbs-up; an empty resolved-comment set is the
  approval signal.
- **Cross-workspace search and reviews.** Workspaces are
  independent; reviews don't span them.
- **In-band CI integration.** Kata doesn't know about external
  status checks. (Linking them from the summary is fine; running
  them isn't its job.)

---

## 20. Design principles

The product is shaped by a few principles that recur across every
section above. Stated once, for reference:

1. **Drift is visible, not silent.** When history is rewritten,
   the system either preserves the comment exactly or surfaces the
   drift with chrome. It never relocates without telling the user.
2. **The unit of feedback is a round.** Reviewers batch their
   thoughts and publish atomically. The author gets one
   notification, one timestamp, one set of changes to read.
3. **Reading the diff is the foreground task.** Every piece of
   chrome — filter, navigation, fold, view-mode — exists to
   subtract from the diff when something else is in the way, not
   to add a UI on top of the reading flow.
4. **Permalinks preserve state.** A URL reproduces the view, not
   just the page. Patchset, compare, scope, filter, hash anchor —
   all in the URL.
5. **Same product surface for humans and agents.** MCP and HTTP
   sit on the same review service. An agent leaves the same kinds
   of comments a human would, with the same lifecycle.
6. **Default to discoverable, not magical.** Keyboard shortcuts,
   `?demo=1`, the refresh hint — every behaviour the user has to
   know about is reachable from on-screen affordances or from the
   docs. Hidden shortcuts are escape hatches, not the primary path.
