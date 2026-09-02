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
  { key: "p4_users", label: "users", read: true },
  { key: "p4_dirs", label: "dirs", read: true },
  { key: "p4_files", label: "files", read: true },
  { key: "p4_changes", label: "changes", read: true },
  { key: "p4_pending", label: "pending changes", read: true },
  { key: "p4_have_change", label: "changes (have)", read: true },
  { key: "p4_changes_exact", label: "changes (have)", read: true },
  { key: "p4_status", label: "status (offline changes)", read: true },
  { key: "cancel_offline_scan", label: "cancel offline scan", read: true },
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
  { key: "p4_diff_local_forced", label: "diff", read: true },
  { key: "open_diff_local", label: "diff", read: true },
  { key: "p4_streams", label: "streams", read: true },
  { key: "p4_depots", label: "depots", read: true },
  { key: "p4_env_port", label: "set P4PORT", read: true },
  { key: "p4_login_status", label: "login -s", read: true },
  { key: "p4_ticket_user", label: "tickets", read: true },
  { key: "p4_ticket_value", label: "tickets", read: true },
  { key: "swarm_url", label: "swarm url", read: true },
  { key: "swarm_reviews_for", label: "swarm review", read: true },
  { key: "swarm_reviews", label: "swarm reviews", read: true },
  { key: "swarm_shelved_no_review", label: "swarm reviews", read: true },
  { key: "review_patch", label: "describe -S (review patch)", read: true },
  { key: "sync_cancel", label: "cancel sync", read: true },
  { key: "export_patch", label: "diff (export patch)", read: true },
  { key: "stash_save", label: "diff (take a stash)", read: true },
  { key: "stash_list", label: "stashes", read: true },
  { key: "stash_patch_file", label: "stash (patch)", read: true },
  { key: "stash_patch", label: "stash (patch)", read: true },
  { key: "stash_rename", label: "stash (rename)", read: true },
  { key: "pick_patch_file", label: "pick patch file", read: true },
  { key: "preview_patch", label: "patch (preview)", read: true },
  { key: "resolve_needed", label: "fstat (needs resolve)", read: true },
  { key: "watch_file", label: "watch file", read: true },
  { key: "unwatch_file", label: "watch file (stop)", read: true },
  { key: "merge_reload", label: "resolve (reload)", read: true },
  { key: "merge_start_resolve", label: "resolve (prepare merge)", read: true },
  { key: "merge_start_patch", label: "patch (prepare merge)", read: true },
  { key: "print_to_temp", label: "print (open in editor)", read: true },
  { key: "diff_pair_rev", label: "diff", read: true },
  { key: "diff_pair_shelved", label: "diff (shelved)", read: true },
  { key: "diff_pair_local", label: "diff", read: true },
  { key: "open_unreal_diff", label: "diff (unreal)", read: true },
  // non-reads (need approval by default)
  { key: "p4_sync", label: "sync", read: false },
  { key: "p4_sync_stream", label: "sync", read: false },
  { key: "p4_resync", label: "sync (re-sync)", read: false },
  { key: "p4_flush", label: "flush (repair sync record)", read: false },
  { key: "p4_reconcile", label: "reconcile", read: false },
  { key: "p4_reconcile_files", label: "reconcile (check out)", read: false },
  { key: "p4_clean", label: "clean (revert offline)", read: false },
  { key: "apply_patch", label: "patch (apply)", read: false },
  { key: "stash_delete", label: "stash (delete)", read: false },
  { key: "review_copy_files", label: "review (copy shelved files)", read: false },
  { key: "write_local_file", label: "write file (diff editor)", read: false },
  { key: "merge_save", label: "resolve (save merge)", read: false },
  { key: "merge_external", label: "resolve (P4MERGE)", read: false },
  { key: "p4_switch", label: "switch", read: false },
  { key: "p4_submit", label: "submit", read: false },
  { key: "p4_shelve", label: "shelve", read: false },
  { key: "p4_shelve_delete", label: "shelve -d", read: false },
  { key: "p4_unshelve", label: "unshelve", read: false },
  { key: "p4_request_review", label: "request review", read: false },
  { key: "p4_revert", label: "revert", read: false },
  // Unlisted commands default to read=true, so anything that WRITES has to be
  // here or safe mode waves it through. p4_revert_local reverts open files and
  // cleans offline ones — the most destructive thing in the Pending tab.
  { key: "p4_revert_local", label: "revert / clean (discard local changes)", read: false },
  { key: "p4_client_spec", label: "client -o (read a workspace spec)", read: true },
  { key: "p4_client_save", label: "client -i (change a workspace)", read: false },
  { key: "p4_client_delete", label: "client -d (delete a workspace)", read: false },
  { key: "p4_client_rename", label: "renameclient", read: false },
  { key: "p4_unchanged_open", label: "diff -sr (opened but unchanged)", read: true },
  { key: "p4_sync_blockers", label: "fstat (why a sync was refused)", read: true },
  { key: "p4_case_twins", label: "dirs/files (case-clash check)", read: true },
  { key: "session_log_path", label: "session log path", read: true },
  { key: "p4_shelved_changes", label: "changes -s shelved", read: true },
  { key: "p4_file_holders", label: "fstat (who has it)", read: true },
  { key: "p4_open_files", label: "edit / add / delete", read: false },
  { key: "p4_move_file", label: "move (rename)", read: false },
  { key: "p4_annotate", label: "annotate (blame)", read: true },
  { key: "open_blame_window", label: "annotate (blame)", read: true },
  { key: "open_file_history_window", label: "file history (window)", read: true },
  { key: "p4_revert_change", label: "revert -c (whole changelist)", read: false },
  { key: "p4_delete_change", label: "change -d (delete changelist)", read: false },
  { key: "p4_undo_preview", label: "undo -n (preview)", read: true },
  { key: "p4_undo_change", label: "undo (into a new changelist)", read: false },
  { key: "p4_revert_keep", label: "revert -k", read: false },
  { key: "p4_reopen", label: "reopen", read: false },
  { key: "p4_new_changelist", label: "new changelist", read: false },
  { key: "p4_new_client", label: "new workspace", read: false },
  { key: "p4_set_description", label: "change description", read: false },
  { key: "p4_login", label: "login", read: false },
  { key: "p4_trust", label: "trust", read: false },
  { key: "swarm_review_detail", label: "swarm review (detail)", read: true },
  { key: "swarm_transitions", label: "swarm review (allowed actions)", read: true },
  { key: "review_version_files", label: "describe / fstat (compare review versions)", read: true },
  { key: "diff_pair_versions", label: "print (diff review versions)", read: true },
  { key: "open_review_window", label: "review (window)", read: true },
  { key: "review_job", label: "review (window)", read: true },
  // Changes the review's state on the server, for everyone. Not a p4 write, but
  // a write all the same, and the one action in the review window that is not
  // undoable from the app.
  { key: "swarm_set_state", label: "swarm review (approve / reject)", read: false },
  { key: "swarm_comments", label: "swarm comments (read)", read: true },
  { key: "diff_job", label: "diff (window)", read: true },
  { key: "comments_notify_immediately", label: "swarm comments (read)", read: true },
  // Asking Swarm which task states it accepts is a REJECTED patch — it changes
  // nothing, which is why it counts as a read.
  { key: "swarm_task_transitions", label: "swarm comments (read)", read: true },
  // Posting mails everyone on the review the moment it lands (this server has no
  // /comments/notify route to batch with), so it is a non-read twice over.
  { key: "swarm_add_comment", label: "swarm comment (post)", read: false },
  { key: "swarm_edit_comment", label: "swarm comment (edit / task / archive)", read: false },
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
