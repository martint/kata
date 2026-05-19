//! Keep the visible content in place across DOM mutations that
//! might happen off-screen.
//!
//! The browser's built-in scroll anchoring (`overflow-anchor: auto`)
//! is supposed to do this for free, but it's unreliable in our
//! layout: sticky file headers are skipped as anchor candidates,
//! the IntersectionObserver-driven `FileSlot` virtualisation
//! replaces entire subtrees, and fold/unfold toggles can change
//! the layout above the viewport without the browser picking the
//! right anchor.
//!
//! `preserveScrollAnchor` is the explicit alternative: it takes
//! a snapshot of the topmost visible element's screen-Y, runs the
//! mutation, awaits Svelte's `tick`, and adjusts `window.scroll`
//! by the delta. The visible line at the top of the viewport
//! stays put regardless of what happens above (or below).

import { tick } from 'svelte';

/**
 * Run `work` while keeping the topmost on-screen content visually
 * pinned. Wrap any handler whose state change might add or remove
 * content above the current viewport — fold toggles, file-fold
 * toggles, gutter-marker bulk toggles, hash navigation that
 * collapses other panels, etc.
 *
 * The helper picks an anchor element just below any sticky
 * header, runs the work, awaits Svelte's reactive flush, and
 * scrolls by the delta of the anchor's `getBoundingClientRect()
 * .top`. When the work itself removes the anchor from the DOM —
 * which is a fair signal that the caller is intentionally
 * reorganising — no adjustment is made.
 *
 * Returns whatever `work` returned so callers can `await` it
 * inline without losing the result.
 */
export async function preserveScrollAnchor<T>(
  work: () => T | Promise<T>,
): Promise<T> {
  const anchor = pickAnchor();
  const beforeTop = anchor ? anchor.getBoundingClientRect().top : 0;
  const result = await work();
  await tick();
  if (anchor && anchor.isConnected) {
    const afterTop = anchor.getBoundingClientRect().top;
    const delta = afterTop - beforeTop;
    if (delta !== 0) {
      window.scrollBy(0, delta);
    }
  }
  return result;
}

/**
 * The topmost element currently below any sticky header, walked up
 * to a "stable" wrapper that's unlikely to be replaced by a
 * fold/unfold inside it. Returns `null` when nothing useful is at
 * the probe point (the page is at the very top, the layout is
 * mid-mount, the environment is jsdom which doesn't implement
 * `elementFromPoint`) — callers treat that as "no adjustment".
 */
function pickAnchor(): Element | null {
  if (typeof document === 'undefined' || typeof window === 'undefined') {
    return null;
  }
  if (typeof document.elementFromPoint !== 'function') {
    return null;
  }
  const probeX = Math.max(1, Math.floor(window.innerWidth / 2));
  const probeY = stickyHeaderHeight() + 1;
  let el: Element | null;
  try {
    el = document.elementFromPoint(probeX, probeY);
  } catch {
    return null;
  }
  while (el && !isStableAnchor(el)) {
    el = el.parentElement;
  }
  return el;
}

/**
 * Whether `el` is one of the wrappers we trust to survive a
 * typical fold/unfold without changing identity. A `<span>` token
 * inside a code line can disappear when the line is rewritten by
 * syntax highlighting; the row container around it doesn't. Same
 * idea for `.file-header` (the sticky file row, but sticky elements
 * are *skipped* by `pickAnchor`'s probe-Y because the probe sits
 * just below them).
 */
function isStableAnchor(el: Element): boolean {
  return (
    el.classList.contains('row') ||
    el.classList.contains('sbs-row') ||
    el.classList.contains('file-diff') ||
    el.classList.contains('comment') ||
    el.classList.contains('annotation') ||
    el.classList.contains('thread-sticky')
  );
}

/**
 * Height of the sticky app header, as published by `App.svelte` via
 * the `--app-header-h` custom property. Falls back to a sensible
 * default if the property hasn't been measured yet (mid-mount or
 * mid-teardown).
 */
function stickyHeaderHeight(): number {
  const v = getComputedStyle(document.documentElement).getPropertyValue(
    '--app-header-h',
  );
  const n = parseInt(v, 10);
  return Number.isFinite(n) && n > 0 ? n : 48;
}
