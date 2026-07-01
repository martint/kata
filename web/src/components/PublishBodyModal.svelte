<!--
  Modal that collects the review body for a GitHub Approve or
  Request-changes submission. Shared by the toolbar's split
  "Publish to GitHub" button and the top-bar's standalone
  "Request changes" action so the body-capture UX stays consistent.

  The caller owns the open/closed state (`event`); rendering
  `<PublishBodyModal>` shows the modal, unmounting it closes it.
  Submit and cancel are surfaced as `onsubmit` / `onclose` callbacks.

  Request-changes requires a non-empty body (GitHub itself rejects an
  empty REQUEST_CHANGES review), so the Submit button is disabled
  until the user types something. Approve is optional — the empty
  case is fine and we don't want to force a textarea on a thumbs-up.
-->
<script lang="ts">
  import { portal } from '../lib/portal';

  type PublishEvent = 'APPROVE' | 'REQUEST_CHANGES';

  let {
    event,
    saving,
    returnFocusTo = null,
    onsubmit,
    onclose,
  }: {
    event: PublishEvent;
    saving: boolean;
    /** The element that should regain focus when the modal
     *  unmounts — usually the button that opened it. Passed in by
     *  the caller because capturing `document.activeElement` at
     *  mount would race the modal's own `textarea.focus()` autofocus
     *  effect: Svelte 5 runs effects in declaration order, so any
     *  effect running after the autofocus one sees the textarea as
     *  the active element, not the true opener. `null` disables
     *  restoration (accepted for callers that don't have a stable
     *  trigger to return to). */
    returnFocusTo?: HTMLElement | null;
    onsubmit: (body: string | undefined) => void | Promise<void>;
    onclose: () => void;
  } = $props();

  let body = $state('');
  let textarea: HTMLTextAreaElement | undefined = $state();
  let dialogEl: HTMLDivElement | undefined = $state();

  const isApprove = $derived(event === 'APPROVE');

  $effect(() => {
    // Autofocus so the user can start typing immediately. The bind
    // callback isn't a good hook for this because it runs before the
    // element is attached to the DOM.
    if (textarea) textarea.focus();
  });

  $effect(() => {
    // Backs the `aria-modal="true"` promise: on unmount, return
    // focus to the caller-supplied trigger so keyboard / AT users
    // end up where they started rather than at `<body>`.
    return () => {
      returnFocusTo?.focus();
    };
  });

  /** Focusable elements inside the dialog, in DOM order. Recomputed
   *  on each Tab because the set changes when the primary button
   *  disables/enables. Filters out anything the browser wouldn't
   *  reach with a normal Tab. */
  function focusables(): HTMLElement[] {
    if (!dialogEl) return [];
    const nodes = dialogEl.querySelectorAll<HTMLElement>(
      'button:not([disabled]), textarea:not([disabled]), input:not([disabled]), select:not([disabled]), a[href], [tabindex]:not([tabindex="-1"])',
    );
    return Array.from(nodes);
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.stopPropagation();
      onclose();
      return;
    }
    if (e.key !== 'Tab') return;
    // Trap Tab / Shift-Tab within the dialog so focus can't escape
    // to the greyed-out page behind the backdrop. Standard cycle:
    // Tab past the last element → first; Shift-Tab past first →
    // last. If nothing is focusable (shouldn't happen — there's
    // always a Cancel button), let the browser do its thing.
    const items = focusables();
    if (items.length === 0) return;
    const first = items[0];
    const last = items[items.length - 1];
    const active = document.activeElement as HTMLElement | null;
    if (e.shiftKey && active === first) {
      e.preventDefault();
      last.focus();
    } else if (!e.shiftKey && active === last) {
      e.preventDefault();
      first.focus();
    }
  }

  async function submit() {
    if (!isApprove && !body.trim()) return;
    const trimmed = body.trim() ? body : undefined;
    await onsubmit(trimmed);
  }

</script>

<svelte:window onkeydown={onKey} />

<div class="modal-backdrop" role="presentation" onclick={onclose} use:portal></div>
<div
  class="modal"
  role="dialog"
  aria-modal="true"
  aria-label={isApprove ? 'Publish and approve' : 'Publish and request changes'}
  bind:this={dialogEl}
  use:portal
>
  <header class="modal-head">
    <span class="modal-title">
      {isApprove ? 'Publish & approve' : 'Publish & request changes'}
    </span>
    <button
      type="button"
      class="modal-close"
      onclick={onclose}
      aria-label="Close"
    >✕</button>
  </header>
  <div class="modal-body">
    <label class="modal-label" for="publish-body">
      {isApprove ? 'Review body (optional)' : 'Review body (required)'}
    </label>
    <textarea
      id="publish-body"
      bind:this={textarea}
      bind:value={body}
      rows="6"
      placeholder={isApprove
        ? 'Optional summary attached to the approval…'
        : 'Explain what needs to change before this PR can merge…'}
    ></textarea>
    <p class="modal-help">
      {isApprove
        ? 'Posted as the review summary on github.com alongside any inline drafts.'
        : 'GitHub requires a non-empty body on a REQUEST_CHANGES review. This text is posted as the review summary.'}
    </p>
  </div>
  <footer class="modal-actions">
    <button type="button" onclick={onclose} disabled={saving}>Cancel</button>
    <button
      type="button"
      class="primary"
      onclick={submit}
      disabled={saving || (!isApprove && !body.trim())}
    >
      {saving
        ? 'Publishing…'
        : isApprove
          ? 'Publish & approve'
          : 'Publish & request changes'}
    </button>
  </footer>
</div>

<style>
  .modal-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.5);
    z-index: 1100;
  }

  .modal {
    position: fixed;
    top: 50%;
    left: 50%;
    transform: translate(-50%, -50%);
    z-index: 1101;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 8px;
    box-shadow: 0 12px 32px rgba(0, 0, 0, 0.32);
    width: min(520px, calc(100vw - 32px));
    max-height: calc(100vh - 64px);
    display: flex;
    flex-direction: column;
  }

  .modal-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 12px 16px;
    border-bottom: 1px solid var(--border-muted);
  }

  .modal-title {
    font-size: 14px;
    font-weight: 600;
  }

  .modal-close {
    width: 28px;
    height: 28px;
    background: transparent;
    border: none;
    border-radius: 4px;
    color: var(--text-muted);
    cursor: pointer;
    font-size: 16px;
  }
  .modal-close:hover {
    background: var(--bg-elevated);
  }

  .modal-body {
    padding: 14px 16px 6px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .modal-label {
    font-size: 12px;
    font-weight: 600;
    color: var(--text-muted);
  }

  .modal-body textarea {
    font: inherit;
    font-size: 13px;
    width: 100%;
    box-sizing: border-box;
    padding: 8px 10px;
    border: 1px solid var(--border);
    border-radius: 4px;
    background: var(--bg);
    color: inherit;
    resize: vertical;
    min-height: 120px;
  }

  .modal-help {
    margin: 0;
    font-size: 11px;
    color: var(--text-muted);
  }

  .modal-actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    padding: 12px 16px;
    border-top: 1px solid var(--border-muted);
  }
</style>
