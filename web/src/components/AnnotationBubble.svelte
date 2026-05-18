<script lang="ts">
  //! One author-annotation rendered at its anchor.
  //!
  //! Annotations are review-creator-only context notes — no thread,
  //! no replies, no resolution. The bubble is intentionally lighter
  //! than `CommentThread`: amber accent (vs the comment blue) so the
  //! reader can tell at a glance "this is the author's note, not a
  //! reviewer comment", and a single body block — no header chips,
  //! no draft/state badges. Edit and delete affordances ship in a
  //! follow-up commit; this is display-only.

  import { renderMarkdown } from '../lib/markdown';
  import type { AnnotationView } from '../lib/types';

  interface Props {
    annotation: AnnotationView;
    /** When `true` the bubble exposes Edit + Delete affordances.
     *  Caller is responsible for actually gating to creator-only —
     *  this is a thin "should the controls render" toggle. */
    canEdit?: boolean;
    onedit?: (annotation: AnnotationView) => void;
    ondelete?: (annotation: AnnotationView) => Promise<void>;
  }
  const {
    annotation,
    canEdit = false,
    onedit = () => {},
    ondelete = async () => {},
  }: Props = $props();

  let deleting = $state(false);

  async function onDeleteClick() {
    if (deleting) return;
    if (!confirm('Delete this annotation? This cannot be undone.')) return;
    deleting = true;
    try {
      await ondelete(annotation);
    } finally {
      deleting = false;
    }
  }

  function formatDate(iso: string): string {
    const d = new Date(iso);
    if (Number.isNaN(d.getTime())) return iso;
    return d.toLocaleString();
  }
</script>

<div class="annotation">
  <header class="head">
    <span class="badge">Note</span>
    <span class="author">{annotation.author}</span>
    <time class="at" datetime={annotation.created_at}>
      {formatDate(annotation.created_at)}
    </time>
    {#if annotation.updated_at !== annotation.created_at}
      <span class="edited" title={`edited ${formatDate(annotation.updated_at)}`}
        >· edited</span
      >
    {/if}
    {#if canEdit}
      <span style="flex: 1"></span>
      <button
        type="button"
        class="action"
        title="Edit this note"
        onclick={() => onedit(annotation)}>Edit</button
      >
      <button
        type="button"
        class="action danger"
        title="Delete this note"
        disabled={deleting}
        onclick={onDeleteClick}>{deleting ? 'Deleting…' : 'Delete'}</button
      >
    {/if}
  </header>
  <div class="body markdown">{@html renderMarkdown(annotation.body)}</div>
</div>

<style>
  .annotation {
    border: 1px solid var(--attention-border);
    background: var(--attention-bg);
    border-radius: 4px;
    padding: 6px 10px;
    margin: 4px 0;
    /* Left rule echoes CommentThread's accent stripe — but in amber
     * so the eye registers this as a different category of artefact
     * before reading the badge. */
    box-shadow: inset 3px 0 0 var(--attention-border);
  }

  .head {
    display: flex;
    align-items: baseline;
    gap: 8px;
    font-size: 11px;
    color: var(--attention-text);
    margin-bottom: 4px;
  }

  .action {
    background: transparent;
    border: 1px solid var(--attention-border);
    color: var(--attention-text);
    border-radius: 3px;
    padding: 1px 8px;
    font-size: 11px;
    cursor: pointer;
  }
  .action:hover {
    background: var(--attention-border);
    color: var(--bg);
  }
  .action.danger {
    border-color: var(--remove-bg-strong, #d34545);
    color: var(--remove-bg-strong, #d34545);
  }
  .action.danger:hover {
    background: var(--remove-bg-strong, #d34545);
    color: var(--bg);
  }

  .badge {
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    font-size: 10px;
    /* Inverted chip so "Note" is more visually distinct than the rest
     * of the header text, but still on-palette with the bubble. */
    background: var(--attention-border);
    color: var(--bg);
    padding: 1px 6px;
    border-radius: 3px;
  }

  .author {
    font-weight: 600;
  }

  .at {
    color: var(--text-faint);
  }

  .edited {
    color: var(--text-faint);
    font-style: italic;
  }

  .body {
    font-size: 13px;
    color: var(--text);
  }

  .body :global(p:first-child) {
    margin-top: 0;
  }
  .body :global(p:last-child) {
    margin-bottom: 0;
  }
</style>
