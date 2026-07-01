<!--
  Top-toolbar cluster of three actions that apply to any GitHub-bound
  review, whether or not the user has draft comments queued:

    - **Approve**: submit `event=APPROVE` immediately, no body prompt.
      Matches the muscle-memory of GitHub's own "Approve" one-click
      button — a thumbs-up doesn't need a textarea.
    - **Request changes**: open the shared PublishBodyModal to collect
      a required review body, then submit `event=REQUEST_CHANGES`.
    - **Refresh**: re-import the PR discussion + head SHA from
      github.com. Same code path the head-drift recovery uses.

  These sit next to the commit-navigation chevrons in row 1 of the
  sticky header so they're always reachable — the split "Publish to
  GitHub" button in the draft cluster is only visible when the user
  has drafts to publish, which leaves quick-approve and refresh
  homeless. This component fills that gap.
-->
<script lang="ts">
  import PublishBodyModal from './PublishBodyModal.svelte';

  let {
    approve,
    requestChanges,
    refresh,
    saving,
    refreshing,
  }: {
    approve: () => Promise<boolean> | boolean;
    /** Returns `true` on success, `false` on failure. The modal
     *  stays open on `false` so the user's typed body — which
     *  REQUEST_CHANGES requires and which can be several
     *  paragraphs — isn't lost to a network or head-drift error. */
    requestChanges: (body: string) => Promise<boolean> | boolean;
    refresh: () => Promise<void> | void;
    saving: boolean;
    refreshing: boolean;
  } = $props();

  let modalOpen = $state(false);
  /** The "Request changes" button — captured so we can return
   *  focus to it when the modal closes. */
  let requestBtnEl: HTMLButtonElement | undefined = $state();

  async function submitRequestChanges(body: string | undefined) {
    // Modal enforces non-empty body for REQUEST_CHANGES, so this
    // branch is only reached with a real string. The signature
    // still accepts `undefined` because PublishBodyModal is shared
    // with the (optional-body) APPROVE flow.
    if (!body) return;
    // Keep the modal mounted through the await so failure doesn't
    // discard the typed body.
    const ok = await requestChanges(body);
    if (ok) modalOpen = false;
  }
</script>

<div class="gh-actions" role="group" aria-label="GitHub review actions">
  <button
    type="button"
    class="gh-btn approve"
    onclick={() => approve()}
    disabled={saving || refreshing}
    title="Approve this PR on github.com (no body)"
    data-tour="gh-quick-approve"
  >
    <span class="ico approve" aria-hidden="true">✓</span>
    <span class="lbl">{saving ? 'Approving…' : 'Approve'}</span>
  </button>
  <button
    type="button"
    class="gh-btn request"
    bind:this={requestBtnEl}
    onclick={() => (modalOpen = true)}
    disabled={saving || refreshing}
    title="Submit a REQUEST_CHANGES review with a required body"
    data-tour="gh-request-changes"
  >
    <span class="ico request" aria-hidden="true">✗</span>
    <span class="lbl">{saving ? 'Requesting…' : 'Request changes'}</span>
  </button>
  <button
    type="button"
    class="gh-btn refresh"
    onclick={() => refresh()}
    disabled={saving || refreshing}
    title="Re-import PR discussion + head SHA from github.com"
    aria-label="Refresh from GitHub"
    data-tour="gh-refresh"
  >
    <span class="ico" aria-hidden="true">↻</span>
    <span class="lbl">{refreshing ? 'Refreshing…' : 'Refresh'}</span>
  </button>
</div>

{#if modalOpen}
  <PublishBodyModal
    event="REQUEST_CHANGES"
    {saving}
    returnFocusTo={requestBtnEl ?? null}
    onsubmit={submitRequestChanges}
    onclose={() => (modalOpen = false)}
  />
{/if}

<style>
  .gh-actions {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding-left: 6px;
    margin-left: 2px;
    border-left: 1px solid var(--border-muted);
  }

  .gh-btn {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 3px 8px;
    font: inherit;
    font-size: 12px;
    background: transparent;
    color: inherit;
    border: 1px solid var(--border);
    border-radius: 4px;
    cursor: pointer;
  }
  .gh-btn:hover:not(:disabled) {
    background: var(--bg-elevated);
  }
  .gh-btn:disabled {
    opacity: 0.5;
    cursor: default;
  }

  .ico {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 14px;
    height: 14px;
    border-radius: 50%;
    color: white;
    font-size: 10px;
    font-weight: 700;
  }
  .ico.approve { background: #2da44e; }
  .ico.request { background: #cf222e; }
  /* Refresh keeps the neutral border-only shape — no coloured badge. */
  .gh-btn.refresh .ico {
    background: transparent;
    color: var(--text-muted);
    font-size: 14px;
    font-weight: 500;
  }

  /* On narrow widths, drop the button label and keep just the icon
   * so all three still fit on one row next to commit-nav. */
  @media (max-width: 900px) {
    .gh-btn .lbl {
      display: none;
    }
    .gh-btn {
      padding: 3px 6px;
    }
  }
</style>
