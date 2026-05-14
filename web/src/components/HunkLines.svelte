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
  import CommentThread from './CommentThread.svelte';

  interface Props {
    hunk: Hunk;
    filePath: string;
    comments: CommentView[];
    responses: ResponseView[];
    composing: ComposerTarget | null;
    saving: boolean;
    /** Per-side, 1-based line number → rendered HTML. Populated from a
     *  whole-file tokenization so multi-line constructs render correctly. */
    highlights: { base: Map<number, string>; tip: Map<number, string> };
    /** 'both' for modified/renamed files; 'base' for deleted; 'tip' for added. */
    lineNumberMode?: 'both' | 'base' | 'tip';
    onstartcompose: (target: ComposerTarget) => void;
    onreply: (input: DraftResponseInput) => Promise<void>;
    onstatus: (commentId: string, action: ResolutionAction) => Promise<void>;
    ondelete: (comment: CommentView) => Promise<void>;
    onedit: (comment: CommentView) => void;
  }
  const {
    hunk,
    filePath,
    comments,
    responses,
    composing,
    saving,
    highlights,
    lineNumberMode = 'both',
    onstartcompose,
    onreply,
    onstatus,
    ondelete,
    onedit,
  }: Props = $props();

  const showBase = $derived(lineNumberMode !== 'tip');
  const showTip = $derived(lineNumberMode !== 'base');
  const lnCols = $derived((showBase ? 1 : 0) + (showTip ? 1 : 0));
  /** ln cells + the single content cell (which also hosts the `+` button). */
  const colspan = $derived(lnCols + 1);

  let dragging: { side: Side; start: number; end: number } | null = $state(null);
  let tableEl: HTMLTableElement | undefined = $state();
  let dragSelected: HTMLElement[] = [];

  /** Apply the `.selected` class directly to matching rows when dragging.
   *  We bypass per-row Svelte reactivity here because toggling a top-level
   *  state like `composing` would otherwise re-evaluate every row's @const,
   *  which is O(file_size). */
  $effect(() => {
    for (const el of dragSelected) el.classList.remove('selected');
    dragSelected = [];
    if (!tableEl || !dragging) return;
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

  function rowClass(origin: HunkLine['origin']): string {
    switch (origin) {
      case 'added':
        return 'row added';
      case 'removed':
        return 'row removed';
      case 'context':
        return 'row context';
    }
  }

  /** Find the pre-tokenized HTML for a unified-diff line. Removed lines
   *  look up on the base side; added & context use the tip side. */
  function htmlFor(line: HunkLine): string | undefined {
    if (line.origin === 'removed') {
      return line.base_line != null ? highlights.base.get(line.base_line) : undefined;
    }
    if (line.tip_line != null) return highlights.tip.get(line.tip_line);
    if (line.base_line != null) return highlights.base.get(line.base_line);
    return undefined;
  }

  /** Which side+line a row anchors to. Removed → base side; added & context → tip. */
  function anchor(line: HunkLine): { side: Side; line: number } | null {
    if (line.origin === 'removed' && line.base_line != null) {
      return { side: 'base', line: line.base_line };
    }
    if (line.tip_line != null) {
      return { side: 'tip', line: line.tip_line };
    }
    if (line.base_line != null) {
      return { side: 'base', line: line.base_line };
    }
    return null;
  }

  function threadsFor(a: { side: Side; line: number }): CommentView[] {
    return comments.filter((c) => {
      if (c.side !== a.side) return false;
      const effective =
        c.anchor.kind === 'moved' || c.anchor.kind === 'drifted'
          ? c.anchor.new_lines
          : c.lines;
      return effective != null && effective.end === a.line;
    });
  }

  /** Set of `side:line` keys for every line covered by a comment's
   *  anchored range. Used to tint the line so the reader can see what
   *  the thread (rendered below the last line) is talking about — a
   *  five-line range otherwise looks just like a thread attached to
   *  one line. The thread is still anchored to `effective.end`; this
   *  just decorates the rest of the range. */
  const commentedLines = $derived.by(() => {
    const set = new Set<string>();
    for (const c of comments) {
      if (!c.side) continue;
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

  function isCommented(a: { side: Side; line: number } | null): boolean {
    return a != null && commentedLines.has(`${a.side}:${a.line}`);
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
      const row = (el as HTMLElement | null)?.closest('[data-line]') as HTMLElement | null;
      if (row && row.getAttribute('data-side') === side) {
        const ln = Number(row.getAttribute('data-line'));
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
</script>

<table class="hunk" bind:this={tableEl}>
  <tbody>
    {#each hunk.lines as line, i (i)}
      {@const a = anchor(line)}
      {@const stripped = line.content.replace(/\n$/, '')}
      {@const html = htmlFor(line)}
      <tr class={`${rowClass(line.origin)}${isCommented(a) ? ' commented' : ''}`}>
        {#if showBase}
          <!-- data-side/data-line are also on the gutter cell so that the
               drag-selection logic finds it via `elementFromPoint` while
               the user drags down the sticky gutter column. -->
          <td
            class="ln"
            data-side={a?.side ?? ''}
            data-line={a?.line ?? ''}
          >
            <!-- "+" button lives in the gutter cell (not the content
                 cell) so it stays visible while long lines scroll
                 horizontally — the gutter is `position: sticky`. -->
            {#if a && !showTip}
              <button
                type="button"
                class="add-comment"
                title="Click to comment; click-drag (or shift+click) to extend to multiple lines"
                onpointerdown={(e) => onPointerDown(e, a.side, a.line)}>+</button
              >
            {/if}
            {line.base_line ?? ''}
          </td>
        {/if}
        {#if showTip}
          <td
            class="ln"
            data-side={a?.side ?? ''}
            data-line={a?.line ?? ''}
          >
            {#if a}
              <button
                type="button"
                class="add-comment"
                title="Click to comment; click-drag (or shift+click) to extend to multiple lines"
                onpointerdown={(e) => onPointerDown(e, a.side, a.line)}>+</button
              >
            {/if}
            {line.tip_line ?? ''}
          </td>
        {/if}
        <td
          class="content"
          data-side={a?.side ?? ''}
          data-line={a?.line ?? ''}
        >
          {#if html}
            <pre>{@html html || '&nbsp;'}</pre>
          {:else}
            <pre>{stripped || ' '}</pre>
          {/if}
        </td>
      </tr>
      {#if a}
        {@const threads = threadsFor(a)}
        {#if threads.length > 0}
          <tr class="thread-row">
            <td colspan={colspan} class="thread-cell">
              <div class="thread-sticky">
                <CommentThread
                  comments={threads}
                  {responses}
                  {saving}
                  {onreply}
                  {onstatus}
                  {ondelete}
                  {onedit}
                />
              </div>
            </td>
          </tr>
        {/if}
      {/if}
    {/each}
  </tbody>
</table>

<style>
  .hunk {
    width: max-content;
    min-width: 100%;
    /* Firefox bug: `position: sticky` on a `<td>` doesn't reliably paint
     * its own background when the table uses `border-collapse: collapse`,
     * so the scrolling diff text bleeds visibly through the sticky
     * gutter cell. `separate` + zero border-spacing keeps the layout
     * identical while letting each sticky cell own its background. */
    border-collapse: separate;
    border-spacing: 0;
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 12.5px;
    /* Slightly looser than the global 1.45 — code lines packed at body
     * line-height read as cramped. Mirrored on .hunk-half. */
    line-height: 1.6;
  }

  /* Keep code aligned to the top of its row even when the sibling .ln
   * cell grows tall via the `.composing-target` padding-bottom hack
   * (FileDiff applies it when an inline composer opens). Without this,
   * the default `baseline` vertical-align centers the code line in
   * the now-tall row, which looks like the line "shifts down" and
   * leaves the composer overlay sitting on top of it. */
  .hunk td {
    vertical-align: top;
  }

  /* `.row` itself needs no rules — line-height inherits from the table's
   * default, matching the side-by-side renderer. An explicit value here
   * previously made unified rows 2.5px taller and read as inconsistent
   * spacing across diff styles. */

  .row.added {
    background: var(--add-bg);
  }

  .row.removed {
    background: var(--remove-bg);
  }

  .row.context {
    background: var(--bg);
  }

  /* Drag-selection lands on the content TD (that's where data-side/data-line
   * live), not the row, so the rule targets that cell. */
  .content.selected {
    box-shadow: inset 4px 0 0 var(--selection-rule);
    background-image: linear-gradient(var(--selection-tint), var(--selection-tint));
  }

  /* A row covered by a posted comment's anchor range. Tints the content
   * cell so the reader sees which line(s) the thread (rendered below
   * the last covered row) is about — particularly important for
   * multi-line ranges, which would otherwise look indistinguishable
   * from a thread attached to a single line. The left stripe matches
   * the `.thread-sticky` accent so the eye links the two together. */
  .row.commented .content {
    box-shadow: inset 3px 0 0 var(--link);
    background-image: linear-gradient(var(--selection-tint), var(--selection-tint));
  }

  .ln {
    width: 48px;
    text-align: right;
    padding: 0 8px;
    color: var(--text-faint);
    user-select: none;
    background: var(--bg);
    border-right: 1px solid var(--border-muted);
    /* Pin the gutter to the left edge of the horizontal scroll context so
     * the line number AND the "+" button stay visible while long lines
     * scroll out to the right. The textarea-focus workaround in
     * CommentComposer.svelte already absorbs the ~1.5s Firefox cost that
     * scaled with the number of sticky cells, so we can afford this. */
    position: sticky;
    left: 0;
    z-index: 1;
  }

  .row.added .ln {
    background: var(--add-bg-strong);
  }

  .row.removed .ln {
    background: var(--remove-bg-strong);
  }

  .content {
    /* Padding moved to symmetric 8px now that the "+" button lives inside
     * the .ln gutter cell — no need to reserve a left margin in the
     * content cell. */
    padding: 0 8px;
  }

  .content pre {
    margin: 0;
    font: inherit;
    white-space: pre;
    overflow-wrap: normal;
    word-break: normal;
  }

  /* "+" sits inside the sticky `.ln` cell so it scrolls vertically with
   * the row but stays pinned to the left edge during horizontal scroll.
   * The cell is text-align: right (for the line number); we position the
   * button absolutely at the left so the two don't fight over space. */
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

  .row:hover .add-comment {
    visibility: visible;
  }

  .add-comment:hover {
    background: var(--link);
    color: var(--on-accent);
    border-color: var(--link);
  }

  .thread-row {
    background: transparent;
  }

  .thread-cell {
    padding: 0;
    background: transparent;
  }

  /* The sticky wrapper lets the thread stay at the visible viewport while
   * the underlying table scrolls horizontally. Width comes from the file's
   * .hunks ResizeObserver (see FileDiff).
   *
   * The blue tint + left stripe makes inline comment threads pop against
   * the surrounding diff rows; with the previous --bg-panel background
   * they blended into context lines and were easy to miss. */
  .thread-sticky {
    position: sticky;
    left: 0;
    width: var(--content-vp-width, 100%);
    background: var(--link-bg);
    padding: 8px 12px 8px 14px;
    border-top: 1px solid var(--border-muted);
    border-bottom: 1px solid var(--border-muted);
    border-left: 3px solid var(--link);
    box-sizing: border-box;
  }
</style>
