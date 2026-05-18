//! Per-review persistence of fold/expand state across page reloads.
//!
//! The UI has several "folded" surfaces — collapsed files, collapsed
//! comment threads, collapsed commit-message bodies — and previously
//! every one of them re-defaulted on each page load (and on each
//! re-mount, in the case of virtualised file slots). That's a
//! frustrating reset every time the user navigates away or hits
//! refresh.
//!
//! This module owns one JSON blob in `localStorage` per review and
//! exposes a small `get`/`set`/`prune` API. Components hydrate from
//! the store on mount and write back through a `$effect`. Each
//! component still owns its own reactive `$state`; the store is just
//! persistence.
//!
//! Scope: keyed by `(repo, review_number)` — **not** patchset. Folding
//! is treated as the reviewer's intent, not a per-patchset annotation.
//! A file the user has collapsed because they've already reviewed it
//! should stay collapsed across patchsets; a comment thread the user
//! has unfolded should stay unfolded if the next patchset reuses the
//! same comment id.

/** What kind of thing is being folded. The store namespaces by kind so
 *  prune calls can scope to "files that still exist" without touching
 *  comment-thread state, etc.
 *
 *  - `file` — whole-file collapse on `FileSlot`.
 *  - `comment` — per-comment "I unfolded this resolved comment"
 *    overrides inside `CommentThread`.
 *  - `commit` — per-commit body expansion in `CommitsPanel`.
 *  - `thread` — per-anchor (file/side/line) thread fold. Drives the
 *    "diffs view" collapse-by-default + per-line markers in
 *    HunkLines / HunkLinesSideBySide. Keys are `${file}:${side}:${line}`. */
export type FoldKind = 'file' | 'comment' | 'commit' | 'thread';

/** Per-review fold store. */
export interface FoldStore {
  /** Look up an explicit value. Returns `undefined` for entries the
   *  user has never touched — the caller falls back to its own default. */
  get(kind: FoldKind, id: string): boolean | undefined;
  /** Record an explicit value. */
  set(kind: FoldKind, id: string, value: boolean): void;
  /** Every id the user has set explicitly under this kind. Useful for
   *  hydrating a "set of unfolded ids" by iterating then filtering on
   *  the value. */
  ids(kind: FoldKind): string[];
  /** Drop persisted entries whose id is not in `keep`. Lets the caller
   *  garbage-collect after a file rename / comment deletion so the
   *  blob doesn't grow forever. No-op if the kind has no entries. */
  prune(kind: FoldKind, keep: Iterable<string>): void;
}

/** Construct a store bound to one `(repo, number)` pair. Safe to call
 *  in SSR contexts — falls back to an in-memory map when
 *  `localStorage` is unavailable so callers can write without
 *  branching. */
export function createFoldStore(repo: string, number: number): FoldStore {
  const key = `kata:fold:${repo}:${number}`;
  type Blob = { [K in FoldKind]?: Record<string, boolean> };

  // Read once on construction; subsequent gets hit the in-memory copy.
  // A bad/missing/corrupt entry just yields a fresh blob — folding is
  // pure UX state, no need to surface parse failures.
  let data: Blob = {};
  if (typeof localStorage !== 'undefined') {
    try {
      const raw = localStorage.getItem(key);
      if (raw) {
        const parsed = JSON.parse(raw);
        if (parsed && typeof parsed === 'object') data = parsed;
      }
    } catch {
      data = {};
    }
  }

  function bucket(kind: FoldKind): Record<string, boolean> {
    let b = data[kind];
    if (!b) {
      b = {};
      data[kind] = b;
    }
    return b;
  }

  function flush() {
    if (typeof localStorage === 'undefined') return;
    try {
      localStorage.setItem(key, JSON.stringify(data));
    } catch {
      // Quota or private-mode storage failures are non-fatal — fold
      // state just stops persisting until the user clears space.
    }
  }

  return {
    get(kind, id) {
      const v = data[kind]?.[id];
      return typeof v === 'boolean' ? v : undefined;
    },
    set(kind, id, value) {
      const b = bucket(kind);
      if (b[id] === value) return; // no-op write — skip serialisation
      b[id] = value;
      flush();
    },
    ids(kind) {
      const b = data[kind];
      return b ? Object.keys(b) : [];
    },
    prune(kind, keep) {
      const b = data[kind];
      if (!b) return;
      const allowed = new Set(keep);
      let changed = false;
      for (const id of Object.keys(b)) {
        if (!allowed.has(id)) {
          delete b[id];
          changed = true;
        }
      }
      if (changed) flush();
    },
  };
}
