# Working in this repo

Three repo-specific rules that ride alongside the global CLAUDE.md
guidance. Apply them on every feature change.

## Wire new UI features into the guided walkthrough

`?demo=1` in the demo binary launches an in-app tour through the
core review features. When a change adds (or materially alters) a
UI affordance that a first-time user would benefit from being
shown, extend the tour in the same commit.

Mechanics:

- The tour script is `web/src/demo/script.ts`. Each step is a
  `TourStep` (id, target, placement, title, body). Insert new
  steps in narrative order — see the docstring at the top of the
  file for the canonical flow (list → review chrome → diff →
  comments → annotations → folding → filters / search / nav →
  done).
- Tag the relevant DOM element with `data-tour="<name>"`. The
  step references it via `target: '[data-tour=<name>]'`. The
  attribute is the spotlight contract; component class names are
  not — those refactor over time, the `data-tour` doesn't.
- If the new feature needs specific seeded data to demo, extend
  `crates/kata-demo/src/lib.rs` (the seeder) in the same change.
  Adding a step that points at content the demo doesn't produce
  is a bug — the spotlight lands on an empty page.
- Update the closing "Done" step's body list if the change
  introduces a feature category the user should leave with a
  mental model of.

Skip this rule only when the change is invisible to the demo
audience (backend plumbing, dev-only tooling, internal
refactors).

## Update the README + deployment doc when their scope changes

`README.md` is the public-facing pitch and getting-started guide.
The longer team-deployment reference lives in `docs/deploying.md`.
When a change touches any of the following, update the relevant
file in the same commit:

- The architecture table (`Crate | Purpose`) in README — when a
  crate is added, removed, or its purpose meaningfully shifts.
- The Quick Start command(s) in README — when CLI flags change,
  defaults change, or new subcommands are added (e.g. the
  `kata demo` block).
- Build / runtime requirements in README — when a runtime
  dependency is added or removed (e.g. dropping the `jj` binary
  requirement when the libjj backend landed).
- `docs/deploying.md` — when auth / TLS / proxy / OIDC / API-token
  behaviour changes. The README's one-paragraph "Run it for your
  repo" pointer at the doc rarely needs touching, but verify the
  link still reads correctly when you restructure.
- The Reviewing-with-agents (MCP) section in README — when MCP
  tools are added, removed, or renamed.

A frontend-only UX polish typically doesn't need either file. A
new subcommand, a new deployment mode, a changed default, or a
new crate does.

## Keep the product spec consistent

`docs/SPEC.md` describes how the product behaves from a user's
perspective. It is the source of truth for behaviour decisions
that have already been made — both what the product does and
what it deliberately doesn't.

When adding or changing a feature:

1. **Read the relevant section first.** The spec often pre-
   declares scope cuts ("commenting on specific sides of a
   conflict is out of scope for this iteration" in §5.6;
   "force-publishing" in §19's out-of-scope list). A change
   that contradicts the spec is either a scope expansion that
   needs the spec updated, or a bug — clarify which before
   writing code.
2. **Update the spec in the same commit** when the change
   introduces, removes, or alters user-visible behaviour. The
   spec should describe the product as it ships in this commit,
   not the product as it shipped before.
3. **Reuse existing section anchors** — §3.1 (header), §5
   (reading a diff), §6 (commenting), §10 (patchsets), §12
   (filtering / navigation / search), §18 (URL structure),
   §19 (out of scope), §20 (design principles) — and add new
   subsections (e.g. §12.5) rather than churning numbering.
4. **If the change removes a previously-promised affordance**,
   that's a real spec edit — don't paper over it. Note the
   removal and (if relevant) why.

Mention the spec update explicitly in the commit message's body
so the change history reads "behaviour + docs landed together"
rather than the docs trailing.
