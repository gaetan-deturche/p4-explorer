//! Canonical list of the backend commands the app runs, with a display label
//! and whether each is read-only. This is only the DEFAULT classification for
//! safe mode (reads allowed, the rest need approval); the user's actual
//! allow-list (overrides, keyed by label) lives in $lib/safe.svelte.ts.

export interface P4Cmd {
  key: string; // Tauri command name
  label: string; // shown to the user; also the allow-list key (per logical command)
  read: boolean; // default: read commands are allowed without approval
}

export const P4_COMMANDS: P4Cmd[] = [
  // reads (allowed by default)
  { key: "p4_info", label: "info", read: true },
  { key: "p4_clients", label: "clients", read: true },
  { key: "p4_dirs", label: "dirs", read: true },
  { key: "p4_files", label: "files", read: true },
  { key: "p4_changes", label: "changes", read: true },
  { key: "p4_pending", label: "pending changes", read: true },
  { key: "p4_have_change", label: "changes (have)", read: true },
  { key: "p4_describe", label: "describe", read: true },
  { key: "p4_filelog", label: "filelog", read: true },
  { key: "p4_fstat", label: "fstat", read: true },
  { key: "p4_search", label: "search", read: true },
  { key: "p4_diff2", label: "diff", read: true },
  { key: "open_diff", label: "diff", read: true },
  { key: "p4_describe_shelved", label: "describe -S", read: true },
  { key: "p4_diff_shelved", label: "diff (shelved)", read: true },
  { key: "open_diff_shelved", label: "diff (shelved)", read: true },
  { key: "p4_opened", label: "opened", read: true },
  { key: "p4_diff_local", label: "diff", read: true },
  { key: "open_diff_local", label: "diff", read: true },
  { key: "p4_streams", label: "streams", read: true },
  { key: "p4_depots", label: "depots", read: true },
  { key: "p4_env_port", label: "set P4PORT", read: true },
  { key: "p4_login_status", label: "login -s", read: true },
  { key: "p4_ticket_user", label: "tickets", read: true },
  { key: "swarm_url", label: "swarm url", read: true },
  { key: "swarm_review", label: "swarm review", read: true },
  { key: "sync_cancel", label: "cancel sync", read: true },
  // non-reads (need approval by default)
  { key: "p4_sync", label: "sync", read: false },
  { key: "p4_sync_stream", label: "sync", read: false },
  { key: "p4_resync", label: "sync (re-sync)", read: false },
  { key: "p4_reconcile", label: "reconcile", read: false },
  { key: "p4_switch", label: "switch", read: false },
  { key: "p4_submit", label: "submit", read: false },
  { key: "p4_shelve", label: "shelve", read: false },
  { key: "p4_shelve_delete", label: "shelve -d", read: false },
  { key: "p4_request_review", label: "request review", read: false },
  { key: "p4_revert", label: "revert", read: false },
  { key: "p4_revert_keep", label: "revert -k", read: false },
  { key: "p4_reopen", label: "reopen", read: false },
  { key: "p4_new_changelist", label: "new changelist", read: false },
  { key: "p4_set_description", label: "change description", read: false },
  { key: "p4_login", label: "login", read: false },
  { key: "p4_trust", label: "trust", read: false },
];

export const P4_CMD_BY_KEY: Record<string, P4Cmd> = Object.fromEntries(
  P4_COMMANDS.map((c) => [c.key, c]),
);

/** Unique logical commands (by label) for the Safe tab, non-reads first. */
export const P4_COMMAND_LIST: { label: string; read: boolean }[] = (() => {
  const seen = new Map<string, boolean>();
  for (const c of P4_COMMANDS) if (!seen.has(c.label)) seen.set(c.label, c.read);
  return [...seen.entries()]
    .map(([label, read]) => ({ label, read }))
    .sort((a, b) => Number(a.read) - Number(b.read) || a.label.localeCompare(b.label));
})();
