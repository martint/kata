import { agents, terminals, panes, hc, run } from "herdr";

// An agent's primary review is its terminal's kata.review label: "<repo>/<number>", or "unassigned" (set by
// Unassign) or no label at all. A review pane pulls the agents labelled with its review into its tab, and labels
// unlabelled agents that appear in its tab; "unassigned" keeps an agent out of that.
export const LABEL = "kata.review";
export const UNASSIGNED = "unassigned";
export const reviewKey = (repo, number) => `${repo}/${number}`;

export function labelsByAgent() {
  const labels = new Map();
  for (const terminal of terminals()) labels.set(terminal.address, terminal.labels?.[LABEL] ?? null);
  return labels;
}

// "ok" with { repo, review }, "unassigned", or an error the sidebar shows next to the agent.
export function checkLabel(label, repos, openHere) {
  if (label == null || label === "" || label === UNASSIGNED) return { kind: "unassigned" };
  const match = /^(.+)\/(\d+)$/.exec(label);
  if (!match) return { kind: "error", message: `Not a review: ${label}` };
  const [, repo, number] = match;
  if (repos && !repos.some((r) => r.name === repo)) return { kind: "error", message: `No kata repo ${repo}` };
  if (openHere && !openHere.has(label)) return { kind: "error", message: `${repo}#${number} isn't open in this space`, repo, review: Number(number) };
  return { kind: "ok", repo, review: Number(number) };
}

export function agentsInSpace(space) {
  return agents().filter((a) => a.space === space);
}

export async function setLabel(address, value) {
  const result = await hc(["label", address, `${LABEL}=${value}`]);
  if (result.code !== 0) throw new Error((result.stderr || result.stdout).trim() || `Couldn't label ${address}`);
}

const sleep = (cx, ms) => new Promise((resolve) => cx.timer.after(ms, () => resolve()));

// Herdr has no "move terminal to tab": park it (its pane goes, the program keeps running), relabel its space and
// tab, and keep it, which adopts it into that tab as a new pane. Relabelling alone does nothing.
export async function moveAgent(cx, address, space, tab) {
  run("terminal.park", { terminal: address });
  for (let i = 0; i < 30 && panes().some((p) => p.terminal === address); i++) await sleep(cx, 100);
  const label = await hc(["label", address, `herdr.space=${space}`, `herdr.tab=${tab}`]);
  if (label.code !== 0) throw new Error((label.stderr || label.stdout).trim() || `Couldn't move ${address}`);
  const keep = await hc(["keep", address]);
  if (keep.code !== 0) throw new Error((keep.stderr || keep.stdout).trim() || `Couldn't keep ${address}`);
}
