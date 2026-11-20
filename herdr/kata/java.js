// A small Java tokenizer for diff lines: Herdr exposes no highlighter, and its editor element can't draw a diff.
// state carries block comments and text blocks from one line to the next on the same side of a hunk.

const KEYWORDS = new Set(
  ("abstract assert boolean break byte case catch char class const continue default do double else enum extends final " +
    "finally float for goto if implements import instanceof int interface long native new package private protected " +
    "public return short static strictfp super switch synchronized this throw throws transient try void volatile while " +
    "var record sealed permits yield non-sealed").split(" "),
);
const LITERALS = new Set(["true", "false", "null"]);

const isIdentStart = (c) => /[A-Za-z_$]/.test(c);
const isIdent = (c) => /[A-Za-z0-9_$]/.test(c);

export const newJavaState = () => ({ comment: false, textBlock: false });

// [{ start, end, kind }] for the highlighted parts of one line; kinds: keyword, literal, string, comment,
// number, annotation, type, constant, function. Unlisted characters are plain.
export function javaTokens(line, state) {
  const out = [];
  const push = (start, end, kind) => {
    if (end > start) out.push({ start, end, kind });
  };
  let i = 0;
  while (i < line.length) {
    if (state.comment) {
      const close = line.indexOf("*/", i);
      const end = close < 0 ? line.length : close + 2;
      push(i, end, "comment");
      if (close >= 0) state.comment = false;
      i = end;
      continue;
    }
    if (state.textBlock) {
      const close = line.indexOf('"""', i);
      const end = close < 0 ? line.length : close + 3;
      push(i, end, "string");
      if (close >= 0) state.textBlock = false;
      i = end;
      continue;
    }
    const c = line[i];
    if (c === "/" && line[i + 1] === "/") {
      push(i, line.length, "comment");
      break;
    }
    if (c === "/" && line[i + 1] === "*") {
      state.comment = true;
      const close = line.indexOf("*/", i + 2);
      const end = close < 0 ? line.length : close + 2;
      push(i, end, "comment");
      if (close >= 0) state.comment = false;
      i = end;
      continue;
    }
    if (line.startsWith('"""', i)) {
      state.textBlock = true;
      const close = line.indexOf('"""', i + 3);
      const end = close < 0 ? line.length : close + 3;
      push(i, end, "string");
      if (close >= 0) state.textBlock = false;
      i = end;
      continue;
    }
    if (c === '"' || c === "'") {
      let j = i + 1;
      while (j < line.length && line[j] !== c) j += line[j] === "\\" ? 2 : 1;
      push(i, Math.min(line.length, j + 1), "string");
      i = j + 1;
      continue;
    }
    if (c === "@" && isIdentStart(line[i + 1] ?? "")) {
      let j = i + 1;
      while (j < line.length && (isIdent(line[j]) || line[j] === ".")) j++;
      push(i, j, "annotation");
      i = j;
      continue;
    }
    if (/[0-9]/.test(c) || (c === "." && /[0-9]/.test(line[i + 1] ?? ""))) {
      let j = i + 1;
      while (j < line.length && /[0-9A-Za-z_.]/.test(line[j])) j++;
      push(i, j, "number");
      i = j;
      continue;
    }
    if (isIdentStart(c)) {
      let j = i + 1;
      while (j < line.length && isIdent(line[j])) j++;
      const word = line.slice(i, j);
      let k = j;
      while (line[k] === " ") k++;
      if (KEYWORDS.has(word)) push(i, j, "keyword");
      else if (LITERALS.has(word)) push(i, j, "literal");
      else if (line[k] === "(") push(i, j, "function");
      else if (word.length > 1 && /^[A-Z][A-Z0-9_]*$/.test(word)) push(i, j, "constant");
      else if (/^[A-Z]/.test(word)) push(i, j, "type");
      i = j;
      continue;
    }
    i++;
  }
  return out;
}

export const highlights = (path) => path.endsWith(".java");
