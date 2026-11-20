import { alignBlock, alignedRows, hunkWordDiff } from "./align.js";
import { highlights, javaTokens, newJavaState } from "./java.js";

export const LINE_HEIGHT = 20;
export const HEADER_HEIGHT = 34;
const HUNK_HEIGHT = 22;
const GAP_HEIGHT = 14;
const MARKER_HEIGHT = 16;
const TEXT_LINE = 17;
const CHAR_WIDTH = 7;
export const CARD_INDENT = 88;

const raw = (content) => (content ?? "").replace(/\r?\n$/, "");

// A line as drawable pieces: { text, kind (syntax, or null), changed (a word-diff highlight) }.
// Text draws a tab as nothing, so tabs become spaces to the next 4-column stop, counted across pieces.
function pieces(text, tokens, changed) {
  if (!text.length) return [{ text: " ", kind: null, changed: false }];
  const cuts = new Set([0, text.length]);
  for (const r of [...tokens, ...changed]) {
    cuts.add(r.start);
    cuts.add(r.end);
  }
  const points = [...cuts].filter((x) => x >= 0 && x <= text.length).sort((a, b) => a - b);
  const out = [];
  let column = 0;
  for (let k = 0; k + 1 < points.length; k++) {
    const [a, b] = [points[k], points[k + 1]];
    if (b <= a) continue;
    const kind = tokens.find((r) => r.start <= a && a < r.end)?.kind ?? null;
    const isChanged = changed.some((r) => r.start <= a && a < r.end);
    let piece = "";
    for (const ch of text.slice(a, b)) {
      if (ch === "\t") {
        const pad = 4 - (column % 4);
        piece += " ".repeat(pad);
        column += pad;
      } else {
        piece += ch;
        column += 1;
      }
    }
    const last = out[out.length - 1];
    if (last && last.kind === kind && last.changed === isChanged) last.text += piece;
    else out.push({ text: piece, kind, changed: isChanged });
  }
  return out;
}

// Per hunk, each line's pieces: syntax per side (base: removed and context lines, tip: added and context),
// word-diff ranges for paired changes. Computed once per fetched file diff.
const prepared = new WeakMap();
function prepare(change) {
  if (prepared.has(change)) return prepared.get(change);
  const java = highlights(change.path);
  const hunks = (change.hunks ?? []).map((hunk) => {
    if (hunk.kind !== "regular") return hunk;
    const words = hunkWordDiff(hunk.lines);
    const base = newJavaState();
    const tip = newJavaState();
    const lines = hunk.lines.map((line, index) => {
      const text = raw(line.content);
      let tokens = [];
      if (java) {
        if (line.origin === "context") {
          tokens = javaTokens(text, base);
          javaTokens(text, tip);
        } else {
          tokens = javaTokens(text, line.origin === "removed" ? base : tip);
        }
      }
      return { ...line, text, pieces: pieces(text, tokens, words.get(index) ?? []) };
    });
    return { ...hunk, lines };
  });
  prepared.set(change, hunks);
  return hunks;
}
const byCreated = (a, b) => (a.created_at < b.created_at ? -1 : a.created_at > b.created_at ? 1 : 0);

function wrappedLines(text, columns) {
  return text.split("\n").reduce((count, line) => count + Math.max(1, Math.ceil(line.length / columns)), 0);
}

// "open", "resolved" or "wont-fix": the last resolution action among the thread's responses.
export function resolution(thread) {
  let state = "open";
  for (const response of thread.responses) {
    if (response.action === "resolve") state = "resolved";
    else if (response.action === "wont-fix") state = "wont-fix";
    else if (response.action === "unresolve") state = "open";
  }
  return state;
}

// A thread card's drawn height, estimated from its text so the virtual list can place it.
function cardHeight(thread, width) {
  const columns = Math.max(30, Math.floor((width - CARD_INDENT - 40) / CHAR_WIDTH));
  const entries = [thread.comment, ...thread.responses];
  return entries.reduce((height, entry) => height + 22 + (entry.body ? wrappedLines(entry.body, columns) * TEXT_LINE : 0), 30);
}

// Where a comment draws in the diff shown, keyed "tip:<n>" / "base:<n>"; null when it can't be placed.
// The server re-anchors comments against the review's diff: valid keeps its lines, moved and drifted carry new
// ones, outdated has none. One commit's diff uses that commit's own line numbers, so there only comments written
// on that change are placed.
function placement(comment, endpoints) {
  if (!comment.lines || !comment.side || comment.anchor?.kind === "outdated") return null;
  const side = comment.side;
  const moved = comment.anchor?.kind === "moved" || comment.anchor?.kind === "drifted";
  if (endpoints.scoped) {
    const commit = side === "tip" ? endpoints.tip : endpoints.base;
    const change = side === "tip" ? endpoints.tipChange : endpoints.baseChange;
    if (comment.anchor_commit_id !== commit && comment.anchor_change_id !== change) return null;
  }
  return `${side}:${(moved ? comment.anchor.new_lines : comment.lines).end}`;
}

// The whole file as one hunk: the diff's own lines where it has hunks, the tip's text as context between them.
// Context base numbers follow the running offset of lines added minus removed so far.
export function expandChange(change, tipText) {
  const tip = tipText.replace(/\n$/, "").split("\n");
  const lines = [];
  let next = 1;
  let offset = 0;
  const context = (upTo) => {
    for (; next < upTo && next <= tip.length; next++) {
      lines.push({ origin: "context", base_line: next - offset, tip_line: next, content: tip[next - 1] });
    }
  };
  for (const hunk of change.hunks ?? []) {
    if (hunk.kind !== "regular") return change;
    const first = hunk.lines.find((l) => l.tip_line != null)?.tip_line ?? hunk.tip_range?.start ?? next;
    context(first);
    for (const line of hunk.lines) {
      lines.push(line);
      if (line.origin === "added") offset += 1;
      if (line.origin === "removed") offset -= 1;
      if (line.tip_line != null) next = line.tip_line + 1;
    }
  }
  context(tip.length + 1);
  return { ...change, hunks: [{ kind: "regular", base_range: { start: 1, end: 1 }, tip_range: { start: 1, end: tip.length + 1 }, lines }], expanded: true };
}

// Every thread of the review, grouped by file: { file: [...], elsewhere: [...], inline: Map } per path.
// endpoints: the { base, tip } commits of the diff shown, which decide what can be placed inline.
export function threadsByFile(view, endpoints) {
  const responses = new Map();
  const draftResponses = (view.drafts?.responses ?? []).map((r) => ({ ...r, draft: true }));
  for (const response of [...view.responses, ...draftResponses].sort(byCreated)) {
    const list = responses.get(response.in_reply_to) ?? [];
    list.push(response);
    responses.set(response.in_reply_to, list);
  }
  const comments = [...view.comments, ...(view.drafts?.comments ?? []).map((c) => ({ ...c, draft: true }))].sort(byCreated);
  const byFile = new Map();
  const all = [];
  for (const comment of comments) {
    if (!comment.file) continue;
    const thread = { comment, responses: responses.get(comment.comment_id) ?? [] };
    all.push(thread);
    if (!byFile.has(comment.file)) byFile.set(comment.file, { file: [], elsewhere: [], inline: new Map() });
    const group = byFile.get(comment.file);
    if (!comment.lines) {
      group.file.push(thread);
      continue;
    }
    const at = placement(comment, endpoints);
    if (!at) {
      group.elsewhere.push(thread);
      continue;
    }
    const list = group.inline.get(at) ?? [];
    list.push(thread);
    group.inline.set(at, list);
  }
  return { byFile, all };
}

// The whole review as one list of rows, files stacked in order. Rows hold no handlers: the list reports clicks by key.
// ui: { selection: {file, side, start, end} | null, thread: comment id | null, collapsed: Set of paths, unified: boolean,
//       openTops: Set of paths, shows(thread): boolean (the filters), mode: "both" | "diff" | "comments" }
export function reviewRows(files, changes, threads, width, ui) {
  const rows = [];
  for (const file of files) {
    const path = file.path;
    const all = threads.byFile.get(path) ?? { file: [], elsewhere: [], inline: new Map() };
    const keep = (list) => (ui.mode === "diff" ? [] : list.filter((t) => ui.shows(t) || ui.thread === t.comment.comment_id));
    const group = {
      file: keep(all.file),
      elsewhere: keep(all.elsewhere),
      inline: new Map([...all.inline].map(([at, list]) => [at, keep(list)]).filter(([, list]) => list.length)),
    };
    const commentsOnly = ui.mode === "comments";
    if (commentsOnly && !group.file.length && !group.elsewhere.length && !group.inline.size) continue;
    if (rows.length) rows.push({ key: `gap:${path}`, kind: "gap", file: path, height: GAP_HEIGHT });
    const collapsed = ui.collapsed.has(path);
    rows.push({ key: `file:${path}`, kind: "file", file: path, info: file, collapsed, height: HEADER_HEIGHT });
    if (collapsed) continue;
    const thread = (t, note, inline, under = null) => {
      const id = t.comment.comment_id;
      const resolved = resolution(t) !== "open";
      // A resolved thread stays a marker even while open in the drawer, so its history shows once.
      if (inline && resolved) {
        rows.push({ key: `marker:${id}`, kind: "marker", file: path, thread: t, selected: ui.thread === id, under, height: MARKER_HEIGHT });
      } else {
        rows.push({ key: `thread:${id}`, kind: "thread", file: path, thread: t, note, resolved, under, height: cardHeight(t, width) });
      }
    };
    // The comments above the diff fold to one line once every one of them is resolved.
    const top = [
      ...group.file.map((t) => [t, "File"]),
      ...group.elsewhere.map((t) => [t, `Line ${t.comment.lines.start}${t.comment.anchor?.kind === "outdated" ? " · outdated" : " · another revision"}`]),
    ];
    const allResolved = top.length > 0 && top.every(([t]) => resolution(t) !== "open");
    if (allResolved) {
      const open = ui.openTops?.has(path) ?? false;
      rows.push({ key: `fold:${path}`, kind: "fold", file: path, count: top.length, open, height: MARKER_HEIGHT + 4 });
      if (open) for (const [t] of top) thread(t, null, true);
    } else {
      for (const [t, note] of top) thread(t, note, true);
    }
    const change = changes.get(path);
    if (commentsOnly) {
      // Inline threads in line order, without the code around them.
      const at = [...group.inline.keys()].sort((a, b) => Number(a.split(":")[1]) - Number(b.split(":")[1]));
      for (const key of at) for (const t of group.inline.get(key)) thread(t, `Line ${t.comment.lines.start}${t.comment.side === "base" ? " (old)" : ""}`, false);
      continue;
    }
    if (!change) {
      rows.push({ key: `loading:${path}`, kind: "note", file: path, text: "Loading…", height: LINE_HEIGHT });
      continue;
    }
    if (change.error) {
      rows.push({ key: `error:${path}`, kind: "note", file: path, text: change.error, error: true, height: LINE_HEIGHT });
      continue;
    }
    if (change.binary) {
      rows.push({ key: `binary:${path}`, kind: "note", file: path, text: "Binary file", height: LINE_HEIGHT });
      continue;
    }
    const selection = ui.selection?.file === path ? ui.selection : null;
    const isSelected = (side, number) => Boolean(selection && number != null && selection.side === side && number >= selection.start && number <= selection.end);
    const threadsAt = (side, number) => (number == null ? [] : group.inline.get(`${side}:${number}`) ?? []);
    const split = !ui.unified && file.status !== "added" && file.status !== "deleted";
    for (const [hunkIndex, hunk] of prepare(change).entries()) {
      const at = `${path}:${hunkIndex}`;
      if (hunk.kind === "conflict") {
        rows.push({ key: `hunk:${at}`, kind: "hunk", file: path, text: "Conflict", height: HUNK_HEIGHT });
        for (const [termIndex, term] of hunk.terms.entries()) {
          rows.push({ key: `term:${at}:${termIndex}`, kind: "hunk", file: path, text: term.label, height: HUNK_HEIGHT });
          for (const [lineIndex, line] of (term.lines ?? []).entries()) {
            const text = raw(line.content);
            rows.push({ key: `cline:${at}:${termIndex}:${lineIndex}`, kind: "line", origin: line.origin ?? "context", file: path, pieces: pieces(text, [], []), height: LINE_HEIGHT });
          }
        }
        continue;
      }
      const range = (r) => (r ? `${r.start},${Math.max(0, r.end - r.start)}` : "0,0");
      rows.push({ key: `hunk:${at}`, kind: "hunk", file: path, text: `@@ -${range(hunk.base_range)} +${range(hunk.tip_range)} @@`, height: HUNK_HEIGHT });
      if (!split) {
        for (const [lineIndex, line] of hunk.lines.entries()) {
          const side = line.origin === "removed" ? "base" : "tip";
          rows.push({
            key: `line:${at}:${lineIndex}`, kind: "line", origin: line.origin, file: path, base: line.base_line, tip: line.tip_line,
            pieces: line.pieces, selected: isSelected(side, side === "base" ? line.base_line : line.tip_line), height: LINE_HEIGHT,
          });
          for (const t of [...threadsAt("tip", line.tip_line), ...(line.origin !== "added" ? threadsAt("base", line.base_line) : [])]) {
            thread(t, null, true, { origin: line.origin });
          }
        }
        continue;
      }
      // Side by side: context lines across from themselves, change blocks paired by content (alignBlock).
      const half = (line, side) => (line ? { origin: line.origin, number: side === "base" ? line.base_line : line.tip_line, pieces: line.pieces } : null);
      let index = 0;
      let k = 0;
      const lines = hunk.lines;
      const pushSplit = (left, right) => {
        const leftHalf = half(left, "base");
        const rightHalf = half(right, "tip");
        if (leftHalf) leftHalf.selected = isSelected("base", leftHalf.number);
        if (rightHalf) rightHalf.selected = isSelected("tip", rightHalf.number);
        rows.push({ key: `split:${at}:${k++}`, kind: "split", file: path, left: leftHalf, right: rightHalf, height: LINE_HEIGHT });
        const here = [...(rightHalf ? threadsAt("tip", rightHalf.number) : []), ...(leftHalf ? threadsAt("base", leftHalf.number) : [])];
        const under = { split: true, left: leftHalf?.origin ?? null, right: rightHalf?.origin ?? null };
        for (const t of new Set(here)) thread(t, null, true, under);
      };
      while (index < lines.length) {
        if (lines[index].origin === "context") {
          pushSplit(lines[index], lines[index]);
          index++;
          continue;
        }
        const removes = [];
        while (index < lines.length && lines[index].origin === "removed") removes.push(lines[index++]);
        const adds = [];
        while (index < lines.length && lines[index].origin === "added") adds.push(lines[index++]);
        for (const pair of alignedRows(alignBlock(removes.map((l) => l.text), adds.map((l) => l.text)))) {
          pushSplit(pair.removeIndex == null ? null : removes[pair.removeIndex], pair.addIndex == null ? null : adds[pair.addIndex]);
        }
      }
    }
  }
  return rows;
}
