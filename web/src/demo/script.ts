//! The demo tour script — an ordered sequence of `TourStep` entries
//! the overlay walks the user through. Each step pairs a short
//! narration with an optional spotlight target (a `data-tour="…"`
//! selector on the element it describes).
//!
//! Extending the tour:
//!   1. Pick the feature you want to surface.
//!   2. Tag the relevant element with `data-tour="<name>"` so the
//!      step's selector survives component-class refactors.
//!   3. Add a new `TourStep` here, in a position that respects the
//!      script's narrative flow (start → list → review chrome →
//!      diff → comments → annotations → folding → top-bar
//!      filters → done).
//!
//! Adding a step that points at content the demo seed doesn't
//! produce is a bug — the spotlight will sit on an empty page. If
//! you add a step that needs new seed data, extend
//! `crates/kata-demo/src/lib.rs` in the same change.

import type { TourStep } from './types';

export const tour: TourStep[] = [
  {
    id: 'welcome',
    placement: 'center',
    title: 'Welcome to Kata',
    body:
      "Kata is a code review tool built on jj. This guided tour " +
      "walks through the core workflow on a tiny seeded review. Hit " +
      "Next (or → on your keyboard) to advance, Back (←) to step " +
      "back, Esc to leave the tour.",
  },
  {
    id: 'review-list',
    page: '/',
    target: '[data-tour=review-list]',
    placement: 'right',
    title: 'Reviews on this repo',
    body:
      "Each row is a branch under review. Kata creates a review by " +
      "pinning a jj revset; advancing the branch on top of that " +
      "revset records a new patchset on the same review — no force-" +
      "push, no losing thread anchors.",
  },
  {
    id: 'open-review',
    page: '/r/demo/1',
    target: '[data-tour=review-summary]',
    placement: 'bottom',
    title: 'A review',
    body:
      "A review is the manifest (revset, patchsets, summary) plus " +
      "the diff and any comments. The header keeps the summary " +
      "editable; the diff and threads live below.",
  },
  {
    id: 'patchset-picker',
    target: '[data-tour=patchset-picker]',
    placement: 'bottom',
    title: 'Patchsets and interdiffs',
    body:
      "This review has two patchsets — PS1 (the initial round) and " +
      "PS2 (Alice's follow-up). The left dropdown picks which " +
      "patchset you're reading. Set the right dropdown (\"compared " +
      "to\") to a different patchset to see the interdiff — what " +
      "changed between rounds, not just the cumulative diff against " +
      "the base.",
  },
  {
    id: 'file-tree',
    setup: { click: '.panel-toggle.collapsed' },
    target: '[data-tour=file-tree]',
    placement: 'right',
    title: 'Changed files',
    body:
      "The left pane lists every file in the diff with adds/removes " +
      "per file. Click a file to scroll its diff into view, type in " +
      "the filter box to narrow the list, or use the arrows in the " +
      "header to walk file-by-file.",
  },
  {
    id: 'commits-panel',
    target: '[data-tour=commits-panel]',
    placement: 'right',
    title: 'Commits in this patchset',
    body:
      "Every commit in the revset shows up here, newest first. The " +
      "panel also hosts review-wide and per-commit comment buckets " +
      "— anything that doesn't anchor to a specific line lives here " +
      "rather than inline in the diff.",
  },
  {
    id: 'scope-to-commit',
    target: '[data-tour=commit-row]',
    placement: 'right',
    title: 'Scope to a single commit',
    body:
      "Click a commit row to scope the diff below to just that " +
      "commit — useful for reviewing a stack one commit at a time. " +
      "\"All commits\" at the top of the panel restores the " +
      "cumulative diff for the whole review.",
  },
  {
    id: 'inline-thread',
    target: '[data-tour=comment-thread]',
    placement: 'left',
    title: 'Inline threads',
    body:
      "Comments anchor to specific lines (or partial selections) in " +
      "the diff. Each thread tracks resolution; published responses " +
      "surface as \"unread\" until the author acknowledges. Severity " +
      "(must-do / suggestion / question) shows as a chip in the " +
      "header so a skim tells you what kind of feedback it is.",
  },
  {
    id: 'comment-targets',
    target: '[data-tour=add-review-comment]',
    placement: 'right',
    title: 'Three ways to comment',
    body:
      "Comments come in three flavours, each with its own entry " +
      "point: drag-select text in the diff to anchor to specific " +
      "lines; click the 💬 next to a commit row to comment on the " +
      "whole commit; or click this 💬 to comment on the review as a " +
      "whole. Authors get a fourth option — annotations — covered " +
      "next.",
  },
  {
    id: 'annotation',
    target: '[data-tour=annotation]',
    placement: 'left',
    title: 'Author annotations',
    body:
      "An annotation is a one-sided note from the review's author " +
      "— no thread, no resolution, just context the reviewer needs " +
      "before reading the diff. Colour-coded amber so you can tell " +
      "at a glance it's from the author, not a reviewer. The author " +
      "creates one via the same drag-select popup that creates " +
      "inline comments.",
  },
  {
    id: 'thread-fold',
    target: '[data-tour=thread-fold]',
    placement: 'left',
    title: 'Fold an individual thread',
    body:
      "When two or more comments or notes share an anchor (here: " +
      "Bob's question and Alice's annotation), each gets its own " +
      "fold chevron so you can collapse the ones you've read while " +
      "keeping the rest open. Single-anchor threads use the gutter " +
      "marker instead — same idea, less chrome.",
  },
  {
    id: 'file-fold',
    target: '[data-tour=file-fold]',
    placement: 'right',
    title: 'Fold a whole file',
    body:
      "The ▾ in each file header collapses that file's diff (and " +
      "all its threads) to just its header row. Useful when a " +
      "review touches a generated file or a noisy lockfile you " +
      "want out of the way while you read the substantive changes.",
  },
  {
    id: 'filter-chips',
    target: '[data-tour=filter-chips]',
    placement: 'bottom',
    title: 'Filter the noise',
    body:
      "Chips on the top bar filter threads by status (draft / open " +
      "/ resolved) and severity (must-do / suggestion / question). " +
      "Toggle off everything resolved to focus on what still needs " +
      "your attention.",
  },
  {
    id: 'comment-nav',
    target: '[data-tour=comment-nav]',
    placement: 'bottom',
    title: 'Jump between comments',
    body:
      "Use ← / → here (or the keyboard) to step through every " +
      "comment in reading order. Great for a second pass once " +
      "you've skimmed the diff — you won't miss a thread tucked " +
      "into a collapsed file.",
  },
  {
    id: 'view-toggle',
    target: '[data-tour=view-toggle]',
    placement: 'bottom',
    title: 'Focus the layout',
    body:
      "Switch between Both (diff + threads), Diffs (just the code), " +
      "and Comments (just the threads). Helpful when you want to " +
      "speed-read either the code or the conversation without the " +
      "other in the way.",
  },
  {
    id: 'done',
    placement: 'center',
    title: "That's it for the tour",
    body:
      "You've seen the main moving parts: reviews, patchsets, the " +
      "diff, threads, annotations, folding, filtering, and " +
      "navigation. Poke around — everything you saw is real data " +
      "the demo wrote through the normal service APIs. Hit Done to " +
      "leave the tour.",
  },
];
