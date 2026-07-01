/**
 * Svelte `use:` action that reparents a node onto `document.body`
 * so it escapes its natural stacking context — critical for
 * popovers, menus, and modals that sit inside a sticky header or
 * transformed ancestor. Reverses on destroy.
 *
 * Every component that opens a fixed-position overlay used to
 * re-declare the same 6-line closure; a single shared action is
 * kinder to grep and keeps the "escape stacking context" note in
 * exactly one place.
 */
export function portal(node: HTMLElement) {
  document.body.appendChild(node);
  return {
    destroy() {
      node.parentNode?.removeChild(node);
    },
  };
}
