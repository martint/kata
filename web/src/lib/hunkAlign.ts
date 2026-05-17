//! Best-alignment pairing for a hunk's remove/add blocks.
//!
//! The naive side-by-side renderer pairs by index — 1st remove with
//! 1st add, 2nd with 2nd, etc. That degrades to noise as soon as the
//! block sizes diverge or the lines aren't in 1-to-1 correspondence:
//! a 3-removes-4-adds block where actually `r0↔a0`, `r1↔a2`, `r2↔a3`,
//! `a1` is a brand-new line, renders as four sequentially-paired rows
//! with the third pair visually nonsensical.
//!
//! This module pairs lines by content similarity via Needleman–Wunsch
//! over (remove, add) cells, then emits an ordered sequence of rows
//! where every pair sits on one row, and unpaired remove / unpaired
//! add rows leave the other side blank. The SBS renderer can then
//! lay out every paired-add directly across from its remove, with
//! gaps that make the alignment self-evident.
//!
//! Same alignment data drives `computeHunkWordDiff` so the inline
//! word-level highlight covers uneven blocks too — previously it
//! only ran for strict N:N.
//!
//! O(M*N) DP, M and N being the remove and add counts within a
//! block. Real-world hunks rarely have more than a couple dozen
//! either way, so the cost is trivial.

/** Public alignment result for one remove/add change block.
 *  `pairs` lists `(removeIndex, addIndex)` for every cell the DP
 *  picked as a paired row. `unpairedRemoves` and `unpairedAdds` list
 *  the indices that ended up alone on their side. All indices are
 *  positions WITHIN the block's own lists, not the hunk-line index. */
export interface BlockAlignment {
  pairs: Array<{ removeIndex: number; addIndex: number }>;
  unpairedRemoves: number[];
  unpairedAdds: number[];
}

/** A single laid-out row in the aligned output. Caller maps the
 *  indices back to its own HunkLine objects. Either index can be
 *  `null` for blank-on-that-side rows. */
export interface AlignedRow {
  removeIndex: number | null;
  addIndex: number | null;
}

/** Minimum LCS-over-shorter score for two lines to count as a viable
 *  pair. Below this the DP prefers two gaps (no pairing) over the
 *  forced match, which keeps unrelated lines from being yoked
 *  together. Tuned by eyeballing real Trino review diffs — 0.30 was
 *  the value the existing word-diff already used as a "is this
 *  meaningful?" cutoff (see diffLines in wordDiff.ts). */
const SIMILARITY_THRESHOLD = 0.3;

/** DP gap penalty. Set to 0 so any pair score above 0 wins over two
 *  gaps; the threshold above already excludes spurious pairings. */
const GAP_PENALTY = 0;

/** Compute the best alignment of a remove/add block. Both arrays are
 *  the block's lines in their original order; the result is positions
 *  within those arrays. */
export function alignBlock(
  removes: readonly string[],
  adds: readonly string[],
): BlockAlignment {
  const m = removes.length;
  const n = adds.length;
  // Degenerate cases: nothing to align.
  if (m === 0) {
    return {
      pairs: [],
      unpairedRemoves: [],
      unpairedAdds: adds.map((_, i) => i),
    };
  }
  if (n === 0) {
    return {
      pairs: [],
      unpairedRemoves: removes.map((_, i) => i),
      unpairedAdds: [],
    };
  }

  // Precompute the similarity matrix. Cell sim[i*n+j] = similarity
  // of removes[i] vs adds[j], in [0, 1]. Sub-threshold scores become
  // -Infinity so the DP refuses to pair them at all.
  const sim = new Float64Array(m * n);
  for (let i = 0; i < m; i++) {
    for (let j = 0; j < n; j++) {
      const s = lineSimilarity(removes[i], adds[j]);
      sim[i * n + j] = s >= SIMILARITY_THRESHOLD ? s : -Infinity;
    }
  }

  // Needleman–Wunsch table. score[i*(n+1)+j] = best total score for
  // aligning removes[0..i) with adds[0..j). Three transitions:
  //  - diagonal (pair removes[i-1] with adds[j-1])
  //  - up       (skip removes[i-1] — blank on the add side)
  //  - left     (skip adds[j-1]    — blank on the remove side)
  const score = new Float64Array((m + 1) * (n + 1));
  // `back` records which transition was chosen so we can walk the
  // path back. 0 = diagonal (pair), 1 = up (skip remove), 2 = left
  // (skip add).
  const back = new Uint8Array((m + 1) * (n + 1));
  for (let i = 1; i <= m; i++) {
    score[i * (n + 1)] = i * GAP_PENALTY;
    back[i * (n + 1)] = 1;
  }
  for (let j = 1; j <= n; j++) {
    score[j] = j * GAP_PENALTY;
    back[j] = 2;
  }
  for (let i = 1; i <= m; i++) {
    for (let j = 1; j <= n; j++) {
      const diag = score[(i - 1) * (n + 1) + (j - 1)] + sim[(i - 1) * n + (j - 1)];
      const up = score[(i - 1) * (n + 1) + j] + GAP_PENALTY;
      const left = score[i * (n + 1) + (j - 1)] + GAP_PENALTY;
      let best = diag;
      let choice: 0 | 1 | 2 = 0;
      if (up > best) {
        best = up;
        choice = 1;
      }
      if (left > best) {
        best = left;
        choice = 2;
      }
      score[i * (n + 1) + j] = best;
      back[i * (n + 1) + j] = choice;
    }
  }

  // Walk back from (m, n) to (0, 0), emitting alignment rows in
  // REVERSE; flip at the end. A cell whose back-pointer is "diagonal"
  // contributes a pair; "up" contributes an unpaired remove; "left"
  // contributes an unpaired add.
  const pairs: BlockAlignment['pairs'] = [];
  const unpairedRemoves: number[] = [];
  const unpairedAdds: number[] = [];
  let i = m;
  let j = n;
  while (i > 0 || j > 0) {
    if (i > 0 && j > 0 && back[i * (n + 1) + j] === 0) {
      pairs.push({ removeIndex: i - 1, addIndex: j - 1 });
      i--;
      j--;
    } else if (i > 0 && back[i * (n + 1) + j] === 1) {
      unpairedRemoves.push(i - 1);
      i--;
    } else if (j > 0) {
      unpairedAdds.push(j - 1);
      j--;
    } else {
      // Defensive — shouldn't happen if `back` is well-formed.
      break;
    }
  }
  pairs.reverse();
  unpairedRemoves.reverse();
  unpairedAdds.reverse();
  return { pairs, unpairedRemoves, unpairedAdds };
}

/** Lay out an aligned block as a sequence of side-by-side rows by
 *  merging the DP's pair / unpaired-remove / unpaired-add sets while
 *  preserving each side's original order. Each output row is either
 *  a pair (`removeIndex` + `addIndex`), a remove-only row
 *  (`addIndex: null`), or an add-only row (`removeIndex: null`). */
export function alignedRows(alignment: BlockAlignment): AlignedRow[] {
  const totalRemoves = alignment.pairs.length + alignment.unpairedRemoves.length;
  const totalAdds = alignment.pairs.length + alignment.unpairedAdds.length;

  // No pair survived the DP — the block is fully unrelated on both
  // sides. Fall back to index-zipping (the pre-alignment behavior)
  // so the output looks like two columns of comparable lines rather
  // than "all adds, then all removes." Reader can still tell nothing
  // genuinely pairs because there's no word-diff highlight, but the
  // shape is familiar and compact.
  if (alignment.pairs.length === 0) {
    const out: AlignedRow[] = [];
    const max = Math.max(totalRemoves, totalAdds);
    for (let k = 0; k < max; k++) {
      out.push({
        removeIndex: k < totalRemoves ? k : null,
        addIndex: k < totalAdds ? k : null,
      });
    }
    return out;
  }

  // At least one pair exists. Walk remove indices in order; for each
  // remove either emit its paired-add row or a remove-only row. Slot
  // unpaired adds in at the position implied by their original
  // index — before any remove that's paired to a later add, after
  // any remove paired to an earlier add. Preserves both sides'
  // original order in the common non-crossing case.
  const out: AlignedRow[] = [];
  const removeToAdd = new Map<number, number>();
  for (const { removeIndex, addIndex } of alignment.pairs) {
    removeToAdd.set(removeIndex, addIndex);
  }
  const unpairedAddSet = new Set(alignment.unpairedAdds);
  const unpairedRemoveSet = new Set(alignment.unpairedRemoves);
  let addCursor = 0;
  for (let ri = 0; ri < totalRemoves; ri++) {
    const pairedAdd = removeToAdd.get(ri);
    const advanceTo = pairedAdd ?? totalAdds;
    while (addCursor < advanceTo) {
      if (unpairedAddSet.has(addCursor)) {
        out.push({ removeIndex: null, addIndex: addCursor });
      }
      addCursor++;
    }
    if (pairedAdd !== undefined) {
      out.push({ removeIndex: ri, addIndex: pairedAdd });
      addCursor = pairedAdd + 1;
    } else if (unpairedRemoveSet.has(ri)) {
      out.push({ removeIndex: ri, addIndex: null });
    }
  }
  while (addCursor < totalAdds) {
    if (unpairedAddSet.has(addCursor)) {
      out.push({ removeIndex: null, addIndex: addCursor });
    }
    addCursor++;
  }
  return out;
}

/** Token-overlap similarity in [0, 1]: LCS length over the LONGER
 *  token sequence (equivalent to Jaccard's spirit). The pairing DP
 *  needs to penalize one-sided length so a brand-new short line
 *  doesn't outscore a real-but-heavily-edited continuation of the
 *  removed line. `LCS/shorter` (which `diffLines` uses for its
 *  internal "is this word-diff worth showing?" cutoff) is too
 *  lenient for pairing: it gives `LCS=1/shorter=1 = 1.0` to a one-
 *  token short line that happens to share that one token with the
 *  remove, beating a longer line that shares 5+ tokens but is also
 *  much longer. `LCS/longer` keeps the score honest about how much
 *  of EACH line's content is shared. */
export function lineSimilarity(left: string, right: string): number {
  if (left === right) return 1;
  if (left.length === 0 || right.length === 0) return 0;
  const a = tokenize(left);
  const b = tokenize(right);
  if (a.length === 0 || b.length === 0) return 0;
  const lcs = lcsLength(a, b);
  const longer = a.length > b.length ? a.length : b.length;
  return lcs / longer;
}

interface Token {
  text: string;
}

function tokenize(line: string): Token[] {
  // Same token shape as wordDiff.ts uses for its highlights — we
  // only need the text here (positions aren't relevant for
  // similarity scoring).
  const tokens: Token[] = [];
  let i = 0;
  while (i < line.length) {
    const ch = line.charCodeAt(i);
    const isWord =
      (ch >= 48 && ch <= 57) ||
      (ch >= 65 && ch <= 90) ||
      (ch >= 97 && ch <= 122) ||
      ch === 95 ||
      ch > 127;
    if (isWord) {
      const start = i;
      while (i < line.length) {
        const c = line.charCodeAt(i);
        const w =
          (c >= 48 && c <= 57) ||
          (c >= 65 && c <= 90) ||
          (c >= 97 && c <= 122) ||
          c === 95 ||
          c > 127;
        if (!w) break;
        i++;
      }
      tokens.push({ text: line.slice(start, i) });
    } else {
      tokens.push({ text: line[i] });
      i++;
    }
  }
  return tokens;
}

function lcsLength(a: readonly Token[], b: readonly Token[]): number {
  const m = a.length;
  const n = b.length;
  // Rolling two-row table — we only need lcs length, not the path.
  let prev = new Uint32Array(n + 1);
  let cur = new Uint32Array(n + 1);
  for (let i = 1; i <= m; i++) {
    for (let j = 1; j <= n; j++) {
      if (a[i - 1].text === b[j - 1].text) {
        cur[j] = prev[j - 1] + 1;
      } else {
        cur[j] = Math.max(prev[j], cur[j - 1]);
      }
    }
    [prev, cur] = [cur, prev];
    cur.fill(0);
  }
  return prev[n];
}
