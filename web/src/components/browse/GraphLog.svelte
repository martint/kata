<!--
  The graph-log pane. Renders the column-stem graph that the
  backend laid out, with the textual commit metadata sitting in
  HTML to the right of each row's SVG nodes.
  The layout itself (column assignments + edge paths) is computed
  server-side; this component is the dumb renderer.

  Structure per row:
    +-------------------+---------------------------+
    | <svg graph area>  | <html text area>          |
    | nodes + edges     | description + meta        |
    +-------------------+---------------------------+

  Both areas share a single scroll container so they move in lock-
  step. Edges that span multiple rows are drawn ONCE as part of
  the overall SVG above the rows, not duplicated per row — same
  trick jjuicy's GraphLog uses.
-->
<script lang="ts">
  import type { LogRow, CommitId } from '../../lib/types';
  import { copyText } from '../../lib/clipboard';
  import GraphNode from './GraphNode.svelte';
  import GraphLine from './GraphLine.svelte';
  import RevId from '../RevId.svelte';

  let {
    repo,
    rows,
    selectedCommitId = null,
    rangeIds = new Set(),
    onselect,
    columnWidth = 18,
    rowHeight = 30,
  }: {
    repo: string;
    rows: LogRow[];
    /** The "anchor" commit — the one shown in the detail pane.
     *  Always also a member of `rangeIds`. */
    selectedCommitId?: CommitId | null;
    /** Commit-ids in the current selection range. One element
     *  for a plain click; many for a shift-click range. Rows in
     *  this set get the `.in-range` highlight. */
    rangeIds?: Set<string>;
    /** Plain click sets `extendRange: false` (resets the
     *  selection to just this row). Shift-click sets
     *  `extendRange: true` so the parent can extend a range. */
    onselect: (commitId: CommitId, opts: { extendRange: boolean }) => void;
    columnWidth?: number;
    rowHeight?: number;
  } = $props();

  /** Width of the SVG portion, in pixels. Driven by the widest
   *  row's effective graph width (column + padding) so the text
   *  column starts at the same x across the whole pane. */
  const graphWidth = $derived.by(() => {
    if (rows.length === 0) return 0;
    let max = 0;
    for (const r of rows) {
      const w = r.location.col + 1 + r.padding;
      if (w > max) max = w;
    }
    return Math.max(1, max) * columnWidth + columnWidth;
  });

  const totalHeight = $derived(rows.length * rowHeight);

  // Flat list of lines + their owning row index, for the single
  // SVG render below. Drawn once each (each line has a unique
  // identity in its owning row, so no dedup needed — the layout
  // emits exactly one LogLine per visual edge).
  const allLines = $derived.by(() => {
    const out: { line: import('../../lib/types').LogLine; rowIdx: number }[] = [];
    rows.forEach((r, rowIdx) => {
      for (const line of r.lines) out.push({ line, rowIdx });
    });
    return out;
  });

  let rowsEl: HTMLElement | undefined = $state();

  /** Up / down arrow keys step the selection through the graph —
   *  the keyboard counterpart to clicking a row. Ignored while a
   *  text field has focus (the revset box) so typing isn't
   *  hijacked, and when a modifier is held. */
  function onKey(e: KeyboardEvent) {
    if (e.key === 'Escape' && bmMenu) {
      bmMenu = null;
      return;
    }
    if (e.key !== 'ArrowUp' && e.key !== 'ArrowDown') return;
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    const t = e.target as HTMLElement | null;
    if (t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA')) return;
    if (rows.length === 0) return;
    const cur = rows.findIndex((r) => r.commit.commit_id === selectedCommitId);
    // No selection yet → arrow-down lands on the first row,
    // arrow-up on the last.
    let next: number;
    if (cur < 0) {
      next = e.key === 'ArrowDown' ? 0 : rows.length - 1;
    } else {
      next = cur + (e.key === 'ArrowDown' ? 1 : -1);
      if (next < 0 || next >= rows.length) return;
    }
    e.preventDefault();
    const id = rows[next].commit.commit_id;
    onselect(id, { extendRange: false });
    rowsEl
      ?.querySelector(`[data-row="${next}"]`)
      ?.scrollIntoView({ block: 'nearest' });
  }

  /** Right-click context menu for a bookmark chip. Anchored at the
   *  cursor; both actions are read-only (the browser never mutates
   *  the repo) — "Create review" hands off to the new-review form,
   *  "Copy" puts the bookmark name on the clipboard. */
  let bmMenu = $state<{ x: number; y: number; bookmark: string } | null>(null);

  function openBmMenu(e: MouseEvent, bookmark: string) {
    e.preventDefault();
    e.stopPropagation();
    bmMenu = { x: e.clientX, y: e.clientY, bookmark };
  }

  function bmCreateReview() {
    if (!bmMenu) return;
    // `trunk()..<bookmark>` is the natural "everything on this
    // branch" revset; the new-review form opens pre-filled with it.
    const expr = `trunk()..${bmMenu.bookmark}`;
    location.href = `/?prefill_revset=${encodeURIComponent(expr)}`;
  }

  async function bmCopy() {
    if (bmMenu) await copyText(bmMenu.bookmark);
    bmMenu = null;
  }
</script>

<svelte:window onkeydown={onKey} onclick={() => (bmMenu = null)} />

<div class="graph-log">
  <div class="rows" style:--graph-width="{graphWidth}px">
    <!-- Single SVG layer covers all rows; edges and nodes live
         here so they share coordinates and clip cleanly. -->
    <svg
      class="graph-svg"
      width={graphWidth}
      height={totalHeight}
      viewBox={`0 0 ${graphWidth} ${totalHeight}`}
      aria-hidden="true"
    >
      {#each allLines as { line } (line)}
        <GraphLine {line} {columnWidth} {rowHeight} />
      {/each}
      {#each rows as row (row.commit.commit_id)}
        <GraphNode
          col={row.location.col}
          row={row.location.row}
          {columnWidth}
          {rowHeight}
          isWorkingCopy={row.is_working_copy ?? false}
          hasBookmarks={(row.bookmarks ?? []).length > 0}
          immutable={row.immutable ?? false}
          selected={row.commit.commit_id === selectedCommitId
            || rangeIds.has(row.commit.commit_id)}
        />
      {/each}
    </svg>
    <!-- Text column. Each row is a button so the whole row is a
         click target (selecting the commit). Positioned absolutely
         so vertical alignment with the SVG nodes is exact. -->
    <div class="text-col" bind:this={rowsEl}>
      {#each rows as row, i (row.commit.commit_id)}
        {@const subj = row.commit.description_first_line || '(no description)'}
        <button
          type="button"
          class="row"
          class:selected={row.commit.commit_id === selectedCommitId}
          class:in-range={rangeIds.has(row.commit.commit_id) &&
            row.commit.commit_id !== selectedCommitId}
          class:immutable={row.immutable}
          data-row={i}
          style:height="{rowHeight}px"
          title={subj}
          onclick={(e) =>
            onselect(row.commit.commit_id, { extendRange: e.shiftKey })}
        >
          <span class="subject">{subj}</span>
          {#if (row.bookmarks ?? []).length > 0}
            {#each row.bookmarks ?? [] as bm (bm)}
              <span
                class="ref"
                role="button"
                tabindex="-1"
                title="Right-click for actions"
                oncontextmenu={(e) => openBmMenu(e, bm)}
              >{bm}</span>
            {/each}
          {/if}
          <!-- Trailing change-id only. The commit-id pill was
               dropped — two ids per row crowded the narrow pane and
               the change-id is the one a reader references. The `@`
               marker lives on the graph node, not here. -->
          <span class="meta">
            <RevId id={row.commit.change_id} kind="change" {repo} inline />
          </span>
        </button>
      {/each}
    </div>
  </div>
</div>

{#if bmMenu}
  <div
    class="bm-menu"
    role="menu"
    style:left="{bmMenu.x}px"
    style:top="{bmMenu.y}px"
  >
    <button type="button" role="menuitem" onclick={bmCreateReview}>
      Create review from <strong>{bmMenu.bookmark}</strong>
    </button>
    <button type="button" role="menuitem" onclick={bmCopy}>
      Copy bookmark name
    </button>
  </div>
{/if}

<style>
  /* Fills whatever the parent flex column leaves over (the search
   * bar above it has fixed height). `min-height: 0` is the magic
   * incantation that lets a flex child shrink past its content,
   * which is what enables the scroll bar — without it the
   * container expands to fit the rows and the pane stops
   * scrolling at all. */
  .graph-log {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
  }

  .rows {
    position: relative;
    /* The SVG anchors at left: 0; text starts after the graph
     * width. Using a CSS variable lets the text column know
     * where to indent without a JS resize observer. */
  }

  .graph-svg {
    position: absolute;
    top: 0;
    left: 0;
    pointer-events: none;
  }

  .text-col {
    position: relative;
    margin-left: var(--graph-width);
    display: flex;
    flex-direction: column;
  }

  .row {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 0 12px;
    background: transparent;
    border: none;
    border-radius: 0;
    cursor: pointer;
    text-align: left;
    font: inherit;
    color: inherit;
    /* No min-width:0 dance because we control the column. */
  }

  .row:hover {
    background: var(--bg-elevated);
  }

  /* Anchor row of a selection — the row whose detail shows in the
   * right pane. Always also `.in-range`. The inset left bar in the
   * accent colour is what separates it from the plain range tint:
   * the background difference alone (full vs. 35% `--link-bg`) is
   * too subtle to read at a glance across a range of rows. */
  .row.selected {
    background: var(--link-bg);
    box-shadow: inset 3px 0 0 var(--link);
  }

  /* Non-anchor rows in a shift-extended range. A faint tint — the
   * eye should land on the anchor's accent bar, not compete with a
   * second strong fill. */
  .row.in-range {
    background: color-mix(in srgb, var(--link-bg) 35%, transparent);
  }

  .subject {
    flex: 0 1 auto;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  /* Immutable rows recede — settled history reads as subordinate
   * to mutable in-progress work. Matches the faint graph node. */
  .row.immutable .subject {
    color: var(--text-faint);
  }

  .ref {
    flex: 0 0 auto;
    font-size: 11px;
    padding: 1px 6px;
    border: 1px solid var(--border);
    border-radius: 4px;
    background: var(--bg-panel);
    color: var(--link);
    cursor: context-menu;
  }

  /* Trailing change-id pill, pushed to the row's right edge. */
  .meta {
    margin-left: auto;
    flex: 0 0 auto;
    display: inline-flex;
  }

  /* Right-click menu for a bookmark chip. `position: fixed`,
   * anchored at the cursor; closes on any click or Escape. */
  .bm-menu {
    position: fixed;
    z-index: 1000;
    min-width: 180px;
    display: flex;
    flex-direction: column;
    padding: 4px;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 6px;
    box-shadow: 0 4px 16px rgba(0, 0, 0, 0.15);
  }
  .bm-menu button {
    text-align: left;
    background: transparent;
    border: none;
    border-radius: 4px;
    padding: 6px 10px;
    font: inherit;
    color: inherit;
    cursor: pointer;
    white-space: nowrap;
  }
  .bm-menu button:hover {
    background: var(--bg-elevated);
  }
</style>
