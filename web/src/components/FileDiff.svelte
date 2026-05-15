<script lang="ts">
  import { api } from '../lib/api';
  import type {
    CommentView,
    ComposerTarget,
    DraftCommentInput,
    FileChange,
    Hunk,
    HunkLine,
    Patchset,
  } from '../lib/types';
  import { SvelteMap } from 'svelte/reactivity';
  import {
    langForPath,
    loadLang,
    tokenizeWholeFile,
    themeState,
    type LineHighlights,
  } from '../lib/highlight.svelte';
  import CommentComposer from './CommentComposer.svelte';
  import CommentThread from './CommentThread.svelte';
  import HunkLines from './HunkLines.svelte';
  import HunkLinesSideBySide from './HunkLinesSideBySide.svelte';

  import type { DraftResponseInput, ResolutionAction, ResponseView } from '../lib/types';

  interface Props {
    repo: string;
    file: FileChange;
    /** The patchset whose endpoints back the displayed diff. */
    patchset: Patchset;
    comments: CommentView[];
    responses: ResponseView[];
    currentPatchset: number;
    composing: ComposerTarget | null;
    saving: boolean;
    /** Hide the diff hunks and show line-level comments as a flat list.
     *  Used by the top-bar "Comments only" toggle. */
    compact?: boolean;
    /** True while `FileSlot` is fetching the per-file diff. We render a
     *  "Loading…" placeholder instead of "Diff omitted" so the user
     *  gets feedback rather than confusing them into thinking the
     *  diff was suppressed. */
    loadingHunks?: boolean;
    onstartcompose: (target: ComposerTarget) => void;
    oncancelcompose: () => void;
    onsubmit: (input: DraftCommentInput) => Promise<void>;
    onreply: (input: DraftResponseInput) => Promise<void>;
    onstatus: (commentId: string, action: ResolutionAction) => Promise<void>;
    ondelete: (comment: CommentView) => Promise<void>;
    onedit: (comment: CommentView) => void;
    onselectpatchset: (n: number, commentId?: string) => void;
  }
  const {
    repo,
    file,
    patchset,
    comments,
    responses,
    currentPatchset,
    composing,
    saving,
    compact = false,
    loadingHunks = false,
    onstartcompose,
    oncancelcompose,
    onsubmit,
    onreply,
    onstatus,
    ondelete,
    onedit,
    onselectpatchset,
  }: Props = $props();

  let collapsed = $state(false);
  let sectionEl: HTMLElement | undefined = $state();
  let hunksWrapperEl: HTMLDivElement | undefined = $state();
  let composerOverlayEl: HTMLDivElement | undefined = $state();
  let composerTargetEl: HTMLElement | null = null;
  /** Vertical position of the line composer overlay relative to
   *  `hunksWrapperEl`. Null while no line composer is open. */
  let composerTop: number | null = $state(null);
  let hunksEl: HTMLDivElement | undefined = $state();
  let composeSelected: HTMLElement[] = [];

  /** Track the visible width of the file's .hunks scroll viewport so the
   *  sticky thread wrappers inside know how wide to render. */
  $effect(() => {
    if (!hunksEl) return;
    const ro = new ResizeObserver((entries) => {
      for (const e of entries) {
        (e.target as HTMLElement).style.setProperty(
          '--content-vp-width',
          `${e.contentRect.width}px`,
        );
      }
    });
    ro.observe(hunksEl);
    return () => ro.disconnect();
  });

  /** Apply the range-selection highlight to rows within this file when a
   *  line-level composer is open here. Direct DOM so toggling `composing`
   *  doesn't re-evaluate every row in every hunk. */
  $effect(() => {
    for (const el of composeSelected) el.classList.remove('selected');
    composeSelected = [];
    if (!sectionEl) return;
    if (composing?.kind !== 'line' || composing.file !== file.path) return;
    for (let ln = composing.startLine; ln <= composing.endLine; ln++) {
      const matches = sectionEl.querySelectorAll(
        `[data-side="${composing.side}"][data-line="${ln}"]`,
      );
      for (const el of matches) {
        (el as HTMLElement).classList.add('selected');
        composeSelected.push(el as HTMLElement);
      }
    }
  });

  /** Position the line-level composer overlay below the target row. The
   *  `composing-target` class on the row adds `padding-bottom` so the
   *  overlay has space to occupy without overlapping the next row. */
  $effect(() => {
    if (composerTargetEl) {
      composerTargetEl.classList.remove('composing-target');
      composerTargetEl = null;
    }
    composerTop = null;

    if (!composing || composing.kind !== 'line' || composing.file !== file.path) {
      return;
    }
    if (!sectionEl || !hunksWrapperEl) return;

    const target = sectionEl.querySelector(
      `[data-side="${composing.side}"][data-line="${composing.endLine}"]`,
    ) as HTMLElement | null;
    if (!target) return;

    target.classList.add('composing-target');
    composerTargetEl = target;

    requestAnimationFrame(() => {
      if (!composerTargetEl || !hunksWrapperEl) return;
      const tRect = composerTargetEl.getBoundingClientRect();
      const wRect = hunksWrapperEl.getBoundingClientRect();
      const padding =
        parseFloat(getComputedStyle(composerTargetEl).paddingBottom) || 0;
      composerTop = tRect.bottom - wRect.top - padding;
    });
  });

  /** Keep the target row's padding in sync with the composer's actual
   *  rendered height so the gap matches whatever the composer needs. */
  $effect(() => {
    if (!composerOverlayEl || !composerTargetEl) return;
    const ro = new ResizeObserver((entries) => {
      for (const e of entries) {
        if (composerTargetEl) {
          composerTargetEl.style.setProperty(
            '--composer-h',
            `${Math.ceil(e.contentRect.height) + 4}px`,
          );
        }
      }
    });
    ro.observe(composerOverlayEl);
    return () => ro.disconnect();
  });

  const fileComments = $derived(comments.filter((c) => c.file === file.path));
  /** Whole-file comments — those targeting this file with no line range. */
  const fileLevelComments = $derived(
    fileComments.filter((c) => c.lines == null),
  );
  /** Line-level comments on this file, sorted by line for compact-mode
   *  rendering. Each entry is one thread root; the inline hunk view
   *  groups these into row overlays instead. */
  const lineCommentsSorted = $derived(
    fileComments
      .filter((c) => c.lines != null)
      .slice()
      .sort((a, b) => {
        const al = a.lines?.start ?? 0;
        const bl = b.lines?.start ?? 0;
        if (al !== bl) return al - bl;
        return a.created_at.localeCompare(b.created_at);
      }),
  );

  /** Every (side, line) the file's hunks actually render. Inline
   *  comment threads attach next to a matching row, so a comment
   *  anchored to a line that fell outside the diff's surrounding
   *  context becomes invisible — visible only in comments-only mode
   *  (which lists every line comment irrespective of hunk
   *  coverage). We surface those orphan threads explicitly below. */
  const renderedLineKeys = $derived.by(() => {
    const set = new Set<string>();
    for (const h of file.hunks ?? []) {
      for (const ln of h.lines) {
        if (ln.base_line != null) set.add(`base:${ln.base_line}`);
        if (ln.tip_line != null) set.add(`tip:${ln.tip_line}`);
      }
    }
    return set;
  });

  /** Line comments whose anchor line isn't in any rendered hunk row.
   *  Render them at the file level so the inline-diff view doesn't
   *  silently drop them. */
  const orphanLineComments = $derived(
    lineCommentsSorted.filter((c) => {
      if (!c.side || !c.lines) return false;
      // Hunks may not be loaded yet — the open_review payload ships
      // file metadata only and hunks stream in per FileSlot. While
      // we wait, don't classify everything as orphan.
      if (!file.hunks) return false;
      const effective =
        c.anchor.kind === 'moved' || c.anchor.kind === 'drifted'
          ? c.anchor.new_lines
          : c.lines;
      return !renderedLineKeys.has(`${c.side}:${effective.end}`);
    }),
  );

  /** Anchor a file-level comment to the tip side of the viewed patchset. */
  const fileAnchorIds = $derived({
    change: patchset.tip_change,
    commit: patchset.tip_commit,
  });

  /** Anchor ids for line-level composers, picked based on the side the
   *  user clicked. */
  const lineAnchorIds = $derived.by(() => {
    if (composing?.kind === 'line') {
      return composing.side === 'tip'
        ? { change: patchset.tip_change, commit: patchset.tip_commit }
        : { change: patchset.base_change, commit: patchset.base_commit };
    }
    return fileAnchorIds;
  });

  /** Reactive narrow-viewport flag. Updated via the media-query listener
   *  in the effect below; used to fall back to unified diff on phones
   *  (side-by-side is unreadable at <640px). */
  let narrowViewport = $state(false);
  $effect(() => {
    if (typeof window === 'undefined') return;
    const mq = window.matchMedia('(max-width: 640px)');
    narrowViewport = mq.matches;
    const update = (e: MediaQueryListEvent) => (narrowViewport = e.matches);
    mq.addEventListener('change', update);
    return () => mq.removeEventListener('change', update);
  });

  /** Side-by-side for modifications + renames on a wide enough screen;
   *  unified everywhere else. Unified is also easier to read when only
   *  one side of content exists (pure add/delete). */
  const sideBySide = $derived(
    !narrowViewport && (file.status === 'modified' || file.status === 'renamed'),
  );

  /** Files where it makes sense to expand context — both sides exist. */
  const canExpand = $derived(
    file.status === 'modified' || file.status === 'renamed',
  );

  /** Which line-number columns to show:
   *  - added file: only tip lines exist
   *  - deleted file: only base lines exist
   *  - modified / renamed: both */
  const lineNumberMode: 'both' | 'base' | 'tip' = $derived(
    file.status === 'added' ? 'tip' : file.status === 'deleted' ? 'base' : 'both',
  );

  // ---- hunk context expansion -------------------------------------------

  const STEP = 20;
  type Expansion = { above: number; below: number };
  let expansions: Map<number, Expansion> = $state(new Map());
  /** Tip-side file text split into lines (1-based via index+1). Populated by
   *  the tokenize effect when the tip side exists. */
  let tipLines: string[] | null = $state.raw(null);

  function expansionFor(i: number): Expansion {
    return expansions.get(i) ?? { above: 0, below: 0 };
  }

  function expand(i: number, direction: 'above' | 'below', amount: number) {
    if (!tipLines) return;
    const next = new Map(expansions);
    const cur = expansionFor(i);
    next.set(i, { ...cur, [direction]: cur[direction] + amount });
    expansions = next;
  }

  /** Apply the user's expansion settings to a hunk, producing a hunk with
   *  extra context lines prepended/appended (clipped to the file bounds). */
  function withContext(hunk: Hunk, i: number): Hunk {
    const exp = expansionFor(i);
    if ((exp.above === 0 && exp.below === 0) || !tipLines) return hunk;

    const baseStart = hunk.base_range?.start;
    const tipStart = hunk.tip_range?.start;
    const baseEnd = hunk.base_range?.end;
    const tipEnd = hunk.tip_range?.end;
    if (tipStart == null || tipEnd == null) return hunk;

    const before: HunkLine[] = [];
    for (let k = exp.above; k > 0; k--) {
      const tipLn = tipStart - k;
      if (tipLn < 1) continue;
      const content = tipLines[tipLn - 1];
      if (content === undefined) continue;
      before.push({
        origin: 'context',
        base_line: baseStart != null ? baseStart - k : undefined,
        tip_line: tipLn,
        content: content + '\n',
      });
    }

    const after: HunkLine[] = [];
    for (let k = 1; k <= exp.below; k++) {
      const tipLn = tipEnd + k;
      if (tipLn > tipLines.length) break;
      const content = tipLines[tipLn - 1];
      if (content === undefined) break;
      after.push({
        origin: 'context',
        base_line: baseEnd != null ? baseEnd + k : undefined,
        tip_line: tipLn,
        content: content + '\n',
      });
    }

    return {
      base_range:
        hunk.base_range && baseStart != null && baseEnd != null
          ? { start: baseStart - before.length, end: baseEnd + after.length }
          : hunk.base_range,
      tip_range: {
        start: tipStart - before.length,
        end: tipEnd + after.length,
      },
      lines: [...before, ...hunk.lines, ...after],
    };
  }

  function canExpandAbove(eh: Hunk): boolean {
    return (eh.tip_range?.start ?? 1) > 1;
  }
  function canExpandBelow(eh: Hunk): boolean {
    if (!eh.tip_range) return false;
    return tipLines == null || eh.tip_range.end < tipLines.length;
  }

  // Highlights are per-file, indexed by (side, 1-based line number). We
  // tokenize each whole side in one pass so multi-line constructs (block
  // comments, template literals, heredocs) keep grammar state across lines.
  // SvelteMap so `.get()` is reactive per key — rows light up as their
  // line becomes available without re-running the whole row's reactivity.
  let highlightsBase: LineHighlights = $state(new SvelteMap());
  let highlightsTip: LineHighlights = $state(new SvelteMap());
  const highlights = $derived({ base: highlightsBase, tip: highlightsTip });

  const baseSideExists = $derived(
    file.status !== 'added' && !file.binary && file.hunks != null,
  );
  const tipSideExists = $derived(
    file.status !== 'deleted' && !file.binary && file.hunks != null,
  );

  /** Path on the base side. Renames moved the file, so base lives at
   *  `old_path`. Falls back to `path` for non-renames. */
  const basePath = $derived(file.old_path ?? file.path);

  /** Tokenization-relevant inputs, factored out as primitive-valued
   *  $derived's so the effect below only re-runs when the *content* we'd
   *  fetch actually changes — not whenever `file` is replaced with a new
   *  object that still points at the same path/status (e.g. when toggling
   *  the commit-scoped view). Avoids the highlight flash on every click. */
  const tipPath = $derived(file.path);
  const baseCommit = $derived(patchset.base_commit);
  const tipCommit = $derived(patchset.tip_commit);
  const fileLang = $derived(langForPath(file.path));

  /** Load each side's full text, then tokenize each in one pass. Reading
   *  themeState.value here makes the effect re-run on OS theme toggle so
   *  highlights re-color without a reload.
   *
   *  No viewport gate here — `FileSlot` only mounts us when we're near
   *  the viewport, so reaching this effect already implies "worth
   *  tokenizing." */
  $effect(() => {
    void themeState.value;
    // Pin the primitive deps so this effect re-runs only on real changes.
    const lang = fileLang;
    const wantBase = baseSideExists;
    const wantTip = tipSideExists;
    const bPath = basePath;
    const tPath = tipPath;
    const bCommit = baseCommit;
    const tCommit = tipCommit;
    if (lang == null) return;
    let cancelled = false;
    const isCancelled = () => cancelled;
    // Reset on re-run (theme toggle, etc.) so we don't see stale colors.
    highlightsBase = new SvelteMap();
    highlightsTip = new SvelteMap();
    const t0 = performance.now();

    (async () => {
      const [baseText, tipText, h] = await Promise.all([
        wantBase
          ? api.readFile(repo, bCommit, bPath).catch(() => null)
          : Promise.resolve(null),
        wantTip
          ? api.readFile(repo, tCommit, tPath).catch(() => null)
          : Promise.resolve(null),
        loadLang(lang),
      ]);
      if (cancelled) return;

      if (tipText != null) {
        const lines = tipText.split('\n');
        if (lines.length > 0 && lines[lines.length - 1] === '') lines.pop();
        tipLines = lines;
      }

      await Promise.all([
        baseText != null
          ? tokenizeWholeFile(h, baseText, lang, highlightsBase, { isCancelled })
          : Promise.resolve(),
        tipText != null
          ? tokenizeWholeFile(h, tipText, lang, highlightsTip, { isCancelled })
          : Promise.resolve(),
      ]);

      const total = performance.now() - t0;
      if (total > 100) {
        // eslint-disable-next-line no-console
        console.log(
          `[tokenize] ${file.path}: ${total.toFixed(0)}ms (base ${
            highlightsBase.size
          } lines, tip ${highlightsTip.size} lines)`,
        );
      }
    })();
    return () => {
      cancelled = true;
    };
  });
</script>

<section
  bind:this={sectionEl}
  class="file-diff"
  data-file-path={file.path}
>
  <header class="file-header">
    <button
      class="toggle"
      aria-label={collapsed ? 'expand' : 'collapse'}
      onclick={() => (collapsed = !collapsed)}
    >
      {collapsed ? '▸' : '▾'}
    </button>
    <span class="status status-{file.status}">{file.status[0].toUpperCase()}</span>
    <!-- The path wrapper is direction:rtl so `text-overflow: ellipsis`
         falls on the left ("…/short/tail.rs") instead of cutting off the
         filename. <bdi> keeps the path itself rendered LTR. -->
    <span class="path">
      <bdi>
        {#if file.status === 'renamed' && file.old_path}
          <span class="muted">{file.old_path} →</span> {file.path}
        {:else}
          {file.path}
        {/if}
      </bdi>
    </span>
    {#if file.binary}
      <span class="meta">binary</span>
    {/if}
    <button
      type="button"
      class="file-comment"
      aria-label="Comment on this file"
      title="Comment on this file"
      onclick={() => onstartcompose({ kind: 'file', file: file.path })}
    >
      <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true" fill="currentColor">
        <path d="M1 2.75A2.75 2.75 0 0 1 3.75 0h8.5A2.75 2.75 0 0 1 15 2.75v6.5A2.75 2.75 0 0 1 12.25 12H8.06l-2.573 2.573A1.457 1.457 0 0 1 3 13.543V12h-.25A1.75 1.75 0 0 1 1 10.25v-7.5z"/>
      </svg>
    </button>
  </header>

  {#if fileLevelComments.length > 0}
    <div class="file-thread">
      <CommentThread
        comments={fileLevelComments}
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
  {/if}

  <!-- Orphan line comments: anchored to a line that the diff's
       surrounding context didn't include, so the inline hunk view
       has no row to attach them to. Render at the file level so
       they're not silently dropped in show-diffs mode. Suppressed
       in compact mode because the compact-line-list below already
       shows every line comment irrespective of hunk coverage. -->
  {#if !compact && orphanLineComments.length > 0}
    <div class="orphan-threads">
      <p class="muted">
        Anchored outside the diff's context — the lines these comments
        attached to aren't part of the visible hunks.
      </p>
      <CommentThread
        comments={orphanLineComments}
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
  {/if}

  {#if composing && composing.kind === 'file' && composing.file === file.path}
    <div class="file-composer">
      <CommentComposer
        target={composing}
        anchorIds={fileAnchorIds}
        {saving}
        oncancel={oncancelcompose}
        onsubmit={onsubmit}
      />
    </div>
  {/if}

  {#if compact}
    {#if lineCommentsSorted.length > 0}
      <ul class="compact-line-list">
        {#each lineCommentsSorted as c (c.comment_id)}
          <li>
            <div class="compact-line-marker">
              <code>L{c.lines?.start}{c.lines && c.lines.end !== c.lines.start ? `–${c.lines.end}` : ''}</code>
              <span class="muted">{c.side ?? ''}</span>
            </div>
            <CommentThread
              comments={[c]}
              {responses}
              {saving}
              {currentPatchset}
              {onreply}
              {onstatus}
              {ondelete}
              {onedit}
              {onselectpatchset}
            />
          </li>
        {/each}
      </ul>
    {:else if fileLevelComments.length === 0}
      <p class="placeholder muted">No comments on this file.</p>
    {/if}
  {:else if !collapsed}
    {#if file.binary}
      <p class="placeholder">Binary file — diff is not shown.</p>
    {:else if loadingHunks}
      <p class="placeholder muted">Loading diff…</p>
    {:else if !file.hunks}
      <p class="placeholder">Diff omitted (file may exceed the size limit).</p>
    {:else}
      <div class="hunks-wrapper" bind:this={hunksWrapperEl}>
        <div class="hunks" bind:this={hunksEl}>
        {#each file.hunks as _, i (i)}
          {@const eh = withContext(file.hunks[i], i)}
          {#if canExpand && canExpandAbove(eh)}
            <div class="expand-row above">
              <button onclick={() => expand(i, 'above', STEP)} disabled={tipLines == null}>
                ↑ Show {STEP} lines above
              </button>
            </div>
          {/if}
          {#if sideBySide}
            <HunkLinesSideBySide
              hunk={eh}
              filePath={file.path}
              comments={fileComments}
              {responses}
              {currentPatchset}
              {composing}
              {saving}
              {highlights}
              {onstartcompose}
              {onreply}
              {onstatus}
              {ondelete}
              {onedit}
              {onselectpatchset}
            />
          {:else}
            <HunkLines
              hunk={eh}
              filePath={file.path}
              comments={fileComments}
              {responses}
              {currentPatchset}
              {composing}
              {saving}
              {highlights}
              {lineNumberMode}
              {onstartcompose}
              {onreply}
              {onstatus}
              {ondelete}
              {onedit}
              {onselectpatchset}
            />
          {/if}
          {#if canExpand && canExpandBelow(eh)}
            <div class="expand-row below">
              <button onclick={() => expand(i, 'below', STEP)} disabled={tipLines == null}>
                ↓ Show {STEP} lines below
              </button>
            </div>
          {/if}
          {#if i < file.hunks.length - 1}
            <div class="hunk-gap">…</div>
          {/if}
        {/each}
        </div>
        {#if composing?.kind === 'line' && composing.file === file.path && composerTop != null}
          <div
            class="line-composer-overlay"
            bind:this={composerOverlayEl}
            style:top="{composerTop}px"
          >
            <CommentComposer
              target={composing}
              anchorIds={lineAnchorIds}
              {saving}
              oncancel={oncancelcompose}
              onsubmit={onsubmit}
            />
          </div>
        {/if}
      </div>
    {/if}
  {/if}
</section>

<style>
  .file-diff {
    border: 1px solid var(--border);
    border-radius: 6px;
    margin: 16px 0;
    /* overflow: hidden cuts off sticky-positioned children, so don't clip
     * here. The hunk tables manage their own overflow as needed. */
  }

  .file-header {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 12px;
    background: var(--bg-panel);
    border-bottom: 1px solid var(--border);
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 13px;
    /* Pin under the top header (which carries both rows in review
     * mode — see App.svelte). `--app-header-h` is set live by
     * App.svelte's ResizeObserver so this stays correct as the
     * second row appears / disappears. */
    position: sticky;
    top: var(--app-header-h);
    z-index: 10;
  }

  .toggle {
    background: transparent;
    border: none;
    padding: 0;
    font-size: 12px;
    color: var(--text-muted);
    cursor: pointer;
    width: 16px;
  }

  .file-header .path {
    flex: 1 1 auto;
    /* Without min-width:0 the path's intrinsic width pins the header
     * open, which on a phone overflows the viewport and squeezes the
     * diff into a horizontal-scroll slice. direction:rtl moves the
     * ellipsis to the *left* so we keep the filename visible when the
     * path is too long; text-align:left undoes the right-alignment
     * that direction:rtl otherwise imposes on short paths. */
    min-width: 0;
    direction: rtl;
    text-align: left;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .file-header .meta {
    color: var(--text-muted);
    font-size: 12px;
    /* `meta` is the lowest-priority cell in the header: when the path
     * eats all the available width, the hunk count disappears before
     * the filename does. */
    flex: 0 1 auto;
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
    flex-shrink: 100;
  }

  /* Keep status badge + the icon button at full size — both are too
   * small to shrink usefully. */
  .file-header .status,
  .file-comment {
    flex-shrink: 0;
  }

  .file-comment {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 26px;
    height: 22px;
    padding: 0;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 4px;
    color: var(--link);
    cursor: pointer;
  }

  .file-comment:hover {
    background: var(--link-bg);
  }

  .file-thread {
    /* Same accent as inline line-threads (see HunkLines.svelte) — keeps
     * the "this is commentary, not code" cue consistent across file-
     * level and line-level anchors. */
    padding: 8px 12px 8px 14px;
    background: var(--link-bg);
    border-bottom: 1px solid var(--border);
    border-left: 3px solid var(--link);
  }

  /* Orphan-threads block: line comments anchored outside the diff's
   * visible context. The amber left-accent + warn tint distinguishes
   * them from file-level threads (link blue) — they're "should be
   * inline but couldn't be" rather than "explicitly file-scoped." */
  .orphan-threads {
    padding: 8px 12px 8px 14px;
    background: var(--warn-bg);
    border-bottom: 1px solid var(--border);
    border-left: 3px solid var(--warn-text);
  }

  .orphan-threads > p.muted {
    margin: 0 0 6px;
    font-size: 12px;
    color: var(--warn-text);
  }

  .file-composer {
    padding: 8px 12px;
    background: var(--bg-panel);
    border-bottom: 1px solid var(--border);
  }

  .compact-line-list {
    list-style: none;
    margin: 0;
    padding: 0;
  }

  .compact-line-list > li {
    padding: 8px 12px;
    border-top: 1px solid var(--border-muted);
  }

  .compact-line-list > li:first-child {
    border-top: none;
  }

  .compact-line-marker {
    display: flex;
    align-items: baseline;
    gap: 8px;
    margin-bottom: 4px;
    font-size: 12px;
  }

  .compact-line-marker code {
    background: var(--bg-elevated);
    color: var(--text-muted);
    padding: 1px 6px;
    border-radius: 3px;
    font-size: 11px;
  }

  .hunks-wrapper {
    position: relative;
  }

  .line-composer-overlay {
    position: absolute;
    left: 0;
    right: 0;
    /* Must beat .file-header (z-index: 10) so the composer isn't behind
     * the sticky header when commenting on a line near the top. */
    z-index: 12;
    background: var(--bg-panel);
    border-top: 1px solid var(--border);
    border-bottom: 1px solid var(--border);
  }

  /* Push table content down to make room for the absolute-positioned
   * composer overlay. Height set dynamically via --composer-h as the
   * composer's textarea grows / preview toggles. */
  :global(.composing-target) {
    padding-bottom: var(--composer-h, 220px) !important;
  }

  .hunks {
    background: var(--bg);
    /* Single horizontal scroll context for the whole file — long lines
     * scroll the entire hunk pack, not each line independently. */
    overflow-x: auto;
    /* Disable the browser's overscroll-bounce so users can't drag past
     * the diff's left/right edges. */
    overscroll-behavior-x: contain;
  }

  .hunk-gap {
    text-align: center;
    color: var(--text-faint);
    background: var(--bg-panel);
    border-top: 1px solid var(--border-muted);
    border-bottom: 1px solid var(--border-muted);
    padding: 2px 0;
    font-family: ui-monospace, monospace;
    font-size: 11px;
  }

  .expand-row {
    background: var(--bg-panel);
    border-top: 1px solid var(--border-muted);
    border-bottom: 1px solid var(--border-muted);
    padding: 2px 12px;
    text-align: left;
  }

  .expand-row button {
    background: transparent;
    border: none;
    color: var(--link);
    cursor: pointer;
    font: inherit;
    font-family: ui-monospace, monospace;
    font-size: 11px;
    padding: 2px 6px;
    border-radius: 3px;
  }

  .expand-row button:hover {
    background: var(--link-bg);
  }

  .expand-row button:disabled {
    opacity: 0.5;
    cursor: default;
  }

  .placeholder {
    color: var(--text-muted);
    padding: 12px;
    margin: 0;
    font-style: italic;
  }
</style>
