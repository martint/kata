<!--
  The circle marking one commit's row in the graph. Visual variants:
    - `working-copy` (`@`): filled hollow ring — the user's `@`.
    - `bookmarked`: filled circle, bookmark colour.
    - default: filled circle, muted colour.

  The renderer above draws a backdrop rectangle behind the node
  before the circle, so line ends hide cleanly behind it instead of
  poking past the edge.
-->
<script lang="ts">
  let {
    col,
    row,
    columnWidth = 18,
    rowHeight = 30,
    isWorkingCopy = false,
    hasBookmarks = false,
    selected = false,
  }: {
    col: number;
    row: number;
    columnWidth?: number;
    rowHeight?: number;
    isWorkingCopy?: boolean;
    hasBookmarks?: boolean;
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
<circle
  cx={cx}
  cy={cy}
  r={isWorkingCopy ? 5 : 4}
  class="node"
  class:working-copy={isWorkingCopy}
  class:bookmarked={hasBookmarks}
  class:selected
/>

<style>
  .node-backdrop {
    fill: var(--bg);
  }

  .node {
    fill: var(--text-muted);
    stroke: var(--bg);
    stroke-width: 1;
  }

  .node.bookmarked {
    fill: var(--link);
  }

  /* `@` renders as a hollow ring so the working copy stands out
   * unmistakably among regular commits in a scrollable log. */
  .node.working-copy {
    fill: var(--bg);
    stroke: var(--link);
    stroke-width: 2;
  }

  .node.selected {
    stroke: var(--link);
    stroke-width: 2;
  }
</style>
