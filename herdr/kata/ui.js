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
