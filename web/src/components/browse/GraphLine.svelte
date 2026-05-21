<!--
  One SVG path per edge in the graph log. The layout pass on the
  server (see kata-jj::log_graph) computed `(col, row)` endpoints
  for every line and tagged each with one of four shape kinds; this
  component just routes the d="…" path between them.

  Coordinate system: columns are 18px wide, rows are 30px tall. A
  node circle sits at the column's centre, mid-row. Lines come out
  of, terminate at, or pass through these centres.
-->
<script lang="ts">
  import type { LogLine } from '../../lib/types';

  let {
    line,
    columnWidth = 18,
    rowHeight = 30,
  }: { line: LogLine; columnWidth?: number; rowHeight?: number } = $props();

  // Centre-of-cell helpers. (col, row) → (x, y) in svg coords.
  function cx(col: number): number {
    return col * columnWidth + columnWidth / 2;
  }
  function cy(row: number): number {
    return row * rowHeight + rowHeight / 2;
  }

  const path = $derived(buildPath(line));

  function buildPath(l: LogLine): string {
    const sx = cx(l.source.col);
    const sy = cy(l.source.row);
    const tx = cx(l.target.col);
    const ty = cy(l.target.row);
    switch (l.kind) {
      case 'to-node':
      case 'to-intersection':
      case 'to-missing': {
        if (sx === tx) {
          // Same column: straight vertical.
          return `M ${sx} ${sy} L ${tx} ${ty}`;
        }
        // Different columns: a cubic curve. Control points sit at
        // the midpoint between the two rows so the curve bends
        // smoothly into the target column. The S-shape mirrors
        // jjuicy's edge style.
        const my = (sy + ty) / 2;
        return `M ${sx} ${sy} C ${sx} ${my}, ${tx} ${my}, ${tx} ${ty}`;
      }
      case 'from-node': {
        if (l.via === undefined) {
          // No intermediate column — same shape as to-node.
          if (sx === tx) return `M ${sx} ${sy} L ${tx} ${ty}`;
          const my = (sy + ty) / 2;
          return `M ${sx} ${sy} C ${sx} ${my}, ${tx} ${my}, ${tx} ${ty}`;
        }
        // Rescue curve: leave source, bend toward `via`, run
        // straight down via, bend into target. Three segments:
        //   1. source → (via, source.row + 1) — initial bend
        //   2. (via, source.row + 1) → (via, target.row - 1) — vertical
        //   3. (via, target.row - 1) → target — final bend
        const vx = cx(l.via);
        const bendY1 = sy + rowHeight / 2;
        const bendY2 = ty - rowHeight / 2;
        return [
          `M ${sx} ${sy}`,
          `C ${sx} ${bendY1}, ${vx} ${bendY1}, ${vx} ${sy + rowHeight}`,
          `L ${vx} ${ty - rowHeight}`,
          `C ${vx} ${bendY2}, ${tx} ${bendY2}, ${tx} ${ty}`,
        ].join(' ');
      }
    }
  }
</script>

<path d={path} class="graph-line" class:missing={line.kind === 'to-missing'} />

<style>
  .graph-line {
    fill: none;
    stroke: var(--graph-stroke, var(--text-muted));
    stroke-width: 1.5;
    stroke-linecap: round;
    stroke-linejoin: round;
  }

  /* Lines to commits outside the revset render dashed so a reader
   * can tell at a glance "this stem leaves the visible graph". */
  .graph-line.missing {
    stroke-dasharray: 3 3;
    opacity: 0.6;
  }
</style>
