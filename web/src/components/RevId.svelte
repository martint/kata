<!--
  One short, coloured, browser-linked revision id.

  Two `kind`s, mirroring jj's CLI conventions:
    - `commit` — pinned to one revision. Renders blue.
      Links to `/r/<repo>/browse?commit=<full id>`.
    - `change` — follows the change across rewrites. Renders
      purple. Links to `/r/<repo>/browse?change=<full id>`; the
      browser resolves change_id → current commit_id on load.

  The component is the single source of truth for *how* an id
  is shown and *where* clicking lands the user. Drop it in
  anywhere an id appears.

  SPA-intercepting link: a normal left-click pushes history and
  syncs the SPA without a full reload; Cmd/Ctrl/middle-click
  lets the browser handle it as usual (open in new tab).
-->
<script lang="ts">
  let {
    id,
    kind = 'commit',
    repo,
    length = 12,
    /** When true, render as a non-clickable coloured span — for
     *  ids embedded inside another clickable element (e.g. a
     *  dropdown option) where a nested anchor would be invalid
     *  and surprising. */
    inline = false,
    /** Override the visible label. Defaults to the id sliced
     *  to `length`. Useful when the call site already has the
     *  short form in hand. */
    label,
  }: {
    id: string;
    kind?: 'commit' | 'change';
    repo: string;
    length?: number;
    inline?: boolean;
    label?: string;
  } = $props();

  const visible = $derived(label ?? (id.length > length ? id.slice(0, length) : id));
  const param = $derived(kind === 'change' ? 'change' : 'commit');
  const href = $derived(
    `/r/${encodeURIComponent(repo)}/browse?${param}=${encodeURIComponent(id)}`,
  );
  const title = $derived(`${kind} id: ${id}`);

  function navigate(e: MouseEvent) {
    // Honour modifier-clicks so users can still open in a new
    // tab / window the way they'd expect from any link.
    if (e.metaKey || e.ctrlKey || e.shiftKey || e.altKey || e.button !== 0) {
      return;
    }
    e.preventDefault();
    history.pushState({}, '', href);
    // popstate is what App.svelte already listens for to sync
    // from the URL. Dispatching one reuses that plumbing
    // without exposing a new event channel.
    window.dispatchEvent(new PopStateEvent('popstate'));
  }
</script>

{#if inline}
  <span class="rev-id {kind}" {title}>{visible}</span>
{:else}
  <a
    class="rev-id {kind}"
    {href}
    {title}
    onclick={navigate}
  >{visible}</a>
{/if}

<style>
  .rev-id {
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 11px;
    padding: 1px 5px;
    border-radius: 3px;
    text-decoration: none;
    white-space: nowrap;
  }

  a.rev-id:hover {
    text-decoration: underline;
  }

  .rev-id.commit {
    color: var(--commit-id-color);
    background: var(--commit-id-bg);
  }

  .rev-id.change {
    color: var(--change-id-color);
    background: var(--change-id-bg);
  }
</style>
