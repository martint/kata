import { View, div } from "gpui-kit";
import { v_flex, h_flex, Scrollbar, v_virtual_list, VirtualListScrollHandle, TextareaState, TextView, InputState, Input } from "gpui-base";
import { props, panes, focused, spaces, theme, settings, run, checking, knownAgents, exec, hc, shared, setShared, openOverlay, closeOverlay } from "herdr";
import { kata, repoRoot, authorName, nextEvent } from "./api.js";
import { readState, updateState } from "./state.js";
import { reviewRows, threadsByFile, resolution, expandChange, CARD_INDENT, LINE_HEIGHT } from "./rows.js";
import { buildTree, orderedFiles, treeRows } from "./tree.js";
import { composerDrawer, threadDrawer } from "./drawer.js";
import { agentsInSpace, checkLabel, labelsByAgent, moveAgent, promptHasText, reviewKey, setLabel, UNASSIGNED } from "./agents.js";
import { FLAG_COLORS, FLAG_LABELS, RESOLUTION_LABELS, STATUS_LETTERS, statusColor, button, chip, toggleSwitch, diffColors } from "./ui.js";

const MOD = "kata";
// Roughly one monospace character at the diff's 11px text, for how far the code can scroll sideways.
const CHAR_WIDTH = 6.7;
const MENU = "kata.menu";
const MENU_CLOSED = "kata.menu-closed";
// A click outside a popup closes it and still reaches the list; one landing this soon after is that click.
const MENU_CLICK_GRACE_MS = 700;
const SIDEBAR_WIDTH = 280;
const DIFF_FETCHES = 6;
const reviewLabel = (repo, number) => `${repo}#${number}`;
const newKey = () => `k${Date.now().toString(36)}${Math.random().toString(36).slice(2, 8)}`;
const sameReview = (a, b) => Boolean(a && b && a.repo === b.repo && a.review === b.review);
const livePanes = () => new Set(panes().map((p) => p.pane));
const spaceOfTab = (tab) => spaces().find((s) => s.tabs.some((t) => t.id === tab)) ?? null;
const mono = () => settings().font_family ?? "JetBrains Mono";
const plural = (n, word, many = `${word}s`) => `${n} ${n === 1 ? word : many}`;

// What a publish holds, for the note to agents: "2 new comments and 1 reply"; null when it's only resolutions.
function publishedSummary(drafts) {
  const comments = drafts?.comments?.length ?? 0;
  const replies = (drafts?.responses ?? []).filter((r) => (r.body ?? "").trim()).length;
  const parts = [];
  if (comments) parts.push(plural(comments, "new comment"));
  if (replies) parts.push(plural(replies, "reply", "replies"));
  return parts.length ? parts.join(" and ") : null;
}

const piecesText = (list) => (list ?? []).map((p) => p.text).join("");
// What a row says, for search.
function rowText(row) {
  switch (row.kind) {
    case "file":
      return row.file;
    case "line":
      return piecesText(row.pieces);
    case "split":
      return `${piecesText(row.left?.pieces)}\n${piecesText(row.right?.pieces)}`;
    case "thread":
    case "marker":
      return [row.thread.comment.body, ...row.thread.responses.map((r) => r.body)].filter(Boolean).join("\n");
    default:
      return "";
  }
}

const SYNTAX_COLORS = {
  keyword: (t) => t.ansi5,
  literal: (t) => t.ansi3,
  string: (t) => t.ansi2,
  comment: (t) => t.dim,
  number: (t) => t.ansi3,
  annotation: (t) => t.ansi3,
  type: (t) => t.ansi6,
  constant: (t) => t.ansi3,
  function: (t) => t.ansi4,
};

export default class Kata extends View {
  init(_props, cx) {
    const given = props() ?? {};
    // A right-click menu is a popup copy of this mod; it only lists the pane's actions and hands the pick back.
    this.menu = given.menu ?? null;
    if (this.menu) return;
    this.key = given.key ?? null;
    this.current = given.repo && given.review ? { repo: given.repo, review: Number(given.review) } : null;
    this.pane = null;
    this.tab = null;
    this.space = null;
    this.mode = "loading";
    this.width = 1000;
    this.error = null;
    this.notice = null;
    this.busy = false;
    this.repos = null;
    this.repo = null;
    this.reviews = new Map();
    this.choosingRepo = false;
    this.showArchived = false;
    this.openHere = new Map();
    this.identifyAttempts = 0;
    this.identifying = false;
    this.scroll = VirtualListScrollHandle.new();
    this.moving = new Set();
    this.showOtherAgents = false;
    this.tellAgents = true;
    this.adopting = new Set();
    this.openInSpace = null;
    this.reconciled = null;
    this.listening = 0;
    this.unified = false;
    this.scrollX = 0;
    this.showSummary = true;
    this.patchset = null;
    this.compare = null;
    this.choosingPatchset = false;
    this.searchOpen = false;
    this.searchInput = null;
    this.searchQuery = "";
    this.matches = [];
    this.matchAt = -1;
    this.viewMode = "both";
    this.statuses = new Set(["draft", "open", "resolved"]);
    this.flags = new Set(["must-do", "suggestion", "question"]);
    this.diffWidth = 0;
    this.pressX = 0;
    this.resetReview();
    if (checking()) return;
    // The list tells us which rows it drew, not where it scrolled; check that a few times a second for the current file.
    cx.timer.every(200, (cx) => this.followScroll(cx));
    this.identify(cx);
  }

  resetReview() {
    this.view = null;
    this.scrollX = 0;
    this.expanded = new Map();
    this.changes = new Map();
    this.files = [];
    this.tree = null;
    this.threads = { byFile: new Map(), all: [] };
    this.rows = [];
    this.rowIndex = new Map();
    this.rowsFor = null;
    this.uiVersion = 0;
    this.collapsed = new Set();
    this.collapsedFolders = new Set();
    this.openTops = new Set();
    this.scope = null;
    this.expanded = new Map();
    this.currentFile = null;
    this.visibleStart = 0;
    this.drawer = null;
    this.composer = null;
    this.reply = null;
    this.thread = null;
    this.shiftPressed = false;
    this.confirmDiscard = false;
    this.confirmDelete = false;
  }

  // A copy can't ask which pane it is: find its record by key, else claim the kata pane in the focused tab.
  // A copy only starts once its tab is shown, so that tab is focused when this runs. A just-made pane can be
  // missing from panes() on the first try, so render retries a few times as Herdr's data catches up.
  identify(cx) {
    this.identifying = true;
    this.identifyAttempts += 1;
    const here = focused();
    const candidates = panes().filter((p) => p.mod === MOD && p.tab === here?.tab);
    const first = this.mode === "loading";
    cx.spawn(async (cx) => {
      const shown = this.current;
      try {
        const { state } = await readState();
        let entry = this.key ? Object.entries(state.panes).find(([, record]) => record.key === this.key) : null;
        if (!entry && candidates.length === 1) {
          const record = state.panes[candidates[0].pane];
          if (!record || !record.key || record.key === this.key) entry = [candidates[0].pane, record ?? null];
        }
        if (entry) {
          const [pane, record] = entry;
          this.pane = pane;
          this.tab = panes().find((p) => p.pane === pane)?.tab ?? record?.tab ?? null;
          this.space = spaceOfTab(this.tab)?.id ?? record?.space ?? null;
          if (record && "review" in record) this.current = record.review;
          await this.persist();
          this.notice = null;
        }
      } catch (e) {
        this.notice = `Couldn't record this pane: ${e}`;
      }
      this.identifying = false;
      if (!this.pane && this.identifyAttempts >= 5) this.notice ??= "Couldn't tell which pane this is; changes here won't be remembered";
      if (first || !sameReview(shown, this.current)) {
        if (this.current) this.load(cx);
        else this.openPicker("empty", cx);
      } else if (this.pane) {
        this.rename();
      }
      cx.notify();
    });
  }

  async persist() {
    if (!this.pane) return;
    const record = { key: this.key, space: this.space, tab: this.tab, review: this.current };
    await updateState(livePanes(), (state) => {
      state.panes[this.pane] = record;
    });
  }

  rename() {
    if (!this.tab) return;
    const tab = spaceOfTab(this.tab)?.tabs.find((t) => t.id === this.tab);
    let name = "";
    if (this.current && this.view) {
      const title = this.view.manifest.name ?? "";
      name = `${reviewLabel(this.current.repo, this.current.review)} · ${title.length > 48 ? `${title.slice(0, 47)}…` : title}`;
    }
    if (tab && (name ? tab.name !== name : tab.named)) run("tab.rename", { id: this.tab, name });
  }

  // ---- Loading -------------------------------------------------------------

  load(cx) {
    const target = this.current;
    this.mode = "review";
    this.error = null;
    this.resetReview();
    cx.notify();
    cx.spawn(async (cx) => {
      try {
        const view = await kata.review(target.repo, target.review, this.version());
        if (!sameReview(this.current, target)) return;
        this.setView(view);
        this.rename();
        this.fetchDiffs(target, cx);
        this.listen(target, view.manifest.review_id, cx);
        if (!this.repos) this.repos = await kata.repos().catch(() => null);
      } catch (e) {
        if (sameReview(this.current, target)) this.error = e.message;
      }
      cx.notify();
    });
  }

  // Which patchset to show and what to compare it to; null means the latest, and the base.
  version() {
    return { patchset: this.patchset ?? undefined, compare: this.compare ?? undefined };
  }

  latestPatchset() {
    return this.view?.manifest.current_patchset ?? 1;
  }

  // Comments go on the latest patchset's endpoints only, as in the web UI's cumulative view.
  writable() {
    return this.patchset == null && this.compare == null;
  }

  selectVersion(patchset, compare, cx) {
    this.patchset = patchset === this.latestPatchset() ? null : patchset;
    this.compare = compare != null && compare < (patchset ?? this.latestPatchset()) ? compare : null;
    this.choosingPatchset = false;
    this.load(cx);
  }

  setView(view) {
    const patchsetChanged = this.view && view.manifest.current_patchset !== this.view.manifest.current_patchset;
    if (patchsetChanged) this.changes = new Map();
    this.view = view;
    if (patchsetChanged) this.scope = null;
    this.derive();
  }

  // Files, tree and thread placement for what's shown: the whole review, or one commit of it.
  derive() {
    const view = this.view;
    const scope = this.scope;
    let files = view.diff.files;
    let endpoints = { scoped: false };
    if (scope) {
      const counts = new Map((scope.commit.changed_files ?? []).map((f) => [f.path, f]));
      files = scope.diff.files.map((f) => ({ ...f, added: counts.get(f.path)?.added ?? f.added, removed: counts.get(f.path)?.removed ?? f.removed }));
      endpoints = { scoped: true, base: scope.diff.base_commit, tip: scope.diff.tip_commit, baseChange: scope.diff.base_change, tipChange: scope.diff.tip_change };
    }
    this.tree = buildTree(files);
    this.files = orderedFiles(this.tree);
    this.threads = threadsByFile(view, endpoints);
    if (this.thread && !this.threads.all.some((t) => t.comment.comment_id === this.thread)) this.closeDrawerState();
    if (!this.files.some((f) => f.path === this.currentFile)) this.currentFile = this.files[0]?.path ?? null;
    this.bump();
  }

  // Fetched hunks for what's shown, with files expanded to their full contents swapped in.
  activeChanges() {
    const base = this.scope ? this.scope.changes : this.changes;
    if (!this.expanded.size) return base;
    const out = new Map(base);
    for (const [path, entry] of this.expanded) {
      if (entry.scope === (this.scope?.commit.change_id ?? null) && entry.change) out.set(path, entry.change);
    }
    return out;
  }

  isExpanded(path) {
    return this.expanded.get(path)?.scope === (this.scope?.commit.change_id ?? null);
  }

  toggleExpand(file, cx) {
    if (this.isExpanded(file.path)) {
      this.expanded.delete(file.path);
      this.bump();
      cx.notify();
      return;
    }
    const scope = this.scope?.commit.change_id ?? null;
    const commit = this.scope ? this.scope.diff.tip_commit : this.view.diff.tip;
    const target = this.current;
    const entry = { scope, change: null };
    this.expanded.set(file.path, entry);
    cx.notify();
    cx.spawn(async (cx) => {
      try {
        const text = await kata.fileText(target.repo, commit, file.path);
        const change = this.activeChanges().get(file.path);
        if (change && this.expanded.get(file.path) === entry) entry.change = expandChange(change, text);
      } catch (e) {
        this.expanded.delete(file.path);
        this.notice = e.message;
      }
      this.bump();
      cx.notify();
    });
  }

  // Selecting the selected commit again goes back to all commits.
  selectCommit(commit, cx) {
    if (!commit || this.scope?.commit.change_id === commit.change_id) {
      this.scope = null;
      this.derive();
      this.scroll.scroll_to_item(0, "top");
      cx.notify();
      return;
    }
    const target = this.current;
    this.scopeLoading = commit.change_id;
    this.expanded = new Map();
    cx.notify();
    cx.spawn(async (cx) => {
      try {
        const diff = await kata.commitDiff(target.repo, target.review, commit.change_id);
        if (sameReview(this.current, target) && this.scopeLoading === commit.change_id) {
          this.scope = { commit, diff, changes: new Map(diff.files.map((f) => [f.path, f])) };
          this.derive();
          this.scroll.scroll_to_item(0, "top");
        }
      } catch (e) {
        this.notice = e.message;
      }
      this.scopeLoading = null;
      cx.notify();
    });
  }

  stepCommit(by, cx) {
    const commits = this.view?.commits ?? [];
    if (!commits.length) return;
    const at = this.scope ? commits.findIndex((c) => c.change_id === this.scope.commit.change_id) : -1;
    const next = at < 0 ? (by > 0 ? 0 : commits.length - 1) : at + by;
    this.selectCommit(next < 0 || next >= commits.length ? null : commits[next], cx);
  }

  // Every file's hunks, a few at a time in display order, so the top of the review fills in first.
  fetchDiffs(target, cx) {
    const queue = this.files.map((f) => f.path).filter((path) => !this.changes.has(path));
    const version = this.version();
    const worker = async (cx) => {
      while (queue.length && sameReview(this.current, target)) {
        const path = queue.shift();
        try {
          const change = await kata.fileDiff(target.repo, target.review, path, version);
          if (sameReview(this.current, target)) this.changes.set(path, change);
        } catch (e) {
          if (sameReview(this.current, target)) this.changes.set(path, { path, error: e.message });
        }
        this.bump();
        cx.notify();
      }
    };
    for (let i = 0; i < DIFF_FETCHES; i++) cx.spawn(worker);
  }

  // Follow the server's events for this review: someone (usually an agent) publishing, refreshing or
  // deleting it reloads the pane. One loop per loaded review; a newer load ends the older loop.
  listen(target, reviewId, cx) {
    const generation = ++this.listening;
    const live = () => generation === this.listening && sameReview(this.current, target);
    cx.spawn(async (cx) => {
      let failures = 0;
      while (live()) {
        // Touching cx each pass ends this loop once a newer build of the mod replaces this copy.
        await new Promise((resolve) => cx.timer.after(1, () => resolve()));
        if (failures > 0) {
          // After the server was unreachable, reload once it answers: what changed meanwhile came without an event.
          try {
            await this.reloadView(cx);
            failures = 0;
          } catch {
            failures += 1;
            await new Promise((resolve) => cx.timer.after(Math.min(30000, 2000 * failures), () => resolve()));
            continue;
          }
        }
        let event = null;
        try {
          event = await nextEvent(exec);
        } catch {
          failures = 1;
          continue;
        }
        if (!live() || !event || event.review_id !== reviewId) continue;
        if (event.kind === "review-deleted") {
          this.notice = "This review was deleted";
          cx.notify();
          continue;
        }
        if (this.busy) continue;
        try {
          await this.reloadView(cx);
        } catch (e) {
          this.notice = e.message;
        }
        cx.notify();
      }
    });
  }

  // Re-fetch comments and drafts after a change, keeping the loaded diffs.
  async reloadView(cx) {
    const target = this.current;
    const view = await kata.review(target.repo, target.review, this.version());
    if (!sameReview(this.current, target)) return;
    this.setView(view);
    this.rename();
    this.fetchDiffs(target, cx);
    cx.notify();
  }

  // Every write goes through here, one at a time; the view then reloads so drafts show as the server has them.
  mutate(cx, work, done) {
    if (this.busy || !this.current) return;
    this.busy = true;
    this.notice = null;
    cx.notify();
    const target = this.current;
    cx.spawn(async (cx) => {
      try {
        await work(target, cx);
        done?.();
        await this.reloadView(cx);
      } catch (e) {
        this.notice = e.message;
      }
      this.busy = false;
      this.bump();
      cx.notify();
    });
  }

  async sessionId(target) {
    const existing = this.view?.drafts?.session;
    if (existing && existing.status === "draft") return existing.session_id;
    const session = await kata.startSession(target.repo, target.review);
    return session.session_id;
  }

  bump() {
    this.uiVersion += 1;
    this.rowsFor = null;
  }

  async close() {
    if (this.menu) {
      setShared(MENU_CLOSED, { pane: this.menu.pane, at: Date.now() });
      return;
    }
    this.listening += 1;
  }

  // ---- Right-click menu ----------------------------------------------------

  menuItems(row) {
    switch (row.kind) {
      case "line":
      case "split": {
        const items = [];
        if (this.writable()) items.push(["comment-line", "Comment on this line"], ["comment-file", "Comment on this file"]);
        return [...items, ["top", "Top of file"], ["toggle-file", "Collapse file"]];
      }
      case "file": {
        const info = this.files.find((f) => f.path === row.file);
        const items = [["toggle-file", row.collapsed ? "Expand file" : "Collapse file"]];
        if (info && (info.status === "modified" || info.status === "renamed")) items.push(["expand", this.isExpanded(row.file) ? "Collapse to diff" : "Show full file"]);
        if (this.writable()) items.push(["comment-file", "Comment on this file"]);
        return items;
      }
      case "thread":
      case "marker": {
        const { comment } = row.thread;
        if (comment.draft) return [["open", "Open"], ["edit", "Edit draft"], ["delete", "Delete draft"]];
        const open = resolution(row.thread) === "open";
        return [["open", "Reply…"], ...(open ? [["resolve", "Resolve"], ["wontfix", "Won't fix"]] : [["reopen", "Reopen"]])];
      }
      default:
        return [];
    }
  }

  openMenu(key, event, cx) {
    const row = this.rows[this.rowIndex.get(key)];
    if (!row || !this.pane) return;
    const items = this.menuItems(row);
    if (!items.length) return;
    // Which half of a split row was pressed, for "Comment on this line".
    const half = (event?.local_position?.x ?? 0) < (event?.bounds?.width ?? this.diffWidth) / 2 ? "left" : "right";
    openOverlay({ mod: MOD, kind: "popup", width: 220, height: items.length * 28 + 12, props: { menu: { pane: this.pane, key, half, items } } });
    this.menuOpen = true;
  }

  // The popup writes its pick to shared state; the pane it was opened for acts on it once.
  takeMenuPick(cx) {
    const closed = shared(MENU_CLOSED);
    if (this.menuOpen && closed?.pane === this.pane && closed.at !== this.menuClosedAt) {
      this.menuClosedAt = closed.at;
      // Escape or a pick closed it; only a click arriving right now is the closing click.
      if (Date.now() - closed.at >= MENU_CLICK_GRACE_MS) this.menuOpen = false;
    }
    const pick = shared(MENU);
    if (!pick || pick.pane !== this.pane || pick.id === this.menuPick) return;
    this.menuPick = pick.id;
    this.menuOpen = false;
    cx.spawn(async (cx) => {
      this.runMenu(pick, cx);
      cx.notify();
    });
  }

  runMenu({ action, key, half }, cx) {
    const row = this.rows[this.rowIndex.get(key)];
    if (!row) return;
    const thread = row.thread;
    switch (action) {
      case "comment-line":
        if (row.kind === "split") {
          this.pressX = half === "left" ? 0 : this.diffWidth;
          this.clickSplit(row, cx);
        } else {
          this.clickUnified(row, cx);
        }
        break;
      case "comment-file":
        this.openComposer({ kind: "file", file: row.file }, cx);
        break;
      case "top":
        this.jumpToFile(row.file, cx);
        break;
      case "toggle-file":
        this.toggleFile(row.file, cx);
        break;
      case "expand": {
        const info = this.files.find((f) => f.path === row.file);
        if (info) this.toggleExpand(info, cx);
        break;
      }
      case "open":
        this.openThread(thread, cx);
        break;
      case "edit":
        this.editDraft(thread.comment, cx);
        break;
      case "delete":
        this.deleteDraft(thread.comment, cx);
        break;
      case "resolve":
        this.respond(thread.comment.comment_id, "resolve", cx);
        break;
      case "wontfix":
        this.respond(thread.comment.comment_id, "wont-fix", cx);
        break;
      case "reopen":
        this.respond(thread.comment.comment_id, "unresolve", cx);
        break;
    }
  }

  renderMenu() {
    const t = theme();
    const { pane, key, half, items } = this.menu;
    return v_flex().id("kata-menu").size_full().p(6).gap(2).bg(t.popover).text_color(t.foreground).text_size(13)
      .children(items.map(([action, label]) => div().id(`menu-${action}`).h(26).flex_none().px(10).flex().items_center().rounded(6).cursor_pointer()
        .hover((s) => s.bg(t.hover))
        .on_click(() => {
          setShared(MENU, { pane, key, half, action, id: `${Date.now()}-${Math.random().toString(36).slice(2, 8)}` });
          closeOverlay(props().overlay);
        })
        .child(label)));
  }

  // ---- Agents --------------------------------------------------------------

  // Herdr re-renders on every terminal, label and tab change; act only when something about this space's agents did.
  maybeReconcile(cx) {
    if (checking() || this.mode !== "review" || !this.current || !this.pane || !this.tab || !this.space) return;
    const labels = labelsByAgent();
    const mine = reviewKey(this.current.repo, this.current.review);
    const list = agentsInSpace(this.space);
    const signature = `${mine}|${this.tab}|${list.map((a) => `${a.address}@${a.tab}=${labels.get(a.address) ?? ""}`).join(",")}`;
    if (signature === this.reconciled) return;
    this.reconciled = signature;
    for (const agent of list) {
      const label = labels.get(agent.address);
      if (label === mine && agent.tab !== this.tab) this.pull(agent, cx);
      else if (label == null && agent.tab === this.tab) this.adopt(agent, mine, cx);
    }
    cx.spawn(async (cx) => {
      this.openInSpace = await this.openReviewsInSpace().catch(() => null);
      cx.notify();
    });
  }

  async openReviewsInSpace() {
    const { state } = await readState();
    const live = livePanes();
    const open = new Set();
    for (const [pane, record] of Object.entries(state.panes)) {
      if (record.review && record.space === this.space && live.has(pane)) open.add(reviewKey(record.review.repo, record.review.review));
    }
    if (this.current) open.add(reviewKey(this.current.repo, this.current.review));
    return open;
  }

  pull(agent, cx) {
    if (this.moving.has(agent.address)) return;
    this.moving.add(agent.address);
    const [space, tab] = [this.space, this.tab];
    cx.spawn(async (cx) => {
      try {
        await moveAgent(cx, agent.address, space, tab);
      } catch (e) {
        this.notice = `Couldn't move ${agent.address}: ${e.message}`;
      }
      this.moving.delete(agent.address);
      cx.notify();
    });
  }

  // An agent started inside a review tab works on that review.
  adopt(agent, mine, cx) {
    if (this.adopting.has(agent.address)) return;
    this.adopting.add(agent.address);
    cx.spawn(async (cx) => {
      try {
        await setLabel(agent.address, mine);
      } catch (e) {
        this.notice = e.message;
      }
      this.adopting.delete(agent.address);
      cx.notify();
    });
  }

  assign(agent, value, cx) {
    cx.spawn(async (cx) => {
      try {
        await setLabel(agent.address, value);
      } catch (e) {
        this.notice = e.message;
      }
      cx.notify();
    });
  }

  // Beside the review, in the review's repo: a mod pane has no folder, so a plain split would start in ~.
  newAgent(cx) {
    if (!this.pane || !this.current) return;
    const pane = this.pane;
    const target = this.current;
    cx.spawn(async (cx) => {
      try {
        const agent = (await knownAgents()).find((a) => a.installed);
        if (!agent) {
          this.notice = "No installed agent to start";
        } else {
          this.repos ??= await kata.repos();
          const repo = this.repos.find((r) => r.name === target.repo);
          run("pane.focus", { pane });
          run("terminal.open", { argv: agent.run, place: "right", ...(repo ? { folder: repoRoot(repo) } : {}) });
        }
      } catch (e) {
        this.notice = e.message;
      }
      cx.notify();
    });
  }

  // ---- Navigation ----------------------------------------------------------

  followScroll(cx) {
    if (this.mode !== "review" || !this.rows.length) return;
    const file = this.rows[Math.min(this.visibleStart, this.rows.length - 1)]?.file ?? null;
    if (file && file !== this.currentFile && !this.jumping) {
      this.currentFile = file;
      cx.notify();
    }
    this.jumping = false;
  }

  jumpToFile(path, cx) {
    this.currentFile = path;
    this.collapsed.delete(path);
    this.bump();
    this.currentRows();
    const index = this.rowIndex.get(`file:${path}`);
    if (index !== undefined) {
      this.jumping = true;
      this.scroll.scroll_to_item(index, "top");
    }
    cx.notify();
  }

  stepFile(by, cx) {
    if (!this.files.length) return;
    const at = Math.max(0, this.files.findIndex((f) => f.path === this.currentFile));
    const next = this.files[(at + by + this.files.length) % this.files.length];
    this.jumpToFile(next.path, cx);
  }

  toggleFile(path, cx) {
    if (this.collapsed.has(path)) this.collapsed.delete(path);
    else this.collapsed.add(path);
    this.bump();
    cx.notify();
  }

  threadStatus(thread) {
    if (thread.comment.draft) return "draft";
    return resolution(thread) === "open" ? "open" : "resolved";
  }

  shows(thread) {
    return this.statuses.has(this.threadStatus(thread)) && this.flags.has(thread.comment.flag);
  }

  toggleIn(set, value, cx) {
    if (set.has(value)) set.delete(value);
    else set.add(value);
    this.bump();
    cx.notify();
  }

  // Next or previous comment in display order, from the selected thread or else the top of the view.
  stepComment(by, cx) {
    const rows = this.currentRows();
    const stops = rows.map((row, i) => [row, i]).filter(([row]) => row.kind === "thread" || row.kind === "marker");
    if (!stops.length) return;
    const from = this.thread ? stops.findIndex(([row]) => row.thread.comment.comment_id === this.thread) : -1;
    let next;
    if (from >= 0) next = stops[(from + by + stops.length) % stops.length];
    else next = (by > 0 ? stops.find(([, i]) => i >= this.visibleStart) : [...stops].reverse().find(([, i]) => i < this.visibleStart)) ?? stops[0];
    const [row] = next;
    this.openThread(row.thread, cx);
    const index = this.currentRows().findIndex((r) => r.thread?.comment.comment_id === row.thread.comment.comment_id);
    if (index >= 0) this.scroll.scroll_to_item(index, "center");
    this.currentFile = row.file;
    cx.notify();
  }

  openSearch(cx) {
    this.searchOpen = !this.searchOpen;
    if (this.searchOpen) {
      if (!this.searchInput) {
        this.searchInput = InputState.new({ placeholder: "Search files, code and comments" });
        this.searchInput.on("change", (_e, cx) => this.runSearch(cx));
        this.searchInput.on("submit", (event, cx) => this.stepMatch(event?.shift ? -1 : 1, cx));
      }
      this.focusSoon(this.searchInput, cx);
    } else {
      this.searchQuery = "";
      this.matches = [];
      this.bump();
    }
    cx.notify();
  }

  // Case-insensitive substring over every file path, diff line and thread, collapsed files included.
  runSearch(cx) {
    const query = this.searchInput.value().trim().toLowerCase();
    this.searchQuery = query;
    this.matches = [];
    this.matchAt = -1;
    if (query) {
      const everything = reviewRows(this.files, this.activeChanges(), this.threads, this.width - SIDEBAR_WIDTH, {
        selection: null, thread: null, collapsed: new Set(), unified: this.unified, openTops: new Set(this.files.map((f) => f.path)),
        mode: this.viewMode, shows: (thread) => this.shows(thread),
      });
      this.matches = everything.filter((row) => rowText(row).toLowerCase().includes(query)).map((row) => ({ key: row.key, file: row.file }));
    }
    this.bump();
    cx.notify();
  }

  stepMatch(by, cx) {
    if (!this.matches.length) return;
    this.matchAt = (this.matchAt + by + this.matches.length) % this.matches.length;
    const match = this.matches[this.matchAt];
    this.collapsed.delete(match.file);
    this.openTops.add(match.file);
    this.currentFile = match.file;
    this.bump();
    const index = this.currentRows().findIndex((row) => row.key === match.key);
    if (index >= 0) {
      this.jumping = true;
      this.scroll.scroll_to_item(index, "center");
    }
    cx.notify();
  }

  toggleTopComments(path, cx) {
    if (this.openTops.has(path)) this.openTops.delete(path);
    else this.openTops.add(path);
    this.bump();
    cx.notify();
  }

  toggleFolder(path, cx) {
    if (this.collapsedFolders.has(path)) this.collapsedFolders.delete(path);
    else this.collapsedFolders.add(path);
    cx.notify();
  }

  // ---- Clicks in the diff --------------------------------------------------

  // The click that closes the right-click menu only closes it.
  closingMenu() {
    const closed = shared(MENU_CLOSED);
    const recent = closed?.pane === this.pane && Date.now() - closed.at < MENU_CLICK_GRACE_MS;
    const open = this.menuOpen;
    this.menuOpen = false;
    return open || recent;
  }

  clickRow(key, cx) {
    if (this.closingMenu()) return;
    const row = this.rows[this.rowIndex.get(key)];
    if (!row) return;
    if (row.kind === "file") this.toggleFile(row.file, cx);
    else if (row.kind === "line") this.clickUnified(row, cx);
    else if (row.kind === "split") this.clickSplit(row, cx);
    else if (row.kind === "thread" || row.kind === "marker") this.openThread(row.thread, cx);
    else if (row.kind === "fold") this.toggleTopComments(row.file, cx);
  }

  clickUnified(row, cx) {
    if (row.base === undefined) return;
    const side = row.origin === "removed" ? "base" : "tip";
    this.clickLine(row.file, side, side === "base" ? row.base : row.tip, cx);
  }

  // A split row is one item to the list; which half was pressed comes from where the press landed.
  clickSplit(row, cx) {
    const leftHalf = this.pressX < this.diffWidth / 2;
    const half = (leftHalf ? row.left : row.right) ?? row.left ?? row.right;
    const side = half === row.left ? "base" : "tip";
    this.clickLine(row.file, side, half?.number, cx);
  }

  // Click a line to start a comment there; shift-click extends the range on the same side. A typed body survives moving.
  clickLine(file, side, line, cx) {
    if (line == null) return;
    if (!this.writable()) {
      this.notice = "Comments go on the latest patchset; switch back to comment";
      cx.notify();
      return;
    }
    const open = this.composer;
    if (this.shiftPressed && this.drawer === "compose" && open?.kind === "line" && !open.editing && open.file === file && open.side === side) {
      open.start = Math.min(open.from, line);
      open.end = Math.max(open.from, line);
    } else {
      this.openComposer({ kind: "line", file, side, start: line, end: line, from: line }, cx);
    }
    this.currentFile = file;
    this.bump();
    cx.notify();
  }

  openComposer(target, cx, draft = null) {
    const keep = this.composer && !this.composer.editing && !draft;
    const input = keep ? this.composer.input : TextareaState.new({ placeholder: "Leave a comment", value: draft?.body ?? "" });
    if (!keep) input.on("submit", (_e, cx) => this.saveComposer(cx));
    this.composer = {
      ...target,
      editing: draft?.comment_id ?? null,
      original: draft,
      flag: draft?.flag ?? this.composer?.flag ?? "must-do",
      input,
      error: null,
    };
    this.drawer = "compose";
    this.thread = null;
    this.focusSoon(input, cx);
    this.bump();
    cx.notify();
  }

  openThread(thread, cx) {
    const id = thread.comment.comment_id;
    this.drawer = "thread";
    this.thread = id;
    if (!thread.comment.draft && this.reply?.id !== id) {
      const input = TextareaState.new({ placeholder: "Reply" });
      input.on("submit", (_e, cx) => this.respond(id, "comment", cx));
      this.reply = { id, input };
    }
    this.bump();
    cx.notify();
  }

  editDraft(comment, cx) {
    this.openComposer({ kind: comment.lines ? "line" : "file", file: comment.file, side: comment.side, start: comment.lines?.start, end: comment.lines?.end }, cx, comment);
  }

  closeDrawerState() {
    this.drawer = null;
    this.thread = null;
    if (this.composer?.editing) this.composer = null;
  }

  // Closing keeps an unsaved comment's text for the next line you click; Cancel in the composer drops it.
  closeDrawer(cx) {
    this.closeDrawerState();
    this.bump();
    cx.notify();
  }

  cancelComposer(cx) {
    this.composer = null;
    this.closeDrawer(cx);
  }

  focusSoon(input, cx) {
    input.focus();
    cx.timer.after(50, () => input.focus());
  }

  // ---- Writes --------------------------------------------------------------

  // New comments in the cumulative diff anchor to the current patchset's endpoints, as in the web UI.
  anchor(side) {
    const scope = this.scope?.diff;
    if (scope) {
      if (side === "base") return { anchor_change_id: scope.base_change, anchor_commit_id: scope.base_commit };
      return { anchor_change_id: scope.tip_change, anchor_commit_id: scope.tip_commit };
    }
    const m = this.view.manifest;
    const patchset = m.patchsets.find((p) => p.n === m.current_patchset) ?? m.patchsets[m.patchsets.length - 1];
    if (side === "base") return { anchor_change_id: patchset.base_change, anchor_commit_id: this.view.diff.base };
    return { anchor_change_id: patchset.tip_change, anchor_commit_id: this.view.diff.tip };
  }

  saveComposer(cx) {
    const open = this.composer;
    if (!open || this.busy) return;
    const body = open.input.value().trim();
    if (!body) {
      open.error = "Write something first";
      cx.notify();
      return;
    }
    this.mutate(cx, async (target) => {
      if (open.editing) {
        const d = open.original;
        await kata.updateComment(target.repo, target.review, d.session_id, d.comment_id, {
          anchor_change_id: d.anchor_change_id,
          anchor_commit_id: d.anchor_commit_id,
          file: d.file,
          side: d.side,
          lines: d.lines,
          columns: d.columns,
          review_wide: d.review_wide,
          flag: open.flag,
          body,
        });
        return;
      }
      const session = await this.sessionId(target);
      const input = { ...this.anchor(open.kind === "line" ? open.side : "tip"), file: open.file, flag: open.flag, body };
      if (open.kind === "line") {
        input.side = open.side;
        input.lines = { start: open.start, end: open.end };
      }
      await kata.createComment(target.repo, target.review, session, input);
    }, () => {
      if (this.composer === open) {
        this.composer = null;
        this.closeDrawerState();
      }
    });
  }

  // A reply, a resolution, or both: one draft response, published with the rest of the session.
  respond(commentId, action, cx) {
    const reply = this.reply?.id === commentId ? this.reply : null;
    const body = reply?.input.value().trim() ?? "";
    if (action === "comment" && !body) return;
    this.mutate(cx, async (target) => {
      const session = await this.sessionId(target);
      await kata.createResponse(target.repo, target.review, session, { in_reply_to: commentId, action, body: body || undefined });
    }, () => {
      reply?.input.set_value("");
    });
  }

  deleteDraft(item, cx) {
    this.mutate(cx, async (target) => {
      if (item.response_id) await kata.deleteResponse(target.repo, target.review, item.session_id, item.response_id);
      else await kata.deleteComment(target.repo, target.review, item.session_id, item.comment_id);
    });
  }

  publish(cx) {
    const session = this.view?.drafts?.session;
    if (!session) return;
    const published = publishedSummary(this.view.drafts);
    // Resolutions alone give an agent nothing to address, so they don't interrupt it.
    const tell = this.tellAgents && published ? this.agentsOnReview() : [];
    this.told = null;
    this.mutate(cx, async (target, cx) => {
      await kata.publishSession(target.repo, target.review, session.session_id);
      await this.tellAgentsAbout(target, published, tell, cx);
    }, () => {
      this.confirmDiscard = false;
    });
  }

  agentsOnReview() {
    if (!this.space || !this.current) return [];
    const labels = labelsByAgent();
    const mine = reviewKey(this.current.repo, this.current.review);
    return agentsInSpace(this.space).filter((a) => labels.get(a.address) === mine).map((a) => a.address);
  }

  // Publishing types a short note into each agent on this review, so it picks the new comments up without being asked.
  // An agent whose prompt has the person's text in it is told once that prompt is empty, checked every few seconds.
  async tellAgentsAbout(target, what, addresses, cx) {
    if (!addresses.length) return;
    const text = `[kata] I published ${what} on ${target.repo}#${target.review}. Read them with the kata MCP (get_review, repo "${target.repo}", review ${target.review}) and address them.`;
    const failed = [];
    let told = 0;
    const send = async (address) => {
      const result = await hc(["send", address, text]);
      if (result.code !== 0) failed.push(address);
      else told += 1;
    };
    const waiting = [];
    for (const address of addresses) {
      if (await promptHasText(address)) waiting.push(address);
      else await send(address);
    }
    const report = () => {
      this.told = told ? `Told ${told} agent${told === 1 ? "" : "s"}` : null;
      const parts = [];
      if (waiting.length) parts.push(`Will tell ${waiting.join(", ")} once its prompt is empty`);
      if (failed.length) parts.push(`Couldn't tell ${failed.join(", ")}`);
      this.notice = parts.length ? parts.join(" · ") : null;
    };
    report();
    if (!waiting.length) return;
    cx.spawn(async (cx) => {
      const deadline = Date.now() + 10 * 60 * 1000;
      while (waiting.length && Date.now() < deadline) {
        await new Promise((resolve) => cx.timer.after(3000, () => resolve()));
        for (const address of [...waiting]) {
          if (await promptHasText(address)) continue;
          waiting.splice(waiting.indexOf(address), 1);
          await send(address);
        }
        report();
        cx.notify();
      }
      if (waiting.length) {
        failed.push(...waiting.splice(0));
        report();
        cx.notify();
      }
    });
  }

  // ---- Picker --------------------------------------------------------------

  // Close review: the pane stays, empty, and the review leaves this space's open reviews.
  closeReview(cx) {
    this.current = null;
    this.listening += 1;
    this.resetReview();
    this.rename();
    cx.spawn(async (cx) => {
      await this.persist().catch(() => {});
      cx.notify();
    });
    this.openPicker("empty", cx);
  }

  openPicker(purpose, cx) {
    this.mode = "picker";
    this.picking = purpose;
    this.choosingRepo = false;
    cx.spawn(async (cx) => {
      try {
        const [repos, { state }] = await Promise.all([this.repos ? this.repos : kata.repos(), readState()]);
        this.repos = repos;
        const live = livePanes();
        this.openHere = new Map(
          Object.entries(state.panes)
            .filter(([pane, record]) => record.review && record.space === this.space && live.has(pane))
            .map(([pane, record]) => [reviewLabel(record.review.repo, record.review.review), { pane, tab: record.tab }]),
        );
        if (!this.repo) this.repo = this.defaultRepo();
        if (this.repo) await this.loadReviews(this.repo, cx);
        this.error = null;
      } catch (e) {
        this.error = e.message;
      }
      cx.notify();
    });
    cx.notify();
  }

  // The repo whose workspace holds the folder you're working in: another pane in this tab, else the space's folder.
  defaultRepo() {
    const folders = [
      ...panes().filter((p) => p.tab === this.tab && p.cwd).map((p) => p.cwd),
      spaceOfTab(this.tab)?.folder,
      focused()?.cwd,
    ].filter(Boolean);
    for (const folder of folders) {
      const match = this.repos
        .filter((r) => folder === repoRoot(r) || folder.startsWith(`${repoRoot(r)}/`))
        .sort((a, b) => repoRoot(b).length - repoRoot(a).length)[0];
      if (match) return match.name;
    }
    return this.current?.repo ?? null;
  }

  async loadReviews(repo, cx) {
    try {
      const reviews = await kata.reviews(repo);
      reviews.sort((a, b) => b.manifest.number - a.manifest.number);
      this.reviews.set(repo, reviews);
    } catch (e) {
      this.error = e.message;
    }
    cx.notify();
  }

  choose(repo, number, cx) {
    if (sameReview(this.current, { repo, review: number })) {
      this.back(cx);
      return;
    }
    const open = this.openHere.get(reviewLabel(repo, number));
    if (open && open.pane !== this.pane) {
      run("tab.select", { id: open.tab });
      if (this.picking === "other") this.back(cx);
      return;
    }
    if (this.picking === "other") {
      run("tab.new", { space: this.space ?? undefined, mod: MOD, props: { key: newKey(), repo, review: number }, focus: true });
      this.back(cx);
      return;
    }
    this.current = { repo, review: number };
    cx.spawn(async (cx) => {
      await this.persist().catch(() => {});
      cx.notify();
    });
    this.load(cx);
  }

  back(cx) {
    if (this.current) {
      this.mode = "review";
      if (!this.view) this.load(cx);
    }
    cx.notify();
  }

  // ---- Rendering -----------------------------------------------------------

  currentRows() {
    const width = this.width - SIDEBAR_WIDTH;
    const memo = `${this.uiVersion}|${Math.round(width / 40)}`;
    if (this.rowsFor !== memo && this.view) {
      this.rowsFor = memo;
      const selection = this.drawer === "compose" && this.composer?.kind === "line" ? this.composer : null;
      this.rows = reviewRows(this.files, this.activeChanges(), this.threads, width, {
        selection, thread: this.thread, collapsed: this.collapsed, unified: this.unified, openTops: this.openTops,
        mode: this.viewMode, shows: (thread) => this.shows(thread),
      });
      this.rowIndex = new Map(this.rows.map((row, i) => [row.key, i]));
      this.widestLine = this.rows.reduce((most, row) => Math.max(most, piecesText(row.pieces).length,
        piecesText(row.left?.pieces).length, piecesText(row.right?.pieces).length), 0);
      const current = this.matches[this.matchAt]?.key;
      const matched = new Set(this.matches.map((m) => m.key));
      for (const row of this.rows) {
        if (matched.has(row.key)) row.match = row.key === current ? "current" : "other";
      }
    }
    return this.rows;
  }

  render(cx) {
    if (this.menu) return this.renderMenu();
    this.takeMenuPick(cx);
    if (!checking() && !this.pane && !this.identifying && this.identifyAttempts < 5) this.identify(cx);
    this.maybeReconcile(cx);
    const t = theme();
    let body;
    if (this.mode === "loading") body = this.message("Loading…", t);
    else if (this.mode === "picker") body = this.renderPicker(t);
    else body = this.renderReview(t);
    return v_flex().id("kata").size_full().min_h(0).bg(t.background).text_color(t.foreground).text_size(13)
      .on_size_change((size, cx) => {
        if (Math.abs(size.width - this.width) > 1) {
          this.width = size.width;
          cx.notify();
        }
      })
      .child(body);
  }

  message(text, t, action) {
    return v_flex().flex_1().items_center().justify_center().gap(10).p(20).text_color(t.muted)
      .child(div().child(text))
      .when(Boolean(action), (e) => e.child(action));
  }

  renderPicker(t) {
    const reviews = (this.repo && this.reviews.get(this.repo)) ?? null;
    const shown = (reviews ?? []).filter((r) => this.showArchived || !r.manifest.archived_at);
    const archived = (reviews ?? []).filter((r) => r.manifest.archived_at).length;
    const header = h_flex().h(44).flex_none().px(14).gap(8).items_center().border_b(1).border_color(t.border)
      .child(div().font_semibold().child(this.picking === "empty" ? "No open review" : "Open review"))
      .child(div().flex_1())
      .when(this.picking === "other", (e) => e.child(button("picker-cancel", "Cancel", (cx) => this.back(cx), t)));
    const repoRow = h_flex().id("picker-repo").h(36).flex_none().px(14).gap(8).items_center().border_b(1).border_color(t.border).cursor_pointer()
      .hover((s) => s.bg(t.hover))
      .on_click((_e, cx) => {
        this.choosingRepo = !this.choosingRepo;
        cx.notify();
      })
      .child(div().text_color(t.dim).child("Repository"))
      .child(div().font_semibold().child(this.repo ?? "Choose…"))
      .child(div().text_color(t.dim).child(this.choosingRepo ? "▴" : "▾"));
    let list;
    if (this.error) {
      list = this.message(this.error, t, button("picker-retry", "Retry", (cx) => this.openPicker(this.picking, cx), t));
    } else if (this.choosingRepo || !this.repo) {
      list = (this.repos ?? []).map((r) =>
        h_flex().id(`repo-${r.name}`).h(30).flex_none().px(14).gap(8).items_center().cursor_pointer()
          .when(r.name === this.repo, (e) => e.bg(t.selected))
          .hover((s) => s.bg(t.hover))
          .on_click((_e, cx) => {
            this.repo = r.name;
            this.choosingRepo = false;
            cx.spawn(async (cx) => this.loadReviews(r.name, cx));
            cx.notify();
          })
          .child(div().child(r.name))
          .child(div().flex_1().min_w(0).truncate().text_color(t.dim).text_size(11).child(repoRoot(r))));
    } else if (!reviews) {
      list = this.message("Loading reviews…", t);
    } else if (!shown.length) {
      list = this.message(`No ${archived ? "open " : ""}reviews in ${this.repo}`, t);
    } else {
      list = shown.map((r) => {
        const m = r.manifest;
        const open = this.openHere.get(reviewLabel(this.repo, m.number));
        const isThis = sameReview(this.current, { repo: this.repo, review: m.number });
        return v_flex().id(`review-${m.number}`).flex_none().px(14).py(7).gap(2).cursor_pointer().border_b(1).border_color(t.border)
          .hover((s) => s.bg(t.hover))
          .on_click((_e, cx) => this.choose(this.repo, m.number, cx))
          .child(h_flex().gap(8).items_center()
            .child(div().text_color(t.dim).child(`#${m.number}`))
            .child(div().flex_1().min_w(0).truncate().child(m.name ?? m.revset))
            .when(Boolean(open) || isThis, (e) => e.child(chip(isThis ? "This tab" : "Open in this space", t.accent, t)))
            .when(Boolean(m.archived_at), (e) => e.child(chip("Archived", null, t))))
          .child(h_flex().gap(10).text_size(11).text_color(t.dim)
            .child(div().child(authorName(m.created_by)))
            .child(div().child(`${r.published_comment_count} comments`))
            .child(div().child(`patchset ${m.current_patchset}`)));
      });
    }
    return v_flex().flex_1().min_h(0)
      .child(header)
      .child(repoRow)
      .child(div().flex_1().min_h(0).relative()
        .child(v_flex().id("picker-list").absolute().inset_0().overflow_y_scroll().children(Array.isArray(list) ? list : [list]))
        .child(Scrollbar.vertical("picker-list").absolute().inset_0()))
      .when(archived > 0 && !this.choosingRepo, (e) => e.child(h_flex().id("picker-archived").h(30).flex_none().px(14).items_center().border_t(1).border_color(t.border)
        .text_size(11).text_color(t.dim).cursor_pointer().hover((s) => s.bg(t.hover))
        .on_click((_e, cx) => {
          this.showArchived = !this.showArchived;
          cx.notify();
        })
        .child(this.showArchived ? "Hide archived reviews" : `Show ${archived} archived`)));
  }

  renderReview(t) {
    if (this.error && !this.view) {
      return this.message(this.error, t, h_flex().gap(8)
        .child(button("review-retry", "Retry", (cx) => this.load(cx), t))
        .child(button("review-close", "Close review", (cx) => this.closeReview(cx), t)));
    }
    if (!this.view) return this.message("Loading review…", t);
    const view = this.view;
    const m = view.manifest;
    const drafts = (view.drafts?.comments?.length ?? 0) + (view.drafts?.responses?.length ?? 0);
    const header = h_flex().h(44).flex_none().px(14).gap(8).items_center().border_b(1).border_color(t.border)
      .child(div().flex_none().text_color(t.dim).child(reviewLabel(this.current.repo, m.number)))
      .child(div().flex_1().min_w(0).truncate().font_semibold().child(m.name ?? m.revset))
      .child(button("review-version", this.versionLabel(), (cx) => {
        this.choosingPatchset = !this.choosingPatchset;
        cx.notify();
      }, t))
      .when(Boolean(m.archived_at), (e) => e.child(chip("Archived", null, t)))
      .when(this.busy, (e) => e.child(chip("Saving…", null, t)))
      .when(Boolean(this.notice), (e) => e.child(div().max_w(260).truncate().text_size(11).text_color(t.ansi1).child(this.notice)))
      .when(view.is_stale, (e) => e.child(button("review-patchset", "New patchset", (cx) => this.newPatchset(cx), t)))
      .child(button("review-open", "Open review", (cx) => this.openPicker("other", cx), t))
      .child(button("review-close", "Close review", (cx) => this.closeReview(cx), t))
      .when(Boolean(this.told) && drafts === 0, (e) => e.child(div().flex_none().text_size(11).text_color(t.dim).child(this.told)))
      .when(drafts > 0, (e) => e
        .child(button("review-discard", this.confirmDiscard ? `Discard ${drafts}?` : "Discard", (cx) => this.discard(cx), t))
        .child(toggleSwitch("review-tell", "Tell agents", this.tellAgents, (cx) => {
          this.tellAgents = !this.tellAgents;
          cx.notify();
        }, t))
        .child(button("review-publish", `Publish ${drafts}`, (cx) => this.publish(cx), t, true)));
    const summary = this.renderSummary(m, t);
    const content = view.diff_unavailable
      ? this.message("This review's commits are no longer in the repo, so there's no diff to show.", t)
      : h_flex().flex_1().min_h(0).child(this.renderSidebar(t)).child(this.renderDiff(t));
    return v_flex().flex_1().min_h(0).child(header).when(this.choosingPatchset, (e) => e.child(this.renderVersions(t))).child(summary).child(content);
  }

  versionLabel() {
    const shown = this.patchset ?? this.latestPatchset();
    return `PS${shown}${this.patchset == null ? " (latest)" : ""} vs ${this.compare == null ? "base" : `PS${this.compare}`} ▾`;
  }

  renderVersions(t) {
    const patchsets = [...this.view.manifest.patchsets].sort((a, b) => b.n - a.n);
    const shown = this.patchset ?? this.latestPatchset();
    const row = (label, items) => h_flex().gap(6).items_center().child(div().w(90).flex_none().text_size(11).text_color(t.dim).child(label)).children(items);
    return v_flex().flex_none().px(14).py(8).gap(6).border_b(1).border_color(t.border).bg(t.surface)
      .child(row("Patchset", patchsets.map((p) => this.toggleChip(`ps-${p.n}`, `PS${p.n}${p.n === this.latestPatchset() ? " (latest)" : ""} · ${p.recorded_at.slice(0, 10)}`,
        p.n === shown, t.accent, (cx) => this.selectVersion(p.n, this.compare, cx), t))))
      .child(row("Compared to", [
        this.toggleChip("cmp-base", "base", this.compare == null, t.accent, (cx) => this.selectVersion(shown, null, cx), t),
        ...patchsets.filter((p) => p.n < shown).map((p) => this.toggleChip(`cmp-${p.n}`, `PS${p.n}`, this.compare === p.n, t.accent, (cx) => this.selectVersion(shown, p.n, cx), t)),
      ]));
  }

  renderSummary(m, t) {
    const archived = Boolean(m.archived_at);
    const bar = h_flex().h(32).flex_none().px(14).gap(8).items_center().text_size(12)
      .child(div().flex_none().text_color(t.dim).child("Revset"))
      .child(div().min_w(0).truncate().font_family(mono()).text_size(11).child(m.revset))
      .when(Boolean(m.bookmark), (e) => e.child(chip(m.bookmark, null, t)))
      .child(div().flex_none().text_size(11).text_color(t.dim).child(`by ${authorName(m.created_by)}`))
      .child(div().flex_1())
      .child(button("summary-archive", archived ? "Unarchive" : "Archive", (cx) => this.toggleArchive(cx), t))
      .child(button("summary-delete", this.confirmDelete ? "Delete for everyone?" : "Delete", (cx) => this.deleteReview(cx), t))
      .child(button("summary-toggle", this.showSummary ? "Hide description" : "Show description", (cx) => {
        this.showSummary = !this.showSummary;
        cx.notify();
      }, t));
    return v_flex().flex_none().border_b(1).border_color(t.border).bg(t.surface)
      .child(bar)
      .when(this.showSummary, (e) => e.child(div().max_h(200).flex_none().relative()
        .child(div().id("kata-summary").max_h(200).overflow_y_scroll().px(14).pb(10).text_size(13)
          .child(m.summary ? TextView.markdown("kata-summary-text", m.summary) : div().text_color(t.dim).child("No description")))));
  }

  renderSidebar(t) {
    const files = this.files;
    const at = files.findIndex((f) => f.path === this.currentFile);
    const added = files.reduce((n, f) => n + f.added, 0);
    const removed = files.reduce((n, f) => n + f.removed, 0);
    const counts = (path) => this.threads.byFile.get(path);
    const header = h_flex().h(34).flex_none().px(10).gap(6).items_center().border_b(1).border_color(t.border).text_size(12)
      .child(div().font_semibold().child(`Files (${files.length})`))
      .child(div().flex_1())
      .child(button("file-prev", "‹", (cx) => this.stepFile(-1, cx), t))
      .child(div().text_size(11).text_color(t.dim).child(`${at + 1}/${files.length}`))
      .child(button("file-next", "›", (cx) => this.stepFile(1, cx), t))
      .child(div().text_size(11).text_color(t.ansi2).child(`+${added}`))
      .child(div().text_size(11).text_color(t.ansi1).child(`−${removed}`));
    const rows = treeRows(this.tree, this.collapsedFolders).map((row) => {
      const indent = 8 + row.depth * 12;
      if (row.type === "folder") {
        const node = row.node;
        const open = !this.collapsedFolders.has(node.path);
        return h_flex().id(`folder-${node.path}`).h(24).flex_none().pl(indent).pr(8).gap(6).items_center().rounded(6).cursor_pointer()
          .hover((s) => s.bg(t.hover))
          .on_click((_e, cx) => this.toggleFolder(node.path, cx))
          .child(div().w(10).flex_none().text_color(t.dim).text_size(10).child(open ? "▾" : "▸"))
          .child(div().flex_1().min_w(0).truncate().text_color(t.muted).child(node.name))
          .child(div().text_size(11).text_color(t.ansi2).child(`+${node.added}`))
          .child(div().text_size(11).text_color(t.ansi1).child(`−${node.removed}`));
      }
      const f = row.file;
      const group = counts(f.path);
      const comments = group ? group.file.length + group.elsewhere.length + [...group.inline.values()].reduce((n, l) => n + l.length, 0) : 0;
      return h_flex().id(`tree-${f.path}`).h(24).flex_none().pl(indent).pr(8).gap(6).items_center().rounded(6).cursor_pointer()
        .when(this.currentFile === f.path, (e) => e.bg(t.selected))
        .hover((s) => s.bg(t.hover))
        .on_click((_e, cx) => this.jumpToFile(f.path, cx))
        .child(div().w(10).flex_none().text_size(10).text_color(statusColor(f.status, t)).child(STATUS_LETTERS[f.status] ?? "?"))
        .child(div().flex_1().min_w(0).truncate().child(row.name))
        .when(comments > 0, (e) => e.child(chip(String(comments), t.accent, t)))
        .child(div().text_size(11).text_color(t.ansi2).child(`+${f.added}`))
        .child(div().text_size(11).text_color(t.ansi1).child(`−${f.removed}`));
    });
    return v_flex().w(SIDEBAR_WIDTH).flex_none().h_full().border_r(1).border_color(t.border).text_size(12)
      .child(this.renderAgents(t))
      .child(this.renderCommits(t))
      .child(header)
      .child(div().flex_1().min_h(0).relative()
        .child(v_flex().id("kata-tree").absolute().inset_0().overflow_y_scroll().p(4).children(rows))
        .child(Scrollbar.vertical("kata-tree").absolute().inset_0()));
  }

  renderAgents(t) {
    if (!this.space || !this.current) return div();
    const labels = labelsByAgent();
    const mine = reviewKey(this.current.repo, this.current.review);
    const here = [];
    const unassigned = [];
    const elsewhere = [];
    for (const agent of agentsInSpace(this.space)) {
      const label = labels.get(agent.address);
      if (label === mine) {
        here.push(agent);
        continue;
      }
      const check = checkLabel(label, this.repos, this.openInSpace);
      if (check.kind === "ok") elsewhere.push([agent, `on ${check.repo}#${check.review}`]);
      else unassigned.push([agent, check.kind === "error" ? check.message : null]);
    }
    const row = (agent, note, action, error) => h_flex().id(`agent-${agent.address}`).h(26).flex_none().px(10).gap(6).items_center().rounded(6).cursor_pointer()
      .hover((s) => s.bg(t.hover))
      .on_click((_e, cx) => run("agent.focus", { terminal: agent.address }))
      .child(div().w(7).h(7).flex_none().rounded(4).bg(t[agent.status] ?? t.dim))
      .child(div().flex_none().child(agent.program?.name ?? agent.app ?? agent.address))
      .child(div().flex_1().min_w(0).truncate().text_size(11).text_color(error ? t.ansi1 : t.dim)
        .child(error ?? note ?? (this.moving.has(agent.address) ? "moving here…" : agent.title)))
      .when(Boolean(action), (e) => e.child(action));
    const group = (title, items) => (items.length ? [div().px(10).pt(4).text_size(10).text_color(t.dim).child(title), ...items] : []);
    const rows = [
      ...group("On this review", here.map((a) => row(a, a.tab === this.tab ? null : "moving here…",
        button(`unassign-${a.address}`, "Unassign", (cx) => this.assign(a, UNASSIGNED, cx), t)))),
      ...group("Unassigned", unassigned.map(([a, error]) => row(a, null, button(`assign-${a.address}`, "Assign here", (cx) => this.assign(a, mine, cx), t), error))),
      ...(elsewhere.length ? [
        div().id("agents-others").px(10).pt(4).text_size(10).text_color(t.dim).cursor_pointer().hover((s) => s.text_color(t.foreground))
          .on_click((_e, cx) => {
            this.showOtherAgents = !this.showOtherAgents;
            cx.notify();
          })
          .child(`${this.showOtherAgents ? "▾" : "▸"} On other reviews (${elsewhere.length})`),
        ...(this.showOtherAgents ? elsewhere.map(([a, note]) => row(a, note, button(`move-${a.address}`, "Move here", (cx) => this.assign(a, mine, cx), t))) : []),
      ] : []),
    ];
    return v_flex().flex_none().max_h(200).border_b(1).border_color(t.border)
      .child(h_flex().h(34).flex_none().px(10).gap(6).items_center().border_b(1).border_color(t.border)
        .child(div().font_semibold().child(`Agents (${here.length})`))
        .child(div().flex_1())
        .child(button("agent-new", "+ Agent", (cx) => this.newAgent(cx), t)))
      .child(v_flex().id("kata-agents").max_h(166).overflow_y_scroll().py(2)
        .children(rows.length ? rows : [div().px(10).py(6).text_size(11).text_color(t.dim).child("No agents in this space")]));
  }

  renderCommits(t) {
    const commits = this.view.commits ?? [];
    if (!commits.length) return div();
    const counts = new Map();
    for (const c of [...this.view.comments, ...(this.view.drafts?.comments ?? [])]) counts.set(c.anchor_change_id, (counts.get(c.anchor_change_id) ?? 0) + 1);
    const selected = this.scope?.commit.change_id ?? this.scopeLoading ?? null;
    const at = selected ? commits.findIndex((c) => c.change_id === selected) : -1;
    const header = h_flex().h(34).flex_none().px(10).gap(6).items_center().border_b(1).border_color(t.border)
      .child(div().font_semibold().child(`Commits (${commits.length})`))
      .child(div().flex_1())
      .child(button("commit-prev", "‹", (cx) => this.stepCommit(-1, cx), t))
      .child(div().text_size(11).text_color(t.dim).child(at < 0 ? "All" : `${at + 1}/${commits.length}`))
      .child(button("commit-next", "›", (cx) => this.stepCommit(1, cx), t));
    const date = (stamp) => (stamp ? stamp.slice(0, 10) : "");
    const rows = commits.map((c) => {
      const comments = counts.get(c.change_id) ?? 0;
      return v_flex().id(`commit-${c.change_id}`).flex_none().px(10).py(5).gap(1).rounded(6).cursor_pointer()
        .when(selected === c.change_id, (e) => e.bg(t.selected))
        .hover((s) => s.bg(t.hover))
        .on_click((_e, cx) => this.selectCommit(c, cx))
        .child(h_flex().gap(6).items_center()
          .child(div().flex_none().font_family(mono()).text_size(11).text_color(t.accent).child(c.change_id.slice(0, 8)))
          .child(div().flex_1().min_w(0).truncate().child(c.description_first_line || "(no description)"))
          .when(comments > 0, (e) => e.child(chip(String(comments), t.accent, t)))
          .when((c.conflict_paths ?? []).length > 0, (e) => e.child(chip("conflict", t.ansi1, t))))
        .child(h_flex().gap(6).text_size(10).text_color(t.dim)
          .child(div().font_family(mono()).child(c.commit_id.slice(0, 8)))
          .child(div().min_w(0).truncate().child(c.author_email ?? ""))
          .child(div().flex_none().child(date(c.author_timestamp))));
    });
    return v_flex().flex_none().max_h(220).border_b(1).border_color(t.border)
      .child(header)
      .child(div().flex_1().min_h(0).relative()
        .child(v_flex().id("kata-commits").max_h(186).overflow_y_scroll().p(4).children(rows)));
  }

  toggleChip(id, label, on, color, onClick, t) {
    return div().id(id).flex_none().px(6).py(1).rounded(4).border_1().border_color(on ? color : t.border).text_size(11).cursor_pointer()
      .text_color(on ? color : t.dim)
      .when(on, (e) => e.bg(`${color}22`))
      .on_click((_e, cx) => onClick(cx))
      .child(label);
  }

  renderSearch(t) {
    const count = this.matches.length ? `${this.matchAt < 0 ? "–" : this.matchAt + 1}/${this.matches.length}` : this.searchQuery ? "No matches" : "";
    return h_flex().h(36).flex_none().px(12).gap(8).items_center().border_b(1).border_color(t.border)
      .on_key_down((e, cx) => {
        if (e.key === "escape") {
          this.openSearch(cx);
          cx.stop_propagation();
        }
      })
      .child(div().id("kata-search").flex_1().min_w(0).h(26).px(8).flex().items_center().rounded(6).border_1().border_color(t.accent).text_size(12)
        .on_mouse_down("left", (_e, cx) => cx.stop_propagation())
        .child(Input.new(this.searchInput).h_full().text_size(12).text_color(t.foreground)))
      .child(div().flex_none().text_size(11).text_color(t.dim).child(count))
      .child(button("match-prev", "‹", (cx) => this.stepMatch(-1, cx), t))
      .child(button("match-next", "›", (cx) => this.stepMatch(1, cx), t))
      .child(button("search-close", "Close", (cx) => this.openSearch(cx), t));
  }

  renderToolbar(t) {
    const total = this.threads.all.filter((x) => this.shows(x)).length;
    const statuses = [["draft", "Draft", t.ansi3], ["open", "Open", t.ansi4], ["resolved", "Resolved", t.ansi2]];
    const flags = [["must-do", "Must do", t.ansi1], ["suggestion", "Suggestion", t.ansi4], ["question", "Question", t.ansi5]];
    const modes = [["both", "Both"], ["diff", "Diff"], ["comments", "Comments"]];
    return h_flex().min_h(34).flex_none().flex_wrap().px(12).py(4).gap(6).items_center().border_b(1).border_color(t.border).text_size(11)
      .child(div().flex_none().text_color(t.dim).child("Comments"))
      .child(button("comment-prev", "‹", (cx) => this.stepComment(-1, cx), t))
      .child(div().flex_none().text_color(t.dim).child(String(total)))
      .child(button("comment-next", "›", (cx) => this.stepComment(1, cx), t))
      .child(div().w(10).flex_none())
      .children(statuses.map(([key, label, color]) => this.toggleChip(`status-${key}`, label, this.statuses.has(key), color, (cx) => this.toggleIn(this.statuses, key, cx), t)))
      .child(div().w(10).flex_none())
      .children(flags.map(([key, label, color]) => this.toggleChip(`flag-filter-${key}`, label, this.flags.has(key), color, (cx) => this.toggleIn(this.flags, key, cx), t)))
      .child(div().flex_1().min_w(8))
      .child(this.toggleChip("search-toggle", "Search", this.searchOpen, t.accent, (cx) => this.openSearch(cx), t))
      .children(modes.map(([key, label]) => this.toggleChip(`mode-${key}`, label, this.viewMode === key, t.accent, (cx) => {
        this.viewMode = key;
        this.bump();
        cx.notify();
      }, t)));
  }

  // The current file's controls: rows inside the list can't hold buttons, so they live here, above it.
  renderFileBar(t) {
    const file = this.files.find((f) => f.path === this.currentFile);
    if (!file) return div();
    const collapsed = this.collapsed.has(file.path);
    return h_flex().h(34).flex_none().px(12).gap(8).items_center().border_b(1).border_color(t.border).text_size(12)
      .child(button("bar-collapse", collapsed ? "▸" : "▾", (cx) => this.toggleFile(file.path, cx), t))
      .child(div().flex_none().text_size(11).text_color(statusColor(file.status, t)).child(STATUS_LETTERS[file.status] ?? "?"))
      .child(div().flex_1().min_w(0).truncate().text_color(t.muted).child(file.old_path ? `${file.old_path} → ${file.path}` : file.path))
      .child(div().flex_none().text_size(11).text_color(t.ansi2).child(`+${file.added}`))
      .child(div().flex_none().text_size(11).text_color(t.ansi1).child(`−${file.removed}`))
      .child(button("bar-layout", this.unified ? "Split" : "Unified", (cx) => {
        this.unified = !this.unified;
        this.bump();
        cx.notify();
      }, t))
      .when(file.status === "modified" || file.status === "renamed", (e) => e.child(button("bar-expand",
        this.isExpanded(file.path) ? "Collapse to diff" : "Expand file", (cx) => this.toggleExpand(file, cx), t)))
      .child(button("bar-top", "Top of file", (cx) => this.jumpToFile(file.path, cx), t))
      .when(this.writable(), (e) => e.child(button("bar-comment", "Comment on file", (cx) => this.openComposer({ kind: "file", file: file.path }, cx), t)));
  }

  renderDiff(t) {
    if (!this.files.length) return this.message("No files changed", t);
    const rows = this.currentRows();
    let drawer = null;
    if (this.drawer === "compose" && this.composer) drawer = composerDrawer(this, t);
    else if (this.drawer === "thread") {
      const thread = this.threads.all.find((x) => x.comment.comment_id === this.thread);
      if (thread) drawer = threadDrawer(this, thread, t);
    }
    const list = v_virtual_list("kata-diff", rows.length, rows.map((r) => r.height), (i) => rows[i].key, (range) => {
      this.visibleStart = range.start;
      return rows.slice(range.start, range.end).map((row) => this.safeRow(row, t));
    }).track_scroll(this.scroll)
      .on_item_click((key, cx) => this.clickRow(key, cx))
      .on_item_secondary_click((key, event, cx) => this.openMenu(key, event, cx))
      .size_full();
    return v_flex().flex_1().min_w(0).h_full()
      .child(this.renderToolbar(t))
      .when(this.searchOpen, (e) => e.child(this.renderSearch(t)))
      .child(this.renderFileBar(t))
      .child(div().id("kata-diff-area").flex_1().min_h(0).relative().font_family(mono()).text_size(11)
        // The press before a click carries modifiers; the list's click report doesn't.
        .on_mouse_down("left", (event) => {
          this.shiftPressed = Boolean(event.modifiers?.shift);
          this.pressX = event.local_position?.x ?? 0;
        })
        // Rows of a virtual list don't ask for a redraw when hover changes; redraw when the pointer crosses into
        // another line's band, so the hover marker follows it. Only the visible rows rebuild.
        .on_mouse_move((event, cx) => {
          const band = `${Math.floor((event.local_position?.y ?? 0) / LINE_HEIGHT)}:${(event.local_position?.x ?? 0) < this.diffWidth / 2}`;
          if (band !== this.hoverBand) {
            this.hoverBand = band;
            cx.notify();
          }
        })
        .on_size_change((size) => {
          this.diffWidth = size.width;
        })
        // The list only scrolls vertically: sideways wheel movement (a trackpad swipe, or shift+scroll) moves every
        // line's code by one shared offset instead, leaving the line numbers in place.
        .on_scroll_wheel((event, cx) => {
          const dx = event.delta?.x ?? 0;
          const dy = event.delta?.y ?? 0;
          const by = Math.abs(dx) > Math.abs(dy) ? dx : event.modifiers?.shift ? dy : 0;
          if (!by) return;
          const split = this.rows.some((row) => row.kind === "split");
          const visible = (split ? this.diffWidth / 2 : this.diffWidth) - 120;
          const most = Math.max(0, (this.widestLine ?? 0) * CHAR_WIDTH - visible + 24);
          const current = this.scrollX ?? 0;
          const next = Math.min(most, Math.max(0, current - by));
          if (Math.abs(next - current) >= 1) {
            this.scrollX = next;
            cx.notify();
          }
        })
        .child(list)
        .child(Scrollbar.vertical("kata-diff").viewport_from_layout().absolute().inset_0()))
      .when(Boolean(drawer), (e) => e.child(drawer));
  }

  // An error thrown inside a virtual list's render blanks the whole list without a word; show it on the row instead.
  safeRow(row, t) {
    try {
      return this.diffRow(row, t);
    } catch (e) {
      return div().w_full().h(row.height).flex_none().px(12).text_color(t.ansi1).truncate().child(`Can't draw this ${row.kind} row: ${e}`);
    }
  }

  // Rows hold no handlers and no state: the list reports clicks by key (clickRow).
  diffRow(row, t) {
    switch (row.kind) {
      case "gap":
        return div().w_full().h(row.height).flex_none();
      case "file": {
        const f = row.info;
        return h_flex().w_full().h(row.height).flex_none().px(12).gap(8).items_center().bg(row.match ? `${t.ansi3}33` : t.raised).border_t(1).border_b(1).border_color(t.border)
          .font_family(settings().ui_font ?? "Geist").text_size(12)
          .child(div().w(10).flex_none().text_color(t.dim).child(row.collapsed ? "▸" : "▾"))
          .child(div().flex_none().text_size(11).text_color(statusColor(f.status, t)).child(STATUS_LETTERS[f.status] ?? "?"))
          .child(div().flex_1().min_w(0).truncate().child(f.old_path ? `${f.old_path} → ${f.path}` : f.path))
          .child(div().flex_none().text_size(11).text_color(t.ansi2).child(`+${f.added}`))
          .child(div().flex_none().text_size(11).text_color(t.ansi1).child(`−${f.removed}`));
      }
      case "hunk":
        return div().w_full().h(row.height).flex_none().px(12).flex().items_center().bg(t.surface).text_color(t.dim).child(row.text);
      case "note":
        return div().w_full().h(row.height).flex_none().px(12).flex().items_center().text_color(row.error ? t.ansi1 : t.dim).child(row.text);
      case "thread":
        return this.threadCard(row, t);
      case "marker":
        return this.threadMarker(row, t);
      case "split":
        return this.splitRow(row, t);
      case "fold":
        return h_flex().w_full().h(row.height).flex_none().pl(CARD_INDENT).pr(12).gap(6).items_center().text_size(10).text_color(t.dim)
          .font_family(settings().ui_font ?? "Geist")
          .child(div().flex_none().child(row.open ? "▾" : "▸"))
          .child(div().flex_1().min_w(0).truncate().child(`${row.count} resolved comment${row.count === 1 ? "" : "s"} on this file`));
      default:
        return this.lineRow(row, t);
    }
  }

  pieces(list, origin, t) {
    const colors = diffColors(t);
    const changedBg = origin === "added" ? colors.addWord : colors.removeWord;
    return list.map((piece) => div().flex_none().whitespace_nowrap().text_color(SYNTAX_COLORS[piece.kind]?.(t) ?? t.foreground)
      .when(piece.kind === "keyword", (e) => e.font_semibold())
      .when(piece.changed, (e) => e.bg(changedBg).rounded(2))
      .child(piece.text));
  }

  lineTint(e, origin, selected, t, match) {
    if (match === "current") return e.bg(`${t.ansi3}55`);
    if (match === "other") return e.bg(`${t.ansi3}22`);
    if (selected) return e.bg(`${t.accent}33`);
    if (origin === "added") return e.bg(diffColors(t).addRow);
    if (origin === "removed") return e.bg(diffColors(t).removeRow);
    return e;
  }

  gutter(width, number, origin, t) {
    return div().w(width).flex_none().h_full().flex().items_center().justify_end().pr(4).text_color(origin === "context" ? t.dim : t.muted)
      .child(number == null ? "" : String(number));
  }

  // Behind a thread or marker under a line, the colors of that line, so the comment doesn't cut a gap in a block.
  underlay(row, t) {
    const under = row.under;
    if (!under) return null;
    const tint = (origin, empty) => {
      const box = div().flex_1().h_full();
      if (empty) return box.bg(t.raised);
      return this.lineTint(box, origin, false, t);
    };
    if (under.split) {
      return h_flex().absolute().inset_0()
        .child(tint(under.left, under.left == null))
        .child(tint(under.right, under.right == null));
    }
    return h_flex().absolute().inset_0().child(tint(under.origin, false));
  }

  // An overlay over the whole row whose hover style shows a "+" in the left gutter and tints the row, as the
  // web UI's comment icon does. Hover is a style, so it costs no handler and no script work as the pointer moves.
  // Hover may only restyle the box, not its text, so the whole overlay (tint and "+") fades in with opacity.
  hoverMark(row, t) {
    if (!this.writable()) return null;
    return h_flex().absolute().inset_0().items_center().pl(2).bg(`${t.accent}14`).text_color(t.accent).text_size(12).font_semibold()
      .opacity(0)
      .hover((s) => s.opacity(1))
      .child("+");
  }

  lineRow(row, t) {
    const marker = row.origin === "added" ? "+" : row.origin === "removed" ? "−" : " ";
    const mark = this.hoverMark(row, t);
    return this.lineTint(h_flex().relative().w_full().h(row.height).flex_none().gap(8).pl(14).overflow_hidden(), row.origin, row.selected, t, row.match)
      .child(this.gutter(38, row.base, row.origin, t))
      .child(this.gutter(38, row.tip, row.origin, t))
      .child(div().w(10).flex_none().text_color(row.origin === "added" ? diffColors(t).add : diffColors(t).remove).child(marker))
      .child(this.code(row.pieces, row.origin, t))
      .when(Boolean(mark), (e) => e.child(mark));
  }

  // A line's code, shifted left by the diff's sideways scroll.
  code(pieces, origin, t) {
    return div().flex_1().min_w(0).h_full().overflow_hidden().relative()
      .child(h_flex().absolute().top(0).bottom(0).left(-(this.scrollX ?? 0)).items_center().children(this.pieces(pieces, origin, t)));
  }

  splitHalf(half, t, divider, match) {
    const box = h_flex().relative().flex_1().min_w(0).h_full().gap(6).pl(14).overflow_hidden().when(divider, (e) => e.border_l(1).border_color(t.border));
    if (!half) return box.bg(`${t.raised}`);
    const marker = half.origin === "added" ? "+" : half.origin === "removed" ? "−" : " ";
    const mark = this.hoverMark(half, t);
    return this.lineTint(box, half.origin, half.selected, t, match)
      .child(this.gutter(42, half.number, half.origin, t))
      .child(div().w(10).flex_none().text_color(half.origin === "added" ? diffColors(t).add : diffColors(t).remove).child(marker))
      .child(this.code(half.pieces, half.origin, t))
      .when(Boolean(mark), (e) => e.child(mark));
  }

  splitRow(row, t) {
    return h_flex().w_full().h(row.height).flex_none()
      .child(this.splitHalf(row.left, t, false, row.match))
      .child(this.splitHalf(row.right, t, true, row.match));
  }

  threadCard(row, t) {
    const { comment, responses } = row.thread;
    const selected = this.thread === comment.comment_id;
    const state = resolution(row.thread);
    const entry = (item, first) => v_flex().gap(2).when(!first, (e) => e.pl(12).border_l(2).border_color(t.border))
      .child(h_flex().h(20).gap(6).items_center()
        .child(div().font_semibold().child(item.external_author?.login ?? authorName(item.author)))
        .when(first && Boolean(comment.flag), (e) => e.child(chip(FLAG_LABELS[comment.flag] ?? comment.flag, t[FLAG_COLORS[comment.flag]], t)))
        .when(!first && item.action && item.action !== "comment", (e) => e.child(chip(RESOLUTION_LABELS[item.action] ?? item.action, null, t)))
        .when(Boolean(item.draft), (e) => e.child(chip("Draft", t.accent, t)))
        .when(first && Boolean(row.note), (e) => e.child(div().text_color(t.dim).text_size(11).child(row.note)))
        .when(first && state !== "open", (e) => e.child(chip(state === "resolved" ? "Resolved" : "Won't fix", t.ansi2, t))))
      .when(Boolean(item.body), (e) => e.child(div().text_size(12).child(item.body)));
    const under = this.underlay(row, t);
    return div().relative().w_full().h(row.height).flex_none().pl(CARD_INDENT).pr(12).py(3)
      .when(Boolean(under), (e) => e.child(under))
      .child(v_flex().size_full().overflow_hidden().gap(4).px(8).py(6).rounded(6).bg(t.surface).border_1()
        .border_color(selected ? t.accent : row.match ? t.ansi3 : t.border).font_family(settings().ui_font ?? "Geist").text_size(12)
        .child(entry(comment, true))
        .children(responses.map((r) => entry(r, false))));
  }

  threadMarker(row, t) {
    const { comment, responses } = row.thread;
    const first = (comment.body ?? "").split("\n")[0];
    const under = this.underlay(row, t);
    return h_flex().relative().w_full().h(row.height).flex_none().pl(CARD_INDENT).pr(12).gap(6).items_center().text_size(10)
      .text_color(row.selected ? t.accent : t.dim)
      .font_family(settings().ui_font ?? "Geist")
      .when(Boolean(under), (e) => e.child(under))
      .when(row.selected, (e) => e.child(div().absolute().inset_0().bg(`${t.accent}22`)))
      .child(div().relative().flex_none().child(row.selected ? "▾" : "▸"))
      .child(div().relative().flex_1().min_w(0).truncate()
        .child(`${resolution(row.thread) === "resolved" ? "Resolved" : "Won't fix"} · ${authorName(comment.author)}: ${first}${responses.length ? ` (${responses.length})` : ""}`));
  }
}
