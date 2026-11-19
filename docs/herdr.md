# Kata in Herdr — design

Status: built (`herdr/kata/`). This describes the experience of reviewing
with kata inside [Herdr](https://herdr.dev), the mechanisms it relies on,
and what was verified about Herdr along the way. `SPEC.md` §22 summarizes
the user-visible behaviour; §7 here lists what's built and what isn't.

## 1. Problem

Kata reviews live in a browser; the agents that create and act on them
live in a terminal. Nothing ties a review to the agents working on it,
so the person switches windows constantly and tells each agent by hand
which review to look at.

Goal: review in Herdr, next to the agents, with no browser. The web UI
remains the full-featured surface; the Herdr mod is a second client of
the same local server.

Assumptions:

- One person and their agents on one machine. Nobody else connects to
  this kata server or this Herdr window.
- The kata server is already running (`kata serve`) and registers the
  repos the person works in.
- jj is the VCS; the mod never shells out to jj or git. Diffs, anchors
  and comments come from the kata HTTP API.

## 2. Model

| Kata in Herdr | Herdr | Notes |
|---|---|---|
| A container for related work | Space | Not tied to a repo: one space can hold reviews from several repos. |
| Review | Tab | One tab per review per space. Its left pane is the kata review pane. |
| Agents of a review | Agent terminals in that tab | To the right of the review pane. |
| Agent's primary review | Terminal label `kata.review=<repo>/<number>` | Source of truth. Location follows it. |
| Unassigned agents | A group in each review pane's Agents section (§3.3) | Not a tab: these agents stay wherever they are. |
| The space's open reviews | Review tabs in the space | What an agent should treat as its working context (§4.5). |

Rules:

1. A review has at most one tab per space. Different spaces can each
   have their own copy, and opening a review in one space never
   switches the person to another.
2. A tab holds at most one review.
3. An agent has one primary review or none. None, or a label that
   doesn't resolve (§4.6), means Unassigned.
4. An agent whose primary review is open in its space sits in that
   review's tab. Changing the primary review moves the agent; it keeps
   running, with its history. Agents never move between spaces.
5. Unassigned agents are never moved.
6. Every agent can already read every kata review through MCP; the
   space's open reviews tell it which ones matter now. Only the primary
   review decides where it lives. This covers the less common shapes
   too: one agent working through a stack, several agents on one
   review.
7. Agents never take the person away from what they're looking at.
   Everything an agent triggers happens unfocused (`focus: false`); the
   one exception is following: when the person is looking at an agent's
   tab and that agent moves to a review, the view goes with it.
8. No timers and no polling. The mod reacts to changes Herdr pushes
   (terminals, labels, tabs, panes) and to the person's clicks.

Because a review is a tab, switching agents and switching reviews are
the same action: selecting an agent in Herdr's agents list shows its
review's tab, and selecting a review's tab shows the agents working on
it.

## 3. Experience

### 3.1 Getting in without knowing a key

Keybindings are optional accelerators, never the only way in. Every
entry point is something visible to click, or something to ask an
agent for:

- **"Open review"** in every review pane's header, and in an empty pane
  (§3.6). It lists repos, then that repo's reviews, defaulting to the
  repo of the folder you're working in; reviews already open in this
  space are marked, and picking one selects its tab instead of opening
  a second.
- **Agents**, which open review tabs themselves (§3.4). "Open
  stargate#12 in kata" is a fine way in.

With no review pane in the space yet, only agents and keys remain. That
gap is accepted; the command palette can't help (§6.8). An earlier
draft had a sidebar panel docked into the layout as the door; it was
dropped because docking into Herdr's read-only shipped layout is
unverified and the mod should work on its own (§3.3).

Herdr's own agents list keeps working unchanged: the mod names each
review tab `<repo>#<number> · <title>`, so that list reads as agents
grouped by review without cloning it.

### 3.2 The review pane

The left pane of a review tab, mirroring the web UI where that makes
sense in a narrower native pane. What it holds is listed in §7.

### 3.3 The agents of a review

Each review pane's sidebar starts with an Agents section for its space:

```
Agents (2)                                  [+ Agent]
On this review
  ● Claude Code   fixing the retry loop     [Unassign]
Unassigned
  ● Claude Code   ⚠ No kata repo stargte    [Assign here]
▸ On other reviews (1)
```

- Clicking an agent shows it.
- `+ Agent` starts the first installed agent beside the review; it's
  adopted as soon as it appears (§3.5).
- **Assign here** and **Move here** set the agent's label to this
  review; the pane then pulls it into this tab (§4.3).
- **Unassign** sets the label to `unassigned`: the agent stays where it
  is and isn't re-adopted. No label and `unassigned` both mean
  Unassigned.
- An agent whose label doesn't resolve shows why (§4.6).
- Agents on other reviews in the space fold to one line; opening it
  lists them with **Move here**.

Agents set their own label the same way, with `kata-here` (§4.5).

### 3.4 An agent creates or adopts a review

- **Agent with no primary review**: after `create_review` it runs
  `kata-here assign <repo>/<number>`, which opens the review's tab in
  its space, labels the agent and moves it into that tab at once (the
  tab exists before its pane starts, §6.10). The person's view follows
  only if they were looking at the agent's tab; otherwise nothing they
  see changes.
- **Agent with a primary review**: it runs `kata-here open
  <repo>/<number>`, then asks the person in its chat whether to make it
  its primary one, and assigns only if they agree. Asking makes the
  agent "blocked", so Herdr's usual attention (sound, toast, agents
  list) brings the person to it.

Opening a review that already has a tab in the space reuses that tab.

The kata review skill (`crates/kata-mcp/skills/review.md`) carries these
instructions, gated on running inside Herdr with the mod installed.

### 3.5 Agents the mod didn't start

Agents are adopted however they were started (Herdr's + menu, `hc
run`, a split):

- Started inside a review tab: that review becomes its primary review,
  so it stays where the person put it.
- Started anywhere else: no label, so it shows under Unassigned and
  stays where it is.

### 3.6 Closing a review

Every way of closing removes the review from the space's open reviews.
They differ in how much UI goes with it:

| Action | Review pane | Agents in the tab |
|---|---|---|
| "Close review" in the pane header | Stays, showing "No open review" with "Open review" | Stay; listed under Unassigned as "not open here", with Reopen |
| Herdr's pane × | Gone | Stay, as above |
| Herdr's tab × | Gone | End. When a review is done, so are its agents. |

An empty pane is a door: opening a review from it shows that review in
the same pane, and the tab becomes that review's tab. Agents still
labelled for the old review stay Unassigned until reassigned; only
unlabelled agents are adopted (§3.5).

The review itself is never touched in kata by closing. Archiving or
deleting it in kata keeps the tab and shows the state in the pane, with
"Close review".

## 4. Mechanisms

### 4.1 One mod, many copies

There is one mod, `kata` (`herdr/kata/` in this repo, installed into
`~/.config/herdr-app/mods/kata`; `herdr/dev.sh` links it there and
reloads it on save). Herdr runs a separate copy wherever
it appears, and every copy sees the same live data:

| Copy | How it starts | Props | Responsible for |
|---|---|---|---|
| Review pane | `tab.new {mod: "kata", props, focus}` | `{key, repo, review}` | Its review in its space (§4.3) |

`mod.json` says `"place": "none"`: the mod only appears where a review
is opened, never docked into a layout. No copy is in charge of the
others. Copies do nothing while `checking()`.

### 4.2 State

| What | Where | Written by | Why |
|---|---|---|---|
| Agent → primary review | Terminal label `kata.review` | The person (Agents section) or the agent (`kata-here`) | A fact about one agent: lives with its terminal in herdr-core, survives app restarts, readable by the agent itself. |
| Review panes, and what each shows | `~/.config/herdr-app/data/kata/state.json`: `{version, panes: {<pane>: {key, space, tab, review: {repo, review} \| null}}}` | Each review pane, when it identifies itself, opens a review or closes one | `panes()` doesn't expose a mod pane's props, and props are fixed at creation (§6.1), so panes record themselves. Copies and agents read the same file; writes are compare-and-swap (`writeTextFile` with the previous text), and records for panes that no longer exist are pruned on every write. |
| Reviews, diffs, comments | kata server | — | Single source of truth, same as the browser. |

Nothing is derived and stored: Unassigned and label errors are worked
out when shown, from labels, tabs and the server.

### 4.3 Each review pane looks after its own review

A review pane for `<repo>/<n>` in space S, on every render (Herdr
re-renders it when terminals, labels, tabs or panes change):

0. **Identify, once at start.** A copy can't ask which pane it is
   (§6.9). Openers pass a random `key` in props; the copy finds its
   record by key, else claims the one kata pane in the focused tab
   (a copy only starts once its tab is shown, §6.10), retrying for a
   few renders while `panes()` catches up with a just-made pane.
1. **Duplicate check.** Opening a review that already has a live pane
   in S selects that pane's tab instead.
2. **Pull.** Every agent in S labelled `<repo>/<n>` that isn't in this
   tab moves here (§4.4).
3. **Adopt.** Every unlabelled agent in this tab gets labelled
   `<repo>/<n>` (§3.5).
4. **Name.** Keep the tab named `<repo>#<n> · <title>`.

On `close()`, or when a pane finds a record whose pane is gone, the
record is removed. Agents labelled for a review whose pane is gone
stay put (§3.6).

There is exactly one pane per review per space, so exactly one copy can
pull a given agent: no coordination, and no double moves. Moves only
compare labels with tabs, so they never wait on the kata server; an
unreachable server affects display, not placement. A move is in flight
at most once per agent.

### 4.4 Moving an agent between tabs (verified)

There is no "move terminal to tab" command. This sequence works, tested
on 2026-11-19 against the shipped Herdr build:

1. `terminal.park {terminal}` — removes its pane; the program keeps
   running (same pid). Herdr drops the terminal's `herdr.space` label
   and disables core keep; the command acknowledges with
   `pending: true`.
2. `hc label <t> herdr.space=<space> herdr.tab=<target tab>`.
3. `hc keep <t>` — Herdr adopts the terminal into the target tab as a
   new pane.

Relabelling `herdr.tab` without parking does nothing: the layout
document owns placement. Parking a tab's only pane closes that tab;
an agent's original tab closing when it was alone there is intended.

### 4.5 Agents finding their context

The mod ships a helper, `mods/kata/bin/kata-here` (Python 3), for the
calling agent (`$HC_TERMINAL_ID`):

| Command | Does |
|---|---|
| `kata-here` | Prints the agent's space, primary review and the reviews open in its space. |
| `kata-here open <repo>/<n>` | Opens that review's tab in the agent's space, unfocused, unless one is open, and records it in `state.json` so it's listed at once (a copy doesn't start until shown, §6.10). |
| `kata-here assign <repo>/<n>` | Opens it if needed and sets the agent's `kata.review`. |
| `kata-here unassign` | Sets `kata.review=unassigned`. |

```json
{
  "terminal": "t1",
  "space": "sp4b0b…",
  "primary": "kata-fork/1",
  "open": ["kata-fork/1", "stargate/35"]
}
```

It reads the agent's own labels (`hc get`) and `state.json`, and checks
the recorded tabs against `herdr-app ls` so a stale record never shows.
The review skill tells agents to run it, then `get_review` as today.
To change its primary review, an agent writes only a `<repo>/<number>`
it got from `list_reviews`, `create_review` or `kata-here`.

Loading it automatically is per agent and optional: Claude Code can run
`kata-here` from a prompt hook and inject the result. Other agents run
it when they need it.

### 4.6 Label validation

A label resolves when it parses as `<repo>/<number>`, the repo is a
registered kata workspace, the review exists (archived counts as
existing), and it is open in the agent's space. Anything else shows the
agent under Unassigned with an error mark:

| Case | Shown |
|---|---|
| Not `<repo>/<number>` | "Not a review: `<label>`" |
| Unknown repo | "No kata repo `<repo>`" |
| Review missing or deleted | "No review `<repo>/<number>`" |
| Review exists, not open here | "`<repo>#<number>` isn't open in this space" (with Open) |

The mod never rewrites or clears a label on its own: the label stays as
written so the cause stays visible, and only an explicit "Set primary
review" (person) or `hc label` (agent) changes it. While the kata
server is unreachable, the server-dependent rows aren't shown; the
pane says kata is offline.

### 4.7 Identity

The pane calls the kata HTTP API the way the web UI does: plain JSON
requests with no identity header. In the default `trust-client` auth
mode the server attributes those to its configured `--author` (the
person). Agents keep using MCP with `?as=ai-agent`, so drafts,
sessions and published comments separate the same way they do today.

## 5. Out of scope for version 1

- Matching the web UI (interdiff, annotations, GitHub publish,
  search, filters, syntax highlighting).
- Remote machines and shared Herdr windows.
- Kata servers in `trust-forwarded-header` or `oidc` auth mode.
- Telling agents that a review they belong to is also open in another
  space.
- Projects that group links (JIRA, PRs) per space: a separate,
  kata-independent Herdr mod.

## 6. Findings

Checked on 2026-11-19 against the shipped Herdr build, with probe mods.

1. A pane can't change its own props; no command exists for it. A
   pane's current review lives in shared state keyed by pane id
   (§4.2), with props as the starting value.
2. `close(cx)` gets no reason, so it can't tell "pane closed" from
   "app quitting". A pane that finds a record whose pane is gone
   removes it (§4.3).
3. Mod commands are reachable from outside: `herdr-app do
   <mod>.<command>` reaches a shown copy's `nextCommand()`.
4. A symlinked mod folder loads, but editing a file behind the link
   doesn't hot-reload; touching the link itself (`touch -h`) does. The
   dev install is a symlink plus a watcher that touches it.
5. `fetch` takes any method, headers and body, returns real status
   codes (404 is `ok: false`, not an exception), and with no identity
   header `/api/whoami` returns the server's `--author`.
6. A label change re-renders mod copies within a moment, with no timer:
   `hc label t7 kata.review=…` showed up in a probe's `terminals()` on
   the next render.
7. Herdr exposes no queryable state to agents beyond `herdr-app ls` and
   `status`, neither of which sees mod panes or mod state; hence
   `state.json` and `kata-here`.
8. The command palette is built from Herdr's app commands only
   (`frame/palette`); mods can't add entries, so it can't be a door
   into kata.
9. A mod pane's `props()` is exactly what the opener passed: no pane
   id, and `layout()` doesn't include tab contents. `tab.new` and
   `pane.split` do return the new pane's id to the opener.
10. A copy in a tab that hasn't been shown doesn't start until it is.
    Consequence: a review tab opened unfocused by an agent pulls its
    agents in the first time the person looks at it, not before.
11. Rows drawn inside a virtual list can't register handlers or hold
    text state: one `on_item_click(key)` per list, without modifiers,
    and an error thrown inside the row renderer blanks the list
    silently. A plain column can hold them, but lays out every row on
    every frame and lags on big files, so the diff is a virtual list
    and composing happens in a drawer under it. The press before a
    click does carry modifiers, so shift-click works through a
    mouse-down handler around the list.

## 7. Roadmap

What's built, what's next, and what's deliberately left out. Anything
that behaves differently from this list is a bug. The web UI
(`docs/kata.png`) is the reference: the pane mirrors it where that
makes sense in a narrower native pane.

### 7.1 Built (story 1, read-only pane)

- Review pane: header with repo#number, title, patchset, draft count,
  archived mark; Refresh, Open review, Close review.
- Empty state ("No open review") with a repo → review picker; archived
  reviews folded away; reviews already open in the space are marked.
- File list (file names, status letter, +/−, comment count) and one
  file's unified diff at a time, with base and tip line numbers.
- Published and draft threads: under their line when written against
  the shown endpoints; otherwise listed at the top of the file with
  their line number.
- Tab renamed to `<repo>#<n> · <title>`; panes recorded in
  `state.json` (§4.2).

### 7.2 Built (story 2, commenting)

- Click a line to draft a comment there; shift-click another line on
  the same side to make it a range. Severity: Must do (default, as in
  the web UI), Suggestion, Question. A typed draft survives moving to
  another line. Escape cancels.
- "Comment on file" in the file header.
- Threads: Reply, Reply and resolve, Resolve, Won't fix, Reopen;
  resolved threads fold to one line ("Show" / "Hide"). Your drafts
  have Edit and Delete, draft replies have Delete.
- Header: Publish N and Discard (asks once more) for your draft
  session; "New patchset" when the branch has moved (the web UI's
  Refresh). There's no reload button: the pane follows the server's
  events (§7.6), and reloads by itself when the server comes back
  after being unreachable.

Known gaps in what's built, fixed in later slices rather than bugs:
comments on another commit of a stack aren't placed inline (slice 4);
nothing refreshes by itself; long lines are clipped at the pane's
edge, as in the web UI.

### 7.3 Built (slice 1, the stacked diff)

- One virtualized list for the whole review: every file stacked in the
  web UI's tree order, each with a header row (click to collapse).
- Sidebar: "Files (N)" with ‹ i/N › and total +/−; collapsible folders
  with merged single-child paths and per-folder +/−; per-file status,
  +/−, comment count. Clicking a file jumps to its top; the current
  file follows the scroll.
- A bar above the list for the current file: collapse, full path, +/−,
  Top of file, Comment on file (rows inside the list can't hold
  buttons, §6.11).
- Comments are composed, replied to and resolved in a drawer under the
  diff. Threads show inline as cards; clicking one opens it in the
  drawer. Resolved threads anchored in the code shrink to a thin marker
  between lines.

### 7.4 Built (slices 2–8, from the web UI)

| Slice | What | Known gaps |
|---|---|---|
| 2. Reading | Split diffs by default, unified for added and deleted files, a Split/Unified toggle; word-level highlights (ported from `web/src/lib/hunkAlign.ts`, `wordDiff.ts`); Java syntax colors from a small tokenizer, since Herdr exposes none; a file's top comments fold to one line when all are resolved. | Syntax state resets at each hunk, so a block comment opened above a hunk isn't known inside it. |
| 3. Review header | Revset, bookmark, creator, the description as markdown (hideable); Archive / Unarchive, Delete (asks again). | Archive and delete need the creator or an admin (`--admin-email`), as on the server. |
| 4. Commits | Commits (N) with change id, commit, title, author, date, comment count and conflicts; selecting one scopes files and diffs to it, again clears; ‹ › cycles. New comments anchor to the shown commit. | |
| 5. Comments | ‹ N › steps through threads in display order; status (Draft, Open, Resolved) and severity filters; Both / Diff / Comments view modes. | The toolbar clips at narrow pane widths. |
| 6. Patchsets | "PS2 (latest) vs base ▾": pick any patchset and compare it to the base or an earlier one. Comments can only be written on the latest, against the base. | The web UI's per-commit interdiff pairs in compare mode aren't there. |
| 7. Search | Files, diff lines and threads, collapsed files included; ‹ i/N ›, Enter / Shift-Enter, Escape. | Commit messages and the review description aren't searched. |
| 8. Context | Expand a modified file to its full contents and back. | |

Comment placement follows the server's anchor state, as the web UI does: valid
comments keep their lines, moved and drifted ones take their new lines,
outdated ones are listed above the file. In one commit's diff, only comments
written on that change are placed.

### 7.5 Built (slice 9, agents)

- Each review pane pulls agents labelled with its review into its tab
  and labels unlabelled agents that appear there (§4.3); an Agents
  section lists the space's agents with Assign here, Move here,
  Unassign and + Agent (§3.3).
- `kata-here` and the review skill's "Inside Herdr" section (§3.4,
  §4.5).

Known gap: label errors other than format and unknown repo (a deleted
review) aren't detected yet.

### 7.6 Built (polish)

- The pane reloads when its review changes on the server (an agent
  publishing, refreshing or deleting it). Herdr's `fetch` can't stream,
  so each pane runs `curl` on `/api/events` and returns on the first
  event: event-driven, no timer.
- Right-click a line, file header or thread for its actions (comment
  here, comment on file, top of file, collapse, show full file; reply,
  resolve, won't fix, reopen, edit, delete). The menu is a popup copy
  of the mod that hands the pick back through shared state.
- The comments toolbar wraps instead of clipping in a narrow pane.
- Hovering a line shows a "+" in its gutter and tints it, as the web
  UI's comment icon does. It's a hover style on an overlay over the
  row, so Herdr draws it natively with no script work per mouse move.
- Publishing types a note into every agent on the review ("I published
  N new comments on repo#n; read them with get_review and address
  them") with `hc send`, so they pick the comments up without being
  asked. "Tell agents" next to Publish turns it off.

### 7.7 Planned

- Nothing on the list; new requests go here.

Not needed: the signed-in user, base and tip change ids in the
header, and "since you were here" (all marked optional).

### 7.8 Not planned for now

- Annotations, GitHub import and publish, creating reviews from the
  pane, the repository browser. The web UI keeps these.
- Remote machines, shared windows, non-default auth modes (§5).
