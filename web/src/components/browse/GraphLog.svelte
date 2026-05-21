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
  import GraphNode from './GraphNode.svelte';
  import GraphLine from './GraphLine.svelte';

  let {
    rows,
    selectedCommitId = null,
    onselect,
    columnWidth = 18,
    rowHeight = 30,
  }: {
    rows: LogRow[];
    selectedCommitId?: CommitId | null;
    onselect: (commitId: CommitId) => void;
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

  function shortId(id: string): string {
    return id.length > 8 ? id.slice(0, 8) : id;
  }
</script>

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
          selected={row.commit.commit_id === selectedCommitId}
        />
      {/each}
    </svg>
    <!-- Text column. Each row is a button so the whole row is a
         click target (selecting the commit). Positioned absolutely
         so vertical alignment with the SVG nodes is exact. -->
    <div class="text-col">
      {#each rows as row (row.commit.commit_id)}
        {@const subj = row.commit.description_first_line || '(no description)'}
        <button
          type="button"
          class="row"
          class:selected={row.commit.commit_id === selectedCommitId}
          style:height="{rowHeight}px"
          onclick={() => onselect(row.commit.commit_id)}
        >
          <span class="subject">{subj}</span>
          {#if (row.bookmarks ?? []).length > 0}
            {#each row.bookmarks ?? [] as bm (bm)}
              <span class="ref">{bm}</span>
            {/each}
          {/if}
          {#if row.is_working_copy}
            <span class="wc">@</span>
          {/if}
          <span class="meta">{shortId(row.commit.commit_id)}</span>
        </button>
      {/each}
    </div>
  </div>
</div>

<style>
  .graph-log {
    overflow-y: auto;
    height: 100%;
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

  .row.selected {
    background: var(--link-bg);
  }

  .subject {
    flex: 0 1 auto;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .ref {
    flex: 0 0 auto;
    font-size: 11px;
    padding: 1px 6px;
    border: 1px solid var(--border);
    border-radius: 4px;
    background: var(--bg-panel);
    color: var(--link);
  }

  .wc {
    flex: 0 0 auto;
    font-size: 11px;
    padding: 1px 6px;
    border-radius: 4px;
    background: var(--link-bg);
    color: var(--link);
    font-weight: 600;
  }

  .meta {
    margin-left: auto;
    flex: 0 0 auto;
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 11px;
    color: var(--text-muted);
  }
</style>
