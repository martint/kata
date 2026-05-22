<!--
  The node marking one commit's row in the graph.

    - working-copy: rendered as a literal `@` glyph, not a circle —
      it's the one row the reader most needs to spot.
    - bookmarked: filled circle in the accent colour.
    - immutable (ancestor of `trunk()`): faint fill — settled
      history, visually subordinate to mutable in-progress work.
    - mutable: solid fill — the commits that are still editable.

  The renderer above draws a backdrop rectangle behind the node so
  line ends hide cleanly behind it instead of poking past the edge.
-->
<script lang="ts">
  let {
    col,
    row,
    columnWidth = 18,
    rowHeight = 30,
    isWorkingCopy = false,
    hasBookmarks = false,
    immutable = false,
    selected = false,
  }: {
    col: number;
    row: number;
    columnWidth?: number;
    rowHeight?: number;
    isWorkingCopy?: boolean;
    hasBookmarks?: boolean;
    immutable?: boolean;
    selected?: boolean;
  } = $props();

  const cx = $derived(col * columnWidth + columnWidth / 2);
  const cy = $derived(row * rowHeight + rowHeight / 2);
</script>

<!-- Backdrop hides line ends so the node visually owns its centre. -->
<rect
  x={cx - 6}
  y={cy - 6}
  width="12"
  height="12"
  class="node-backdrop"
/>
{#if isWorkingCopy}
  <text
    x={cx}
    y={cy}
    class="wc-marker"
    class:selected
    text-anchor="middle"
    dominant-baseline="central"
  >@</text>
{:else}
  <circle
    cx={cx}
    cy={cy}
    r="4"
    class="node"
    class:bookmarked={hasBookmarks}
    class:immutable
    class:selected
  />
{/if}

<style>
  .node-backdrop {
    fill: var(--bg);
  }

  /* Mutable commits — still-editable work — get a solid fill so
   * they read as the "active" part of the graph. */
  .node {
    fill: var(--text);
    stroke: var(--bg);
    stroke-width: 1;
  }

  /* Immutable history (ancestors of `trunk()`) recedes — a faint
   * fill keeps it legible without competing with mutable work. */
  .node.immutable {
    fill: var(--text-faint);
  }

  .node.bookmarked {
    fill: var(--link);
  }

  .node.selected {
    stroke: var(--link);
    stroke-width: 2;
  }

  /* `@` glyph for the working copy — the single most important
   * row to locate in a scrollable log. */
  .wc-marker {
    fill: var(--link);
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 14px;
    font-weight: 700;
  }
  .wc-marker.selected {
    paint-order: stroke;
    stroke: var(--link);
    stroke-width: 0.6;
  }
</style>
