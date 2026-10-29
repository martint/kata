//! Which moved block the pointer is on, shared across every file of a
//! diff so hovering one end of a move lights up the other end too.

export interface MovedHover {
  /** `moveKey` of the hovered block, or null. */
  readonly key: string | null;
  set(key: string | null): void;
}

export function createMovedHover(): MovedHover {
  let key = $state<string | null>(null);
  return {
    get key() {
      return key;
    },
    set(next) {
      key = next;
    },
  };
}
