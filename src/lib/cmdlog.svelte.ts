//! Command log: records every p4 command the app runs (emitted from Rust via
//! the `p4-command` event) for the Commands view. In-memory only, capped.

import { listen } from "@tauri-apps/api/event";

export type CmdEntry = {
  n: number;
  line: string;
  ms: number;
  ok: boolean;
  refused?: boolean; // blocked by safe mode (never ran)
  time: string;
};

const CAP = 1000;
let entries = $state<CmdEntry[]>([]);
let seq = 0;
let started = false;

export const cmdlog = {
  get entries() {
    return entries;
  },
  /** Begin listening for command events (idempotent; call once at startup). */
  async start() {
    if (started) return;
    started = true;
    await listen<{ line: string; ms: number; ok: boolean }>("p4-command", (e) => {
      entries.push({
        n: ++seq,
        line: e.payload.line,
        ms: e.payload.ms,
        ok: e.payload.ok,
        time: new Date().toLocaleTimeString(),
      });
      if (entries.length > CAP) entries.splice(0, entries.length - CAP);
    });
  },
  /** Record a command that safe mode refused (it never ran). */
  refused(label: string) {
    entries.push({
      n: ++seq,
      line: `p4 ${label}`,
      ms: 0,
      ok: false,
      refused: true,
      time: new Date().toLocaleTimeString(),
    });
    if (entries.length > CAP) entries.splice(0, entries.length - CAP);
  },
  clear() {
    entries = [];
  },
};
