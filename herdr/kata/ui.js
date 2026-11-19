import { div } from "gpui-kit";

export const FLAGS = [
  { flag: "must-do", label: "Must do", color: "ansi1" },
  { flag: "suggestion", label: "Suggestion", color: "ansi4" },
  { flag: "question", label: "Question", color: "ansi5" },
];
export const FLAG_COLORS = Object.fromEntries(FLAGS.map((f) => [f.flag, f.color]));
export const FLAG_LABELS = Object.fromEntries(FLAGS.map((f) => [f.flag, f.label]));
export const RESOLUTION_LABELS = { resolve: "Resolved", "wont-fix": "Won't fix", unresolve: "Reopened" };
export const STATUS_LETTERS = { added: "A", deleted: "D", modified: "M", renamed: "R" };

export const statusColor = (status, t) => (status === "added" ? t.ansi2 : status === "deleted" ? t.ansi1 : t.blocked);

export function button(id, text, onClick, t, primary = false) {
  return div().id(id).h(26).px(10).flex().flex_none().items_center().rounded(6).text_size(12).cursor_pointer()
    .when(primary, (e) => e.bg(t.solid).text_color(t.solid_foreground))
    .when(!primary, (e) => e.border_1().border_color(t.border).text_color(t.foreground).hover((s) => s.bg(t.hover)))
    .on_mouse_down("left", (_e, cx) => cx.stop_propagation())
    .on_click((_e, cx) => onClick(cx))
    .child(text);
}

export function chip(text, color, t) {
  return div().flex_none().px(5).py(1).rounded(4).border_1().border_color(color ?? t.border).text_size(10).text_color(color ?? t.muted).child(text);
}

export function link(id, text, onClick, t) {
  return div().id(id).flex_none().text_size(11).text_color(t.muted).cursor_pointer().hover((s) => s.text_color(t.foreground))
    .on_click((_e, cx) => onClick(cx)).child(text);
}

// A labelled on/off switch: a track with a knob that sits right and takes the accent color when on.
export function toggleSwitch(id, label, on, onClick, t) {
  const track = div().w(26).h(14).flex_none().rounded(7).relative().bg(on ? t.accent : t.border)
    .child(div().absolute().top(2).left(on ? 14 : 2).w(10).h(10).rounded(5).bg(on ? t.accent_foreground : t.muted));
  return div().id(id).flex().flex_none().items_center().gap(6).cursor_pointer().text_size(11).text_color(on ? t.foreground : t.dim)
    .on_mouse_down("left", (_e, cx) => cx.stop_propagation())
    .on_click((_e, cx) => onClick(cx))
    .child(track)
    .child(label);
}

// Theme palettes often use pastel greens and reds, which go grey as a faint tint over a dark background. Keep the
// theme's hue, but push saturation up and lightness to the middle so a tint of it still reads as green or red.
function vivid(hex) {
  const n = parseInt(hex.slice(1, 7), 16);
  let [r, g, b] = [(n >> 16) & 255, (n >> 8) & 255, n & 255].map((v) => v / 255);
  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  let h = 0;
  let sat = 0;
  const light = (max + min) / 2;
  if (max !== min) {
    const d = max - min;
    sat = light > 0.5 ? d / (2 - max - min) : d / (max + min);
    if (max === r) h = (g - b) / d + (g < b ? 6 : 0);
    else if (max === g) h = (b - r) / d + 2;
    else h = (r - g) / d + 4;
    h /= 6;
  }
  const s2 = Math.min(0.85, Math.max(0.55, sat * 1.8));
  const l2 = Math.min(0.55, Math.max(0.42, light));
  const q = l2 < 0.5 ? l2 * (1 + s2) : l2 + s2 - l2 * s2;
  const p = 2 * l2 - q;
  const channel = (x) => {
    let t = x;
    if (t < 0) t += 1;
    if (t > 1) t -= 1;
    if (t < 1 / 6) return p + (q - p) * 6 * t;
    if (t < 1 / 2) return q;
    if (t < 2 / 3) return p + (q - p) * (2 / 3 - t) * 6;
    return p;
  };
  return `#${[channel(h + 1 / 3), channel(h), channel(h - 1 / 3)].map((v) => Math.round(v * 255).toString(16).padStart(2, "0")).join("")}`;
}

const tints = new Map();
// Diff colors from the theme's green (ansi2) and red (ansi1): row and word backgrounds, and the +/− marker.
export function diffColors(t) {
  const key = `${t.ansi2}|${t.ansi1}`;
  if (!tints.has(key)) {
    const add = vivid(t.ansi2);
    const remove = vivid(t.ansi1);
    tints.set(key, { addRow: `${add}38`, removeRow: `${remove}38`, addWord: `${add}80`, removeWord: `${remove}80`, add, remove });
  }
  return tints.get(key);
}
