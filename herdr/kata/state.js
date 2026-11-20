import { appHome, readTextFile, writeTextFile } from "herdr";
import * as fs from "fs/promises";

// The mod's own record of review panes, readable by agents: { version, panes: { <pane>: { key, space, tab, repo, review } } }.
const directory = () => `${appHome()}/data/kata`;
const file = () => `${directory()}/state.json`;

function parse(text) {
  try {
    const state = JSON.parse(text);
    if (state && typeof state.panes === "object") return state;
  } catch {}
  return { version: 1, panes: {} };
}

export async function readState() {
  const text = await readTextFile(file()).catch(() => null);
  return { text, state: parse(text) };
}

// livePanes: pane ids from panes(), read by the caller before awaiting. Empty means "unknown": nothing is pruned.
export async function updateState(livePanes, change) {
  await fs.mkdir(directory(), { recursive: true });
  for (let attempt = 0; ; attempt++) {
    const { text, state } = await readState();
    if (livePanes.size > 0) {
      for (const pane of Object.keys(state.panes)) {
        if (!livePanes.has(pane)) delete state.panes[pane];
      }
    }
    change(state);
    try {
      await writeTextFile(file(), `${JSON.stringify(state, null, 2)}\n`, text);
      return state;
    } catch (e) {
      if (attempt >= 4) throw e;
    }
  }
}
