// Ported from web/src/lib/hunkAlign.ts and wordDiff.ts: side-by-side pairing of a hunk's remove/add blocks by
// content similarity, and word-level ranges for paired lines. Keep the two in step when the web version changes.

const SIMILARITY_THRESHOLD = 0.3;

const isWordChar = (c) => (c >= 48 && c <= 57) || (c >= 65 && c <= 90) || (c >= 97 && c <= 122) || c === 95 || c > 127;

function tokenize(line) {
  const tokens = [];
  let i = 0;
  while (i < line.length) {
    if (isWordChar(line.charCodeAt(i))) {
      const start = i;
      while (i < line.length && isWordChar(line.charCodeAt(i))) i++;
      tokens.push({ text: line.slice(start, i), start, end: i });
    } else {
      tokens.push({ text: line[i], start: i, end: i + 1 });
      i++;
    }
  }
  return tokens;
}

function lcsLength(a, b) {
  const n = b.length;
  let prev = new Uint32Array(n + 1);
  let cur = new Uint32Array(n + 1);
  for (let i = 1; i <= a.length; i++) {
    for (let j = 1; j <= n; j++) {
      cur[j] = a[i - 1].text === b[j - 1].text ? prev[j - 1] + 1 : Math.max(prev[j], cur[j - 1]);
    }
    [prev, cur] = [cur, prev];
    cur.fill(0);
  }
  return prev[n];
}

// LCS over the longer token sequence, so a short new line can't outscore a heavily edited continuation.
function lineSimilarity(left, right) {
  if (left === right) return 1;
  if (!left.length || !right.length) return 0;
  const a = tokenize(left);
  const b = tokenize(right);
  if (!a.length || !b.length) return 0;
  return lcsLength(a, b) / Math.max(a.length, b.length);
}

// Needleman–Wunsch over (remove, add) similarity; sub-threshold cells can't pair.
export function alignBlock(removes, adds) {
  const m = removes.length;
  const n = adds.length;
  if (m === 0) return { pairs: [], unpairedRemoves: [], unpairedAdds: adds.map((_, i) => i) };
  if (n === 0) return { pairs: [], unpairedRemoves: removes.map((_, i) => i), unpairedAdds: [] };
  const sim = new Float64Array(m * n);
  for (let i = 0; i < m; i++) {
    for (let j = 0; j < n; j++) {
      const s = lineSimilarity(removes[i], adds[j]);
      sim[i * n + j] = s >= SIMILARITY_THRESHOLD ? s : -Infinity;
    }
  }
  const score = new Float64Array((m + 1) * (n + 1));
  const back = new Uint8Array((m + 1) * (n + 1));
  for (let i = 1; i <= m; i++) back[i * (n + 1)] = 1;
  for (let j = 1; j <= n; j++) back[j] = 2;
  for (let i = 1; i <= m; i++) {
    for (let j = 1; j <= n; j++) {
      const diag = score[(i - 1) * (n + 1) + (j - 1)] + sim[(i - 1) * n + (j - 1)];
      const up = score[(i - 1) * (n + 1) + j];
      const left = score[i * (n + 1) + (j - 1)];
      let best = diag;
      let choice = 0;
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
  const pairs = [];
  const unpairedRemoves = [];
  const unpairedAdds = [];
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
      break;
    }
  }
  pairs.reverse();
  unpairedRemoves.reverse();
  unpairedAdds.reverse();
  return { pairs, unpairedRemoves, unpairedAdds };
}

// The block as side-by-side rows ({ removeIndex, addIndex }, either may be null), keeping each side's order.
export function alignedRows(alignment) {
  const totalRemoves = alignment.pairs.length + alignment.unpairedRemoves.length;
  const totalAdds = alignment.pairs.length + alignment.unpairedAdds.length;
  const out = [];
  if (alignment.pairs.length === 0) {
    for (let k = 0; k < Math.max(totalRemoves, totalAdds); k++) {
      out.push({ removeIndex: k < totalRemoves ? k : null, addIndex: k < totalAdds ? k : null });
    }
    return out;
  }
  const removeToAdd = new Map(alignment.pairs.map((p) => [p.removeIndex, p.addIndex]));
  const unpairedAdds = new Set(alignment.unpairedAdds);
  const unpairedRemoves = new Set(alignment.unpairedRemoves);
  let addCursor = 0;
  for (let ri = 0; ri < totalRemoves; ri++) {
    const pairedAdd = removeToAdd.get(ri);
    // An unpaired remove must not advance the cursor, or adds belonging after a later pair would be emitted twice.
    const advanceTo = pairedAdd ?? addCursor;
    while (addCursor < advanceTo) {
      if (unpairedAdds.has(addCursor)) out.push({ removeIndex: null, addIndex: addCursor });
      addCursor++;
    }
    if (pairedAdd !== undefined) {
      out.push({ removeIndex: ri, addIndex: pairedAdd });
      addCursor = pairedAdd + 1;
    } else if (unpairedRemoves.has(ri)) {
      out.push({ removeIndex: ri, addIndex: null });
    }
  }
  while (addCursor < totalAdds) {
    if (unpairedAdds.has(addCursor)) out.push({ removeIndex: null, addIndex: addCursor });
    addCursor++;
  }
  return out;
}

function lcsTable(a, b) {
  const n = b.length;
  const dp = new Uint32Array((a.length + 1) * (n + 1));
  for (let i = 1; i <= a.length; i++) {
    for (let j = 1; j <= n; j++) {
      const idx = i * (n + 1) + j;
      if (a[i - 1].text === b[j - 1].text) dp[idx] = dp[(i - 1) * (n + 1) + (j - 1)] + 1;
      else dp[idx] = Math.max(dp[(i - 1) * (n + 1) + j], dp[i * (n + 1) + (j - 1)]);
    }
  }
  return dp;
}

function classify(a, b, dp) {
  const n = b.length;
  const aKind = new Array(a.length).fill("r");
  const bKind = new Array(n).fill("+");
  let i = a.length;
  let j = n;
  while (i > 0 && j > 0) {
    if (a[i - 1].text === b[j - 1].text) {
      aKind[i - 1] = "c";
      bKind[j - 1] = "c";
      i--;
      j--;
    } else if (dp[(i - 1) * (n + 1) + j] >= dp[i * (n + 1) + (j - 1)]) {
      i--;
    } else {
      j--;
    }
  }
  return { aKind, bKind };
}

// Whitespace never starts a highlight, but bridges two highlighted words into one.
function rangesFor(tokens, kinds, target) {
  const out = [];
  let cur = null;
  for (let i = 0; i < tokens.length; i++) {
    if (tokens[i].text.trim() === "") {
      if (cur === null) continue;
      let j = i + 1;
      while (j < tokens.length && tokens[j].text.trim() === "") j++;
      if (j < tokens.length && kinds[j] === target) {
        cur.end = tokens[i].end;
        continue;
      }
      out.push(cur);
      cur = null;
      continue;
    }
    if (kinds[i] !== target) {
      if (cur) {
        out.push(cur);
        cur = null;
      }
      continue;
    }
    if (cur && cur.end === tokens[i].start) cur.end = tokens[i].end;
    else {
      if (cur) out.push(cur);
      cur = { start: tokens[i].start, end: tokens[i].end };
    }
  }
  if (cur) out.push(cur);
  return out;
}

// Changed-word ranges for a paired line, or null when they'd be noise (little shared, or most of a long line).
function diffLines(removeLine, addLine) {
  if (!removeLine.length || !addLine.length) return null;
  if (removeLine === addLine) return { removed: [], added: [] };
  const a = tokenize(removeLine);
  const b = tokenize(addLine);
  if (!a.length || !b.length) return null;
  const dp = lcsTable(a, b);
  const lcs = dp[a.length * (b.length + 1) + b.length];
  if (lcs * 10 < Math.min(a.length, b.length) * 3) return null;
  const { aKind, bKind } = classify(a, b, dp);
  const removed = rangesFor(a, aKind, "r");
  const added = rangesFor(b, bKind, "+");
  const covered = (ranges) => ranges.reduce((s, r) => s + (r.end - r.start), 0);
  const noisy = (length, chars) => length > 30 && chars * 5 > length * 3;
  if (noisy(removeLine.length, covered(removed)) || noisy(addLine.length, covered(added))) return null;
  return { removed, added };
}

// Word-diff ranges per hunk-line index, for every paired remove/add in each change block.
export function hunkWordDiff(lines) {
  const out = new Map();
  let i = 0;
  while (i < lines.length) {
    if (lines[i].origin !== "removed") {
      i++;
      continue;
    }
    const removeStart = i;
    while (i < lines.length && lines[i].origin === "removed") i++;
    const addStart = i;
    while (i < lines.length && lines[i].origin === "added") i++;
    if (addStart === removeStart || i === addStart) continue;
    const text = (line) => line.content.replace(/\r?\n$/, "");
    const removes = lines.slice(removeStart, addStart).map(text);
    const adds = lines.slice(addStart, i).map(text);
    for (const { removeIndex, addIndex } of alignBlock(removes, adds).pairs) {
      const d = diffLines(removes[removeIndex], adds[addIndex]);
      if (!d) continue;
      if (d.removed.length) out.set(removeStart + removeIndex, d.removed);
      if (d.added.length) out.set(addStart + addIndex, d.added);
    }
  }
  return out;
}
