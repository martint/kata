import { div } from "gpui-kit";
import { v_flex, h_flex, Scrollbar, Textarea } from "gpui-base";
import { settings } from "herdr";
import { authorName } from "./api.js";
import { resolution } from "./rows.js";
import { FLAGS, FLAG_COLORS, FLAG_LABELS, RESOLUTION_LABELS, button, chip, link } from "./ui.js";

export const DRAWER_HEIGHT = 260;

const field = (t, height, state) => div().h(height).flex_none().rounded(6).border_1().border_color(t.border).bg(t.background)
  .px(8).py(6).text_size(13).text_color(t.foreground).child(Textarea.new(state).size_full());

function where(target) {
  if (target.kind === "file") return "File comment";
  const lines = target.start === target.end ? `Line ${target.start}` : `Lines ${target.start}–${target.end}`;
  return `${lines}${target.side === "base" ? " (old side)" : ""}`;
}

function frame(pane, t, title, body) {
  return v_flex().h(DRAWER_HEIGHT).flex_none().border_t(1).border_color(t.accent).bg(t.surface).font_family(settings().ui_font ?? "Geist")
    .on_key_down((e, cx) => {
      if (e.key === "escape") {
        pane.closeDrawer(cx);
        cx.stop_propagation();
      }
    })
    .child(h_flex().h(34).flex_none().px(12).gap(8).items_center().border_b(1).border_color(t.border)
      .child(title)
      .child(div().flex_1())
      .child(link("drawer-close", "Close", (cx) => pane.closeDrawer(cx), t)))
    .child(body);
}

export function composerDrawer(pane, t) {
  const open = pane.composer;
  const flags = h_flex().gap(4).children(FLAGS.map(({ flag, label, color }) => {
    const on = open.flag === flag;
    return div().id(`flag-${flag}`).px(6).py(1).rounded(4).border_1().border_color(t[color]).text_size(11).cursor_pointer()
      .text_color(on ? t.background : t[color])
      .when(on, (e) => e.bg(t[color]))
      .on_click((_e, cx) => {
        open.flag = flag;
        cx.notify();
      })
      .child(label);
  }));
  const title = h_flex().gap(8).items_center().min_w(0)
    .child(div().font_semibold().text_size(12).child(open.editing ? "Edit draft" : "New comment"))
    .child(div().min_w(0).truncate().text_size(11).text_color(t.dim).child(`${open.file} · ${where(open)}`))
    .child(flags);
  return frame(pane, t, title, v_flex().flex_1().min_h(0).p(12).gap(8)
    .child(field(t, 110, open.input))
    .child(h_flex().gap(8).items_center()
      .child(div().flex_1().text_size(11).text_color(open.error ? t.ansi1 : t.dim)
        .child(open.error ?? (open.kind === "line" ? "Shift-click another line to make it a range · saved as a draft until you publish" : "Saved as a draft until you publish")))
      .child(button("composer-cancel", "Cancel", (cx) => pane.cancelComposer(cx), t))
      .child(button("composer-save", open.editing ? "Save" : "Add draft", (cx) => pane.saveComposer(cx), t, true))));
}

export function threadDrawer(pane, thread, t) {
  const { comment, responses } = thread;
  const id = comment.comment_id;
  const state = resolution(thread);
  const entry = (item, first) => v_flex().gap(2).when(!first, (e) => e.pl(12).border_l(2).border_color(t.border))
    .child(h_flex().h(20).gap(6).items_center()
      .child(div().font_semibold().text_size(12).child(item.external_author?.login ?? authorName(item.author)))
      .when(first && Boolean(comment.flag), (e) => e.child(chip(FLAG_LABELS[comment.flag] ?? comment.flag, t[FLAG_COLORS[comment.flag]], t)))
      .when(!first && item.action && item.action !== "comment", (e) => e.child(chip(RESOLUTION_LABELS[item.action] ?? item.action, null, t)))
      .when(Boolean(item.draft), (e) => e.child(chip("Draft", t.accent, t)))
      .when(!first && Boolean(item.draft), (e) => e.child(link(`delete-${item.response_id}`, "Delete", (cx) => pane.deleteDraft(item, cx), t))))
    .when(Boolean(item.body), (e) => e.child(div().text_size(12).child(item.body)));
  let actions = h_flex().gap(12).items_center();
  if (comment.draft) {
    actions = actions
      .child(link(`edit-${id}`, "Edit", (cx) => pane.editDraft(comment, cx), t))
      .child(link(`delete-${id}`, "Delete", (cx) => pane.deleteDraft(comment, cx), t));
  } else if (state === "open") {
    actions = actions
      .child(link(`resolve-${id}`, "Resolve", (cx) => pane.respond(id, "resolve", cx), t))
      .child(link(`wontfix-${id}`, "Won't fix", (cx) => pane.respond(id, "wont-fix", cx), t));
  } else {
    actions = actions.child(link(`reopen-${id}`, "Reopen", (cx) => pane.respond(id, "unresolve", cx), t));
  }
  const where = comment.lines ? `${comment.file}:${comment.lines.start}${comment.lines.end !== comment.lines.start ? `–${comment.lines.end}` : ""}` : comment.file;
  const title = h_flex().gap(8).items_center().min_w(0)
    .child(div().font_semibold().text_size(12).child("Thread"))
    .child(div().min_w(0).truncate().text_size(11).text_color(t.dim).child(where))
    .when(state !== "open", (e) => e.child(chip(state === "resolved" ? "Resolved" : "Won't fix", t.ansi2, t)));
  const reply = pane.reply?.id === id ? pane.reply : null;
  return frame(pane, t, title, h_flex().flex_1().min_h(0)
    .child(div().flex_1().min_w(0).h_full().relative()
      .child(v_flex().id("drawer-thread").absolute().inset_0().overflow_y_scroll().p(12).gap(8)
        .child(entry(comment, true))
        .children(responses.map((r) => entry(r, false)))
        .child(actions))
      .child(Scrollbar.vertical("drawer-thread").absolute().inset_0()))
    .when(Boolean(reply), (e) => e.child(v_flex().w(320).flex_none().h_full().p(12).gap(8).border_l(1).border_color(t.border)
      .child(field(t, 120, reply.input))
      .child(h_flex().gap(8).items_center().justify_end()
        .child(state === "open"
          ? button(`reply-resolve-${id}`, "Reply and resolve", (cx) => pane.respond(id, "resolve", cx), t)
          : button(`reply-reopen-${id}`, "Reply and reopen", (cx) => pane.respond(id, "unresolve", cx), t))
        .child(button(`reply-send-${id}`, "Reply", (cx) => pane.respond(id, "comment", cx), t, true))))));
}
