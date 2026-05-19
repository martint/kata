//! Types shared between the demo tour script and the overlay
//! engine. The script is plain TypeScript (`script.ts`); the
//! overlay (`DemoOverlay.svelte`) iterates over it.

/** Where to place the narration bubble relative to the target. */
export type Placement = 'top' | 'bottom' | 'left' | 'right' | 'center';

/** One step in the guided tour. The script is an ordered array of
 *  these. Adding a new feature to the tour = adding a step here
 *  and tagging the relevant DOM node with `data-tour="<name>"`. */
export interface TourStep {
  /** Stable identifier — used for resume, deep-linking, telemetry.
   *  Must be unique across the script. */
  id: string;
  /** URL the overlay routes to before showing the step. Omit when
   *  the step continues on the same page as the previous one. */
  page?: string;
  /** CSS selector for the element to highlight. Convention:
   *  `[data-tour=feature-name]` — use the dedicated attribute so
   *  the tour doesn't break when component class names are
   *  refactored. Omit for centered narration cards. */
  target?: string;
  /** Bubble placement relative to `target`. Defaults to `bottom`
   *  when a target is present, `center` otherwise. */
  placement?: Placement;
  /** Short heading shown in the bubble's header. */
  title?: string;
  /** Body text. Rendered as plain text — no markdown for now;
   *  keep prose tight, link out via the docs site if needed. */
  body: string;
  /** When set, the overlay waits for this element to appear in the
   *  DOM (handles route transitions that mount components async).
   *  Defaults to `target`. */
  waitFor?: string;
  /** Pre-step DOM nudges. Run after routing + `waitFor` but before
   *  the spotlight is measured. The intended use is making sure
   *  the step's target is actually visible — e.g. the file-tree
   *  step clicks `.panel-toggle.collapsed` so a previously-hidden
   *  tree pane is open by the time we point at it. The selector
   *  is a no-op when nothing matches, so it's safe to leave
   *  defensive clicks in for the case the user already opened the
   *  panel themselves. */
  setup?: {
    /** Click each matching element, in document order. */
    click?: string;
  };
}
