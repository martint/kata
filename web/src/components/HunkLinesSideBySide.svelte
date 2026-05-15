<script lang="ts">
  import type {
    CommentView,
    ComposerTarget,
    DraftResponseInput,
    Hunk,
    HunkLine,
    ResolutionAction,
    ResponseView,
    Side,
  } from '../lib/types';
  import Bubble from './Bubble.svelte';
  import CommentThread from './CommentThread.svelte';
  import { computeHunkWordDiff, wrapRanges } from '../lib/wordDiff';

  interface Props {
    hunk: Hunk;
    filePath: string;
    comments: CommentView[];
    responses: ResponseView[];
    currentPatchset: number;
    composing: ComposerTarget | null;
    saving: boolean;
    highlights: { base: Map<number, string>; tip: Map<number, string> };
    onstartcompose: (target: ComposerTarget) => void;
    onreply: (input: DraftResponseInput) => Promise<void>;
    onstatus: (commentId: string, action: ResolutionAction) => Promise<void>;
    ondelete: (comment: CommentView) => Promise<void>;
    onedit: (comment: CommentView) => void;
    onselectpatchset: (n: number, commentId?: string) => void;
  }
  const {
    hunk,
    filePath,
    comments,
    responses,
    currentPatchset,
    composing,
    saving,
    highlights,
    onstartcompose,
    onreply,
    onstatus,
    ondelete,
    onedit,
    onselectpatchset,
  }: Props = $props();

  type PairedRow =
    | { kind: 'context'; left: HunkLine; right: HunkLine }
    | { kind: 'change'; left: HunkLine | null; right: HunkLine | null };

  function pair(lines: HunkLine[]): PairedRow[] {
    const rows: PairedRow[] = [];
    let i = 0;
    while (i < lines.length) {
      if (lines[i].origin === 'context') {
        rows.push({ kind: 'context', left: lines[i], right: lines[i] });
        i++;
        continue;
      }
      const removes: HunkLine[] = [];
      while (i < lines.length && lines[i].origin === 'removed') {
        removes.push(lines[i]);
        i++;
      }
      const adds: HunkLine[] = [];
      while (i < lines.length && lines[i].origin === 'added') {
        adds.push(lines[i]);
        i++;
      }
      const max = Math.max(removes.length, adds.length);
      for (let j = 0; j < max; j++) {
        rows.push({
          kind: 'change',
          left: removes[j] ?? null,
          right: adds[j] ?? null,
        });
      }
    }
    return rows;
  }

  const rows = $derived(pair(hunk.lines));
  let dragging: { side: Side; start: number; end: number } | null = $state(null);
  let baseSideEl: HTMLDivElement | undefined = $state();
  let tipSideEl: HTMLDivElement | undefined = $state();
  let baseTableEl: HTMLTableElement | undefined = $state();
  let tipTableEl: HTMLTableElement | undefined = $state();
  let dragSelected: HTMLElement[] = [];

  /** Set --content-vp-width on each side so sticky thread blocks know how
   *  wide to be when content scrolls horizontally beneath them. */
  $effect(() => {
    const ro = new ResizeObserver((entries) => {
      for (const e of entries) {
        (e.target as HTMLElement).style.setProperty(
          '--content-vp-width',
          `${e.contentRect.width}px`,
        );
      }
    });
    if (baseSideEl) ro.observe(baseSideEl);
    if (tipSideEl) ro.observe(tipSideEl);
    return () => ro.disconnect();
  });

  $effect(() => {
    for (const el of dragSelected) el.classList.remove('selected');
    dragSelected = [];
    if (!dragging) return;
    const tableEl = dragging.side === 'base' ? baseTableEl : tipTableEl;
    if (!tableEl) return;
    const min = Math.min(dragging.start, dragging.end);
    const max = Math.max(dragging.start, dragging.end);
    for (let ln = min; ln <= max; ln++) {
      const matches = tableEl.querySelectorAll(
        `[data-side="${dragging.side}"][data-line="${ln}"]`,
      );
      for (const el of matches) {
        (el as HTMLElement).classList.add('selected');
        dragSelected.push(el as HTMLElement);
      }
    }
  });

  function threadsAt(side: Side, line: number | null | undefined): CommentView[] {
    if (line == null) return [];
    return comments.filter((c) => {
      if (c.side !== side) return false;
      const effective =
        c.anchor.kind === 'moved' || c.anchor.kind === 'drifted'
          ? c.anchor.new_lines
          : c.lines;
      return effective != null && effective.end === line;
    });
  }

  /** See `HunkLines.svelte` — same idea, but indexed on the side
   *  this column renders so a multi-line range tints every covered
   *  row, not just the one the thread attaches to. Outdated anchors
   *  are skipped (their range points at content that has since
   *  changed); those threads render at the file level. */
  const commentedLines = $derived.by(() => {
    const set = new Set<string>();
    for (const c of comments) {
      if (!c.side) continue;
      if (c.anchor.kind === 'outdated') continue;
      const effective =
        c.anchor.kind === 'moved' || c.anchor.kind === 'drifted'
          ? c.anchor.new_lines
          : c.lines;
      if (!effective) continue;
      for (let l = effective.start; l <= effective.end; l++) {
        set.add(`${c.side}:${l}`);
      }
    }
    return set;
  });

  function isCommented(side: Side, line: number | null | undefined): boolean {
    return line != null && commentedLines.has(`${side}:${line}`);
  }

  function onPointerDown(e: PointerEvent, side: Side, line: number) {
    if (e.button !== 0) return;
    e.preventDefault();
    if (
      e.shiftKey &&
      composing?.kind === 'line' &&
      composing.file === filePath &&
      composing.side === side
    ) {
      onstartcompose({
        kind: 'line',
        file: filePath,
        side,
        startLine: Math.min(composing.startLine, line),
        endLine: Math.max(composing.endLine, line),
      });
      return;
    }
    dragging = { side, start: line, end: line };

    const onMove = (ev: PointerEvent) => {
      if (!dragging) return;
      const el = document.elementFromPoint(ev.clientX, ev.clientY);
      const cell = (el as HTMLElement | null)?.closest(
        '[data-line]',
      ) as HTMLElement | null;
      if (cell && cell.getAttribute('data-side') === side) {
        const ln = Number(cell.getAttribute('data-line'));
        if (!isNaN(ln)) {
          dragging = { ...dragging, end: ln };
        }
      }
    };
    const onUp = () => {
      document.removeEventListener('pointermove', onMove);
      document.removeEventListener('pointerup', onUp);
      if (dragging) {
        const start = Math.min(dragging.start, dragging.end);
        const end = Math.max(dragging.start, dragging.end);
        const s = dragging.side;
        dragging = null;
        onstartcompose({
          kind: 'line',
          file: filePath,
          side: s,
          startLine: start,
          endLine: end,
        });
      }
    };
    document.addEventListener('pointermove', onMove);
    document.addEventListener('pointerup', onUp);
  }

  function lineText(line: HunkLine | null): string {
    if (!line) return '';
    return line.content.replace(/\n$/, '');
  }

  function highlightedLeft(line: HunkLine | null): string | undefined {
    if (!line) return undefined;
    // Left column is the base view; a context line uses base_line (same
    // content as the right side's tip_line either way).
    if (line.base_line != null) return highlights.base.get(line.base_line);
    if (line.tip_line != null) return highlights.tip.get(line.tip_line);
    return undefined;
  }

  function highlightedRight(line: HunkLine | null): string | undefined {
    if (!line) return undefined;
    if (line.tip_line != null) return highlights.tip.get(line.tip_line);
    if (line.base_line != null) return highlights.base.get(line.base_line);
    return undefined;
  }

  /** Word-level diff per hunk-line index, computed against the original
   *  flat `hunk.lines`. Looked up by the rows below using each side's
   *  source HunkLine. */
  const wordDiff = $derived(computeHunkWordDiff(hunk.lines));

  function hunkLineIndex(line: HunkLine | null): number | null {
    if (!line) return null;
    const idx = hunk.lines.indexOf(line);
    return idx < 0 ? null : idx;
  }

  function withWordDiff(html: string | undefined, line: HunkLine | null): string | undefined {
    if (!html || !line) return html;
    const idx = hunkLineIndex(line);
    if (idx == null) return html;
    const wd = wordDiff.get(idx);
    if (!wd) return html;
    return wrapRanges(html, wd.ranges, wd.kind);
  }
</script>

<div class="sbs-pair">
  <div class="sbs-side base" bind:this={baseSideEl}>
    <table class="hunk-half" bind:this={baseTableEl}>
      <colgroup>
        <col class="col-ln" />
        <col class="col-content" />
      </colgroup>
      <tbody>
        {#each rows as row, i (i)}
          {@const leftLine = row.left?.base_line ?? null}
          <tr class="sbs-row {row.kind}">
            <!-- data-side/data-line are also on the gutter cell so the
                 drag-selection logic finds it via `elementFromPoint` even
                 when the user's cursor stays in the sticky gutter column. -->
            <td
              class="ln {row.left ? row.left.origin : 'empty'}"
              data-side="base"
              data-line={leftLine ?? ''}
            >
              <!-- "+" button lives in the sticky gutter cell, not the
                   content cell, so it stays visible during horizontal
                   scroll of long lines. -->
              {#if row.left?.base_line != null}
                <button
                  type="button"
                  class="add-comment"
                  title="Click to comment; click-drag to extend"
                  onpointerdown={(e) => onPointerDown(e, 'base', row.left!.base_line!)}
                >
                  <Bubble size={12} />
                </button>
              {/if}
              {row.left?.base_line ?? row.left?.tip_line ?? ''}
            </td>
            <td
              class={`content ${row.left ? row.left.origin : 'empty'}${isCommented('base', leftLine) ? ' commented' : ''}`}
              data-side="base"
              data-line={leftLine ?? ''}
            >
              {#if row.left}
                {@const html = withWordDiff(highlightedLeft(row.left), row.left)}
                <pre>{#if html}{@html html}{:else}{lineText(row.left) || ' '}{/if}</pre>
              {/if}
            </td>
          </tr>
          {@const leftThreads = threadsAt('base', row.left?.base_line)}
          {#if leftThreads.length > 0}
            <tr class="sbs-threads from-{row.left?.origin ?? 'context'}">
              <td colspan="2" class="thread-cell">
                <!-- Indent past the side's line-number gutter via
                     padding rather than an empty cell — see
                     HunkLines.svelte for the rationale. -->
                <div class="thread-sticky" style="--gutter-offset: 65px">
                  <CommentThread
                    comments={leftThreads}
                    {responses}
                    {saving}
                    {currentPatchset}
                    {onreply}
                    {onstatus}
                    {ondelete}
                    {onedit}
                    {onselectpatchset}
                  />
                </div>
              </td>
            </tr>
          {/if}
        {/each}
      </tbody>
    </table>
  </div>
  <div class="sbs-side tip" bind:this={tipSideEl}>
    <table class="hunk-half" bind:this={tipTableEl}>
      <colgroup>
        <col class="col-ln" />
        <col class="col-content" />
      </colgroup>
      <tbody>
        {#each rows as row, i (i)}
          {@const rightLine = row.right?.tip_line ?? null}
          <tr class="sbs-row {row.kind}">
            <td
              class="ln {row.right ? row.right.origin : 'empty'}"
              data-side="tip"
              data-line={rightLine ?? ''}
            >
              {#if row.right?.tip_line != null}
                <button
                  type="button"
                  class="add-comment"
                  title="Click to comment; click-drag to extend"
                  onpointerdown={(e) => onPointerDown(e, 'tip', row.right!.tip_line!)}
                >
                  <Bubble size={12} />
                </button>
              {/if}
              {row.right?.tip_line ?? row.right?.base_line ?? ''}
            </td>
            <td
              class={`content ${row.right ? row.right.origin : 'empty'}${isCommented('tip', rightLine) ? ' commented' : ''}`}
              data-side="tip"
              data-line={rightLine ?? ''}
            >
              {#if row.right}
                {@const html = withWordDiff(highlightedRight(row.right), row.right)}
                <pre>{#if html}{@html html}{:else}{lineText(row.right) || ' '}{/if}</pre>
              {/if}
            </td>
          </tr>
          {@const rightThreads = threadsAt('tip', row.right?.tip_line)}
          {#if rightThreads.length > 0}
            <tr class="sbs-threads from-{row.right?.origin ?? 'context'}">
              <td colspan="2" class="thread-cell">
                <!-- Indent past the side's line-number gutter via
                     padding rather than an empty cell — see
                     HunkLines.svelte for the rationale. -->
                <div class="thread-sticky" style="--gutter-offset: 65px">
                  <CommentThread
                    comments={rightThreads}
                    {responses}
                    {saving}
                    {currentPatchset}
                    {onreply}
                    {onstatus}
                    {ondelete}
                    {onedit}
                    {onselectpatchset}
                  />
                </div>
              </td>
            </tr>
          {/if}
        {/each}
      </tbody>
    </table>
  </div>
</div>

<style>
  .sbs-pair {
    display: flex;
    align-items: flex-start;
    gap: 0;
  }

  .sbs-side {
    flex: 1 1 50%;
    min-width: 0;
    overflow-x: auto;
    overscroll-behavior-x: contain;
  }

  .sbs-side.base {
    border-right: 1px solid var(--border);
  }

  .hunk-half {
    width: max-content;
    min-width: 100%;
    /* `separate` (rather than collapse) is needed for sticky cells in
     * Firefox to keep their backgrounds — see HunkLines.svelte. */
    border-collapse: separate;
    border-spacing: 0;
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 12.5px;
    /* Match HunkLines so unified vs side-by-side feel identical. */
    line-height: 1.6;
  }

  /* See HunkLines.svelte for the rationale — keep cells top-aligned so
   * the inline-composer's padding-bottom on `.ln` doesn't drop the
   * code line into the composer overlay. */
  .hunk-half td {
    vertical-align: top;
  }

  .col-ln {
    width: 48px;
  }

  .ln {
    text-align: right;
    padding: 0 8px;
    color: var(--text-faint);
    user-select: none;
    border-right: 1px solid var(--border-muted);
    font-size: 11px;
    background: var(--bg);
    /* Pin the line-number gutter (and the "+" button it now contains) so
     * they stay visible while long lines scroll horizontally. */
    position: sticky;
    left: 0;
    z-index: 1;
    text-align: right;
  }

  .content {
    /* Symmetric padding — the "+" button moved into the sticky `.ln` cell,
     * so the content cell no longer reserves a left margin for it. */
    padding: 0 8px;
  }

  .content pre {
    margin: 0;
    font: inherit;
    white-space: pre;
  }

  .ln.added,
  .content.added {
    background: var(--add-bg);
  }
  .ln.added {
    background: var(--add-bg-strong);
  }

  .ln.removed,
  .content.removed {
    background: var(--remove-bg);
  }
  .ln.removed {
    background: var(--remove-bg-strong);
  }

  /* Word-level diff overlay: the column-tinted cells say a line
   * changed; these stronger backgrounds say which specific characters
   * differ. `:global` because we inject the spans into shiki's
   * pre-rendered HTML via `wrapRanges`. */
  :global(.content.removed .wd-removed) {
    background: var(--remove-word-bg);
    border-radius: 2px;
  }

  :global(.content.added .wd-added) {
    background: var(--add-word-bg);
    border-radius: 2px;
  }

  .ln.empty,
  .content.empty {
    background: var(--bg-panel);
  }

  .content.selected {
    box-shadow: inset 4px 0 0 var(--selection-rule);
    background-image: linear-gradient(var(--selection-tint), var(--selection-tint));
  }

  /* Content cell of a row covered by a posted comment's anchor range —
   * tints the row so multi-line ranges visibly span their lines instead
   * of looking attached to just the last one. Stripe matches the
   * `.thread-sticky` accent so the eye links the two together. */
  .content.commented {
    box-shadow: inset 3px 0 0 var(--link);
    background-image: linear-gradient(var(--selection-tint), var(--selection-tint));
  }

  .add-comment {
    position: absolute;
    left: 2px;
    top: 50%;
    transform: translateY(-50%);
    width: 18px;
    height: 18px;
    padding: 0;
    border: 1px solid transparent;
    border-radius: 4px;
    background: transparent;
    color: var(--link);
    font-weight: 600;
    font-size: 12px;
    line-height: 16px;
    cursor: pointer;
    visibility: hidden;
    user-select: none;
    touch-action: none;
  }

  .sbs-row:hover .add-comment {
    visibility: visible;
  }

  .add-comment:hover {
    background: var(--link);
    color: var(--on-accent);
    border-color: var(--link);
  }

  /* See HunkLines.svelte — match the adjacent diff row's tint so the
   * gutter and right-gap don't read as a dark stripe through the
   * column's color. */
  .sbs-threads {
    background: transparent;
  }

  .sbs-threads.from-added {
    background: var(--add-bg);
  }

  .sbs-threads.from-removed {
    background: var(--remove-bg);
  }

  .thread-cell {
    padding: 0;
    background: transparent;
  }

  /* Blue tint + left stripe so inline threads visually separate from
   * surrounding diff rows. See HunkLines.svelte for rationale. */
  .thread-sticky {
    /* See HunkLines.svelte — same indent / right-trim pattern. The
     * inline `--gutter-offset` is one line-number column's width
     * plus a small gap. */
    position: sticky;
    left: var(--gutter-offset);
    margin-left: var(--gutter-offset);
    width: calc(var(--content-vp-width, 100%) - var(--gutter-offset) - 12px);
    background: var(--link-bg);
    padding: 8px 12px;
    border-top: 1px solid var(--border-muted);
    border-bottom: 1px solid var(--border-muted);
    border-left: 3px solid var(--link);
    box-sizing: border-box;
  }
</style>
