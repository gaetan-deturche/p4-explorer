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

let hooks: Hooks | null = null;
let pending: Update | null = null;
let state = $state<UpdateState | null>(null);

export const updates = {
  init(h: Hooks) {
    hooks = h;
  },
  get state(): UpdateState | null {
    return state;
  },
  dismiss() {
    state = null;
  },

  /** Check for a newer release. `silent` suppresses the "up to date" / dev notices. */
  async check(silent: boolean) {
    if (!hooks) return;
    if (!hooks.isRelease()) {
      if (!silent) hooks.notify("This is a development build — auto-update is disabled.");
      return;
    }
    try {
      const update = await check();
      if (update) {
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
