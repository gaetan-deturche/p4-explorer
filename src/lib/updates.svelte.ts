//! Auto-update feature store: check GitHub for a newer release and install it.
//! Release-only (dev builds skip). A singleton wired once via `init()`.

import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";

type Hooks = {
  isRelease: () => boolean;
  appVersion: () => string;
  notify: (msg: string) => void;
  warn: (msg: string) => void;
};

type UpdateState = {
  version: string;
  notes: string;
  phase: "available" | "downloading" | "error";
  downloaded: number;
  total: number;
  message: string;
};

/** Releases come in bursts, so re-check often enough to catch one the same hour. */
const AUTO_CHECK_MS = 60 * 60 * 1000;

let hooks: Hooks | null = null;
let pending: Update | null = null;
let state = $state<UpdateState | null>(null);
let timer: ReturnType<typeof setInterval> | null = null;
let dismissedVersion: string | null = null;

export const updates = {
  init(h: Hooks) {
    hooks = h;
  },
  get state(): UpdateState | null {
    return state;
  },
  dismiss() {
    if (state) dismissedVersion = state.version;
    state = null;
  },

  /** Re-check periodically while the app stays open. Idempotent; release builds only. */
  startAutoCheck() {
    if (timer || !hooks?.isRelease()) return;
    timer = setInterval(() => {
      if (state) return; // already surfaced or downloading — don't clobber the dialog
      void updates.check(true, true);
    }, AUTO_CHECK_MS);
  },

  /**
   * Check for a newer release. `silent` suppresses the "up to date" / dev notices;
   * `auto` marks a timer-driven check, which won't re-surface a dismissed version.
   */
  async check(silent: boolean, auto = false) {
    if (!hooks) return;
    if (!hooks.isRelease()) {
      if (!silent) hooks.notify("This is a development build — auto-update is disabled.");
      return;
    }
    try {
      const update = await check();
      if (update) {
        if (auto && update.version === dismissedVersion) return; // user already said no
        pending = update;
        state = {
          version: update.version,
          notes: update.body ?? "",
          phase: "available",
          downloaded: 0,
          total: 0,
          message: "",
        };
      } else if (!silent) {
        hooks.notify(`You're on the latest version (v${hooks.appVersion()}).`);
      }
    } catch (e) {
      if (!silent) hooks.warn(`Update check failed: ${e}`);
    }
  },

  /** Download + install the pending update, then relaunch. */
  async install() {
    if (!pending || !state) return;
    state.phase = "downloading";
    let downloaded = 0;
    try {
      await pending.downloadAndInstall((event) => {
        if (!state) return;
        if (event.event === "Started") state.total = event.data.contentLength ?? 0;
        else if (event.event === "Progress") {
          downloaded += event.data.chunkLength;
          state.downloaded = downloaded;
        }
      });
      await relaunch();
    } catch (e) {
      if (state) {
        state.phase = "error";
        state.message = String(e);
      }
    }
  },
};
