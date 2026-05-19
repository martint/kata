<script lang="ts">
  //! The guided-tour overlay. Activated by `?demo=1` in the URL.
  //! Renders a fixed-position bubble with narration + Next/Back/
  //! Skip controls, and an SVG spotlight that dims everything
  //! except the current step's target element. Step state persists
  //! in `localStorage` so a page reload (or a navigation the step
  //! itself triggers via `page`) resumes where the user was.
  //!
  //! The overlay is intentionally read-only: no UI input is
  //! synthesised. The user advances the script manually; the
  //! script just narrates. Interactive steps (where the script
  //! waits for the user to do something specific) are a follow-up.

  import { onDestroy, onMount, tick } from 'svelte';
  import { tour } from './script';
  import type { Placement, TourStep } from './types';

  const STORAGE_KEY = 'kata:demo:step';

  // The whole overlay is opt-in: we don't mount any DOM, listen
  // for events, or run timers unless the URL says so. App.svelte
  // wraps this in `{#if showDemo}` so the conditional is cheap
  // when the user isn't in demo mode.
  let stepIndex = $state(0);
  /** Spotlight rectangle in viewport coords, or null for
   *  center-placement steps (no target). */
  let spotlight = $state<DOMRect | null>(null);
  /** Bubble position. Computed from the spotlight + placement, or
   *  centered when there's no spotlight. */
  let bubblePos = $state<{ top: number; left: number; centered: boolean }>({
    top: 0,
    left: 0,
    centered: true,
  });
  /** Reference to the rendered bubble so we can read its true
   *  size for placement math. The hardcoded 360x180 we used
   *  before clipped long-body steps and overshot short-body
   *  ones; measuring is the only way to get this right. */
  let bubbleEl: HTMLDivElement | null = $state(null);

  const step = $derived<TourStep>(tour[stepIndex] ?? tour[0]);

  onMount(() => {
    const stored = Number(localStorage.getItem(STORAGE_KEY) ?? '0');
    if (Number.isFinite(stored) && stored >= 0 && stored < tour.length) {
      stepIndex = stored;
    }
    void showStep(step);
    window.addEventListener('keydown', onKeydown);
    window.addEventListener('resize', repositionForCurrent);
    window.addEventListener('scroll', repositionForCurrent, true);
  });
  onDestroy(() => {
    window.removeEventListener('keydown', onKeydown);
    window.removeEventListener('resize', repositionForCurrent);
    window.removeEventListener('scroll', repositionForCurrent, true);
  });

  // Re-run when stepIndex changes (Next/Back).
  $effect(() => {
    localStorage.setItem(STORAGE_KEY, String(stepIndex));
    void showStep(step);
  });

  async function showStep(s: TourStep) {
    // Route first, then wait for the target to mount. The wait is
    // capped so a bad selector doesn't hang the overlay.
    if (s.page && window.location.pathname !== s.page) {
      window.history.pushState(null, '', s.page);
      window.dispatchEvent(new PopStateEvent('popstate'));
    }
    if (s.target || s.waitFor) {
      const sel = s.waitFor ?? s.target!;
      const el = await waitForElement(sel, 2000);
      // Step-declared setup runs after the target exists but before
      // we measure — typically a click that uncollapses the panel
      // the target lives inside. A missing selector is a no-op so
      // setup is safe to leave armed for users who already have
      // the panel open.
      if (s.setup?.click) {
        for (const btn of document.querySelectorAll<HTMLElement>(s.setup.click)) {
          btn.click();
        }
        // Let the click's reactive updates flush before we measure.
        await tick();
      }
      if (el) {
        // Scroll the spotlight target into view before measuring.
        // Without this a step that targets something below the fold
        // (a comment thread deep in the diff, an annotation past the
        // viewport) spotlights an offscreen rect and the bubble has
        // nothing to anchor against. `block: center` keeps the
        // element well clear of either the sticky top header or the
        // bubble itself.
        // Instant scroll (rather than smooth) so the bubble settles
        // immediately on the new spotlight position — a smooth
        // animation here would visibly drag the bubble across the
        // screen as the page caught up, which reads as jank during
        // a guided tour.
        (el as HTMLElement).scrollIntoView({ block: 'center', behavior: 'instant' });
        // One frame for layout to commit the new scroll position
        // before we read the bounding rect.
        await new Promise((r) => requestAnimationFrame(() => r(null)));
      }
      spotlight = el?.getBoundingClientRect() ?? null;
    } else {
      spotlight = null;
    }
    // Bubble body changed → wait one tick so the rendered size
    // reflects the new step before we measure.
    await tick();
    positionBubble(s);
  }

  function repositionForCurrent() {
    if (!step.target) return;
    const el = document.querySelector(step.target);
    spotlight = el ? (el as HTMLElement).getBoundingClientRect() : null;
    positionBubble(step);
  }

  /** Whether two rects overlap (axis-aligned, viewport coords). */
  function overlaps(
    a: { top: number; left: number; width: number; height: number },
    b: { top: number; left: number; width: number; height: number },
  ): boolean {
    return (
      a.left < b.left + b.width &&
      a.left + a.width > b.left &&
      a.top < b.top + b.height &&
      a.top + a.height > b.top
    );
  }

  /** Try a single placement and return the bubble's computed rect
   *  in viewport coords (top/left + size). Returns `null` when the
   *  placement doesn't fit (clamped result would overlap the
   *  spotlight) so the caller can flip to the next candidate. */
  function tryPlacement(
    placement: Exclude<Placement, 'center'>,
    spot: DOMRect,
    bubbleW: number,
    bubbleH: number,
    gap: number,
    margin: number,
    vpW: number,
    vpH: number,
  ): { top: number; left: number } | null {
    let top: number;
    let left: number;
    switch (placement) {
      case 'top':
        top = spot.top - bubbleH - gap;
        left = spot.left + spot.width / 2 - bubbleW / 2;
        break;
      case 'bottom':
        top = spot.bottom + gap;
        left = spot.left + spot.width / 2 - bubbleW / 2;
        break;
      case 'left':
        top = spot.top + spot.height / 2 - bubbleH / 2;
        left = spot.left - bubbleW - gap;
        break;
      case 'right':
        top = spot.top + spot.height / 2 - bubbleH / 2;
        left = spot.right + gap;
        break;
    }
    // Clamp inside the viewport on the CROSS axis only — clamping
    // along the main axis would slide the bubble back onto the
    // spotlight, which is what we're avoiding. If the main-axis
    // bubble simply doesn't fit, the caller flips placement.
    if (placement === 'top' || placement === 'bottom') {
      left = Math.max(margin, Math.min(left, vpW - bubbleW - margin));
      if (placement === 'top' && top < margin) return null;
      if (placement === 'bottom' && top + bubbleH > vpH - margin) return null;
    } else {
      top = Math.max(margin, Math.min(top, vpH - bubbleH - margin));
      if (placement === 'left' && left < margin) return null;
      if (placement === 'right' && left + bubbleW > vpW - margin) return null;
    }
    // Final guard: even if the math claims the bubble fits, a
    // very wide/tall spotlight can still overlap. Reject and let
    // the next placement try.
    const rect = { top, left, width: bubbleW, height: bubbleH };
    if (overlaps(rect, spot)) return null;
    return { top, left };
  }

  /** Opposite-side fallback order — same family Popper / Tippy use.
   *  Try the requested side first, then its opposite, then the
   *  cross-axis sides. */
  function placementCandidates(
    requested: Exclude<Placement, 'center'>,
  ): Exclude<Placement, 'center'>[] {
    switch (requested) {
      case 'bottom': return ['bottom', 'top', 'right', 'left'];
      case 'top': return ['top', 'bottom', 'right', 'left'];
      case 'right': return ['right', 'left', 'bottom', 'top'];
      case 'left': return ['left', 'right', 'bottom', 'top'];
    }
  }

  function positionBubble(s: TourStep) {
    if (!spotlight) {
      bubblePos = { top: 0, left: 0, centered: true };
      return;
    }
    if (s.placement === 'center') {
      bubblePos = { top: 0, left: 0, centered: true };
      return;
    }
    const gap = 14;
    const margin = 8;
    // Measure the rendered bubble. Falls back to a conservative
    // estimate on the first pass (before the element has been
    // bound) — the subsequent tick + reposition gets the real
    // size.
    const bubbleW = bubbleEl?.offsetWidth ?? 360;
    const bubbleH = bubbleEl?.offsetHeight ?? 140;
    const vpW = window.innerWidth;
    const vpH = window.innerHeight;
    const requested = (s.placement ?? 'bottom') as Exclude<Placement, 'center'>;
    for (const candidate of placementCandidates(requested)) {
      const result = tryPlacement(
        candidate,
        spotlight,
        bubbleW,
        bubbleH,
        gap,
        margin,
        vpW,
        vpH,
      );
      if (result) {
        bubblePos = { top: result.top, left: result.left, centered: false };
        return;
      }
    }
    // Nothing fits without overlapping or overflowing. Center the
    // bubble — better than wedging it on top of the spotlight.
    bubblePos = { top: 0, left: 0, centered: true };
  }

  function waitForElement(selector: string, timeoutMs: number): Promise<Element | null> {
    return new Promise((resolve) => {
      const found = document.querySelector(selector);
      if (found) {
        resolve(found);
        return;
      }
      const obs = new MutationObserver(() => {
        const el = document.querySelector(selector);
        if (el) {
          obs.disconnect();
          resolve(el);
        }
      });
      obs.observe(document.body, { childList: true, subtree: true });
      // Cap the wait so a typo'd selector doesn't freeze the tour.
      setTimeout(() => {
        obs.disconnect();
        resolve(null);
      }, timeoutMs);
    });
  }

  function next() {
    if (stepIndex < tour.length - 1) stepIndex += 1;
  }
  function back() {
    if (stepIndex > 0) stepIndex -= 1;
  }
  function skip() {
    localStorage.removeItem(STORAGE_KEY);
    // Drop the latched demo-active flag and clean the URL. App.
    // svelte's `updateDemoGate` reads the localStorage flag, so
    // clearing it is what actually unmounts the overlay; the URL
    // strip is cosmetic but keeps reloads from auto-arming the
    // tour again.
    localStorage.removeItem('kata:demo:active');
    const url = new URL(window.location.href);
    url.searchParams.delete('demo');
    window.history.replaceState(null, '', url.toString());
    window.dispatchEvent(new PopStateEvent('popstate'));
  }

  function onKeydown(e: KeyboardEvent) {
    // Don't grab keys while the user is typing in the composer or
    // any other input — Enter / Space have meaning there.
    const t = e.target as HTMLElement | null;
    if (t && (t.tagName === 'TEXTAREA' || t.tagName === 'INPUT')) return;
    if (e.key === 'ArrowRight') {
      e.preventDefault();
      next();
    } else if (e.key === 'ArrowLeft') {
      e.preventDefault();
      back();
    } else if (e.key === 'Escape') {
      e.preventDefault();
      skip();
    }
  }
</script>

<!-- SVG mask gives us a "dim everything except the spotlight" effect
     without painting four edge rectangles ourselves. `pointer-events:
     none` on the dim layer lets the user keep clicking through the
     UI (the tour is read-only — the highlight is a hint, not a
     guard). -->
{#if spotlight}
  <svg class="demo-spotlight" aria-hidden="true">
    <defs>
      <mask id="demo-spot-mask">
        <rect x="0" y="0" width="100%" height="100%" fill="white" />
        <rect
          x={spotlight.left - 6}
          y={spotlight.top - 6}
          width={spotlight.width + 12}
          height={spotlight.height + 12}
          rx="6"
          fill="black"
        />
      </mask>
    </defs>
    <rect
      x="0"
      y="0"
      width="100%"
      height="100%"
      fill="rgba(0, 0, 0, 0.45)"
      mask="url(#demo-spot-mask)"
    />
    <rect
      x={spotlight.left - 6}
      y={spotlight.top - 6}
      width={spotlight.width + 12}
      height={spotlight.height + 12}
      rx="6"
      fill="none"
      stroke="rgba(120, 180, 255, 0.9)"
      stroke-width="2"
    />
  </svg>
{:else}
  <!-- No target: dim the whole screen so the centered bubble has
       breathing room. -->
  <div class="demo-spotlight demo-spotlight-full" aria-hidden="true"></div>
{/if}

<div
  bind:this={bubbleEl}
  class="demo-bubble"
  class:centered={bubblePos.centered}
  style:top={bubblePos.centered ? undefined : `${bubblePos.top}px`}
  style:left={bubblePos.centered ? undefined : `${bubblePos.left}px`}
  role="dialog"
  aria-label="Demo tour"
>
  <header class="demo-bubble-head">
    <span class="demo-step-count">Step {stepIndex + 1} of {tour.length}</span>
    {#if step.title}
      <h2>{step.title}</h2>
    {/if}
  </header>
  <p class="demo-body">{step.body}</p>
  <footer class="demo-bubble-actions">
    <button type="button" class="demo-skip" onclick={skip}>Skip tour</button>
    <span class="demo-spacer"></span>
    <button
      type="button"
      class="demo-back"
      onclick={back}
      disabled={stepIndex === 0}
    >Back</button>
    {#if stepIndex < tour.length - 1}
      <button type="button" class="demo-next primary" onclick={next}>Next</button>
    {:else}
      <button type="button" class="demo-next primary" onclick={skip}>Done</button>
    {/if}
  </footer>
</div>

<style>
  .demo-spotlight {
    position: fixed;
    top: 0;
    left: 0;
    width: 100vw;
    height: 100vh;
    pointer-events: none;
    z-index: 9000;
  }
  .demo-spotlight-full {
    background: rgba(0, 0, 0, 0.45);
  }
  .demo-bubble {
    position: fixed;
    width: 360px;
    background: var(--bg-panel, white);
    color: var(--text, black);
    border: 1px solid var(--border, #d0d7de);
    border-radius: 8px;
    box-shadow: 0 6px 24px rgba(0, 0, 0, 0.18);
    padding: 14px 16px 12px;
    z-index: 9100;
    font-family: ui-sans-serif, system-ui, sans-serif;
    font-size: 13px;
    line-height: 1.45;
  }
  .demo-bubble.centered {
    /* Fallback when there's no target. Center on screen. */
    top: 50%;
    left: 50%;
    transform: translate(-50%, -50%);
  }
  .demo-bubble-head h2 {
    margin: 4px 0 6px;
    font-size: 15px;
    font-weight: 600;
  }
  .demo-step-count {
    font-size: 11px;
    color: var(--text-faint, #57606a);
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }
  .demo-body {
    margin: 0 0 12px;
  }
  .demo-bubble-actions {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .demo-spacer {
    flex: 1;
  }
  .demo-bubble-actions button {
    font: inherit;
    padding: 4px 10px;
    border: 1px solid var(--border, #d0d7de);
    background: transparent;
    border-radius: 4px;
    cursor: pointer;
    color: inherit;
  }
  .demo-bubble-actions button:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  .demo-bubble-actions .primary {
    background: var(--link, #0969da);
    color: var(--on-accent, white);
    border-color: var(--link, #0969da);
  }
  .demo-skip {
    color: var(--text-faint, #57606a) !important;
  }
</style>
