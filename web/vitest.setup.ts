//! Vitest setup file. Loaded before any test runs. Pulls in the
//! `expect(...).toBeInTheDocument()` and friends matchers, and
//! installs the DOM cleanup that runs between tests — without it,
//! `@testing-library/svelte`'s rendered components linger in the
//! document and the next test's `screen.queryByText(...)` can find
//! elements from a previous test's render.

import '@testing-library/jest-dom/vitest';
import { afterEach, beforeEach } from 'vitest';
import { cleanup } from '@testing-library/svelte';

afterEach(() => {
  cleanup();
});

// Reset localStorage between tests so a toggle-flipped persisted
// preference from one test doesn't bleed into the next (e.g. the
// compare-mode "hide unchanged" toggle in CommitsPanel writes to
// `kata:compare:hide-same`).
beforeEach(() => {
  if (typeof localStorage !== 'undefined') {
    localStorage.clear();
  }
});

// jsdom doesn't ship `ResizeObserver` or `IntersectionObserver`, but a
// number of components in this codebase wire up observers in
// `$effect` (sticky-thread sizing, viewport-visibility detection,
// composer height tracking). Provide no-op stubs so component tests
// can mount without those effects throwing.
class ObserverStub {
  observe() {}
  unobserve() {}
  disconnect() {}
  takeRecords() {
    return [];
  }
}
if (typeof (globalThis as unknown as { ResizeObserver?: unknown }).ResizeObserver === 'undefined') {
  (globalThis as unknown as { ResizeObserver: typeof ObserverStub }).ResizeObserver = ObserverStub;
}
if (
  typeof (globalThis as unknown as { IntersectionObserver?: unknown }).IntersectionObserver ===
  'undefined'
) {
  (globalThis as unknown as { IntersectionObserver: typeof ObserverStub }).IntersectionObserver =
    ObserverStub;
}
if (typeof (globalThis as unknown as { MutationObserver?: unknown }).MutationObserver === 'undefined') {
  (globalThis as unknown as { MutationObserver: typeof ObserverStub }).MutationObserver =
    ObserverStub;
}

// jsdom doesn't implement `window.matchMedia` either. Several
// components query it to detect a narrow viewport for the
// side-by-side ↔ unified diff switch. Return a permanent-non-match
// stub with a listener no-op shape; component code that registers
// `addEventListener('change', ...)` then unregisters gets a harmless
// pair of calls.
if (typeof window !== 'undefined' && typeof window.matchMedia === 'undefined') {
  (window as unknown as { matchMedia: typeof window.matchMedia }).matchMedia = (
    query: string,
  ): MediaQueryList =>
    ({
      matches: false,
      media: query,
      onchange: null,
      addListener: () => {},
      removeListener: () => {},
      addEventListener: () => {},
      removeEventListener: () => {},
      dispatchEvent: () => false,
    }) as MediaQueryList;
}
