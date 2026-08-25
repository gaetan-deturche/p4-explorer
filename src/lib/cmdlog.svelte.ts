//! Command log: every p4 command the app runs, for the Commands view.
//!
//! Two events from Rust: `p4-command-begin` when a command is about to run and
//! `p4-command` when it is over, paired by id. The in-flight list is the point of
//! the first one — a command that hangs never produces the second, so before this
//! the one worth seeing was the only one that never appeared.
//!
//! In-memory only (the durable copy is the session log file on disk), capped.

import { listen } from "@tauri-apps/api/event";

/** A command that has started and not yet reported back. */
export type RunningCmd = {
  id: number;
  line: string;
  /** ms since the epoch, for the elapsed display. */
  at: number;
};

export type CmdEntry = {
  n: number;
  line: string;
  ms: number;
  ok: boolean;
  err?: string; // failure text (why the command failed)
  refused?: boolean; // blocked by safe mode (never ran)
  time: string;
};

const CAP = 1000;
/** In-flight commands are never dropped on age — a stuck one must stay visible —
 *  but a runaway count would be its own problem, so the list is bounded. */
const RUNNING_CAP = 200;
let entries = $state<CmdEntry[]>([]);
let running = $state<RunningCmd[]>([]);
let seq = 0;
let started = false;

export const cmdlog = {
  get entries() {
    return entries;
  },
  /** Commands running right now, oldest first — so the one in trouble is on top. */
  get running() {
    return running;
  },
  /** Begin listening for command events (idempotent; call once at startup). */
  async start() {
    if (started) return;
    started = true;
    await listen<{ id: number; line: string }>("p4-command-begin", (e) => {
      running.push({ id: e.payload.id, line: e.payload.line, at: Date.now() });
      if (running.length > RUNNING_CAP) running.splice(0, running.length - RUNNING_CAP);
    });
    await listen<{ id: number; line: string; ms: number; ok: boolean; err?: string }>(
      "p4-command",
      (e) => {
        // Paired by id, never by text: two identical commands can be in flight at
        // once, and clearing the wrong one would hide the stuck one.
        const at = running.findIndex((r) => r.id === e.payload.id);
        if (at >= 0) running.splice(at, 1);
        entries.push({
          n: ++seq,
          line: e.payload.line,
          ms: e.payload.ms,
          ok: e.payload.ok,
          err: e.payload.err || undefined,
          time: new Date().toLocaleTimeString(),
        });
        if (entries.length > CAP) entries.splice(0, entries.length - CAP);
      },
    );
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
  /** Forget the in-flight list. The commands themselves keep running — this is
   *  for a stale entry left by an event that never arrived. */
  clearRunning() {
    running = [];
  },
  clear() {
    entries = [];
  },
};
