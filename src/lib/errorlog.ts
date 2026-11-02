//! Uncaught errors of a window, written into the session log. The release build
//! has no console, and an error thrown while the UI swaps a panel leaves it half
//! updated (a History tab over the Pending list) with no other trace.

import { appLog } from "$lib/p4";

/** At most this many lines a minute, so an error thrown every frame cannot
 *  flood the log; the same message is written once per minute. */
const PER_MINUTE = 20;

let windowStart = 0;
let written = 0;
const seen = new Map<string, number>();

function describe(e: unknown): string {
  if (e instanceof Error) return e.stack || `${e.name}: ${e.message}`;
  try {
    return typeof e === "string" ? e : JSON.stringify(e);
  } catch {
    return String(e);
  }
}

function write(kind: string, e: unknown) {
  const now = Date.now();
  const text = describe(e);
  if ((seen.get(text) ?? 0) > now - 60_000) return;
  if (now - windowStart > 60_000) {
    windowStart = now;
    written = 0;
  }
  if (written++ >= PER_MINUTE) return;
  seen.set(text, now);
  const where = window.location.pathname + window.location.search.slice(0, 80);
  appLog(`ui ${kind} in ${where}: ${text.replace(/\r?\n/g, "\n               ")}`);
}

/** Install once per window. */
export function logUncaughtErrors() {
  window.addEventListener("error", (e) => write("error", e.error ?? e.message));
  window.addEventListener("unhandledrejection", (e) => write("unhandled rejection", e.reason));
  // Svelte reports some errors (effect loops, failed updates) through console.error.
  const original = console.error.bind(console);
  console.error = (...args: unknown[]) => {
    original(...args);
    write("console.error", args.length === 1 ? args[0] : args.map(describe).join(" "));
  };
}
