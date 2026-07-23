//! Safe mode: when enabled, every p4 command that isn't allowed must be approved
//! by the user before it runs. Whether a command is allowed comes from the
//! per-command DEFAULTS in $lib/p4cmds (reads allowed) with USER OVERRIDES on
//! top — both the enabled flag and the overrides (keyed by command label) are
//! persisted and fully editable from Options → Safe.

import { cmdlog } from "$lib/cmdlog.svelte";
import { P4_CMD_BY_KEY } from "$lib/p4cmds";

type Answer = { allow: boolean; always: boolean };
type Request = { label: string; resolve: (a: Answer) => void };

const K_ENABLED = "safe:enabled";
const K_OVERRIDES = "safe:overrides";
function ls(k: string): string | null {
  try {
    return localStorage.getItem(k);
  } catch {
    return null;
  }
}
function save(k: string, v: string): void {
  try {
    localStorage.setItem(k, v);
  } catch {
    /* storage unavailable */
  }
}
function initialOverrides(): Record<string, boolean> {
  try {
    return JSON.parse(ls(K_OVERRIDES) || "{}");
  } catch {
    return {};
  }
}

let enabled = $state(ls(K_ENABLED) === "1");
// Per-label overrides of the default allow state (absent = use the default).
let overrides = $state<Record<string, boolean>>(initialOverrides());
let queue = $state<Request[]>([]);

function defaultRead(label: string): boolean {
  // A label's default: read if any command with that label is a read.
  for (const c of Object.values(P4_CMD_BY_KEY)) if (c.label === label) return c.read;
  return false;
}

export const safe = {
  get enabled() {
    return enabled;
  },
  setEnabled(v: boolean) {
    enabled = v;
    save(K_ENABLED, v ? "1" : "0");
  },

  /** Whether a logical command (by label) is currently allowed without asking:
   *  the user override if set, otherwise the default (read = allowed). */
  isAllowed(label: string, read = defaultRead(label)): boolean {
    return label in overrides ? overrides[label] : read;
  },
  /** Set/clear a user override for a command label. */
  setAllowed(label: string, v: boolean) {
    overrides = { ...overrides, [label]: v };
    save(K_OVERRIDES, JSON.stringify(overrides));
  },
  /** Drop all overrides (back to defaults). */
  resetAllows() {
    overrides = {};
    save(K_OVERRIDES, "{}");
  },
  get overrides() {
    return overrides;
  },

  get current(): Request | null {
    return queue[0] ?? null;
  },
  answer(a: Answer) {
    const r = queue[0];
    if (!r) return;
    queue = queue.slice(1);
    r.resolve(a);
  },

  /** Run the backend call `fn` for Tauri command `cmd`, gated by safe mode. */
  async guard<T>(cmd: string, fn: () => Promise<T>): Promise<T> {
    const meta = P4_CMD_BY_KEY[cmd];
    const label = meta?.label ?? cmd;
    const allowed = safe.isAllowed(label, meta?.read ?? true);
    if (!enabled || allowed) return fn();
    const ans = await new Promise<Answer>((resolve) => {
      queue = [...queue, { label, resolve }];
    });
    if (!ans.allow) {
      cmdlog.refused(label); // reflect the blocked command in the Commands view
      throw new Error(`"${label}" was not approved (safe mode is on).`);
    }
    if (ans.always) safe.setAllowed(label, true);
    return fn();
  },
};
