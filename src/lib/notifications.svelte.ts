//! Notification history: every transient notice/error banner the app shows is
//! also recorded here (in-memory, capped) so past messages can be reviewed in
//! the Notifications view instead of vanishing when the banner auto-clears.

export type NoteEntry = {
  n: number;
  type: "notice" | "error";
  message: string;
  time: string;
};

const CAP = 200;
let entries = $state<NoteEntry[]>([]);
let seq = 0;

export const notifications = {
  get entries() {
    return entries;
  },
  /** Record a message (skips empties, e.g. banner-clear calls). */
  add(type: "notice" | "error", message: string) {
    const m = (message ?? "").trim();
    if (!m) return;
    entries.push({ n: ++seq, type, message: m, time: new Date().toLocaleTimeString() });
    if (entries.length > CAP) entries.splice(0, entries.length - CAP);
  },
  clear() {
    entries = [];
  },
};
