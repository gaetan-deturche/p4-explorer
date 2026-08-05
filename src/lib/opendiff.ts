//! One path for "open a diff outside the list": Unreal's asset-diff tool for a
//! `.uasset`, else the in-app diff window or the external P4DIFF tool, per
//! Options → Editor. History, Pending and Reviews all double-click into this;
//! they differ ONLY in where the two sides come from, which is what `DiffSource`
//! carries. Keeping a copy per tab is how the Reviews tab shipped without the
//! Unreal branch (v0.15.0).

import { p4, openDiffWindow, type DiffPair, type P4Conn } from "$lib/p4";
import { editor, isUnrealAsset, unrealAssetName } from "$lib/editor.svelte";

/** Which two sides to diff — `rev`: a submitted revision vs its predecessor
 *  (empty left side when it was added); `shelved`: the shelf vs its base
 *  revision; `local`: the live workspace file vs the synced (#have) revision. */
export type DiffSource =
  | { kind: "rev"; file: string; rev: number }
  | { kind: "shelved"; file: string; rev: number; change: string }
  | { kind: "local"; file: string };

/** Materialize both sides on disk (Unreal's diff tool and the in-app window
 *  both take files, not a command). */
function diffPair(conn: P4Conn, src: DiffSource): Promise<DiffPair> {
  switch (src.kind) {
    case "rev":
      return p4.diffPairRev(conn, src.file, src.rev);
    case "shelved":
      return p4.diffPairShelved(conn, src.file, src.rev, src.change);
    case "local":
      return p4.diffPairLocal(conn, src.file);
  }
}

/** Hand the same two sides to the configured external P4DIFF tool instead. */
function openExternal(conn: P4Conn, src: DiffSource): Promise<void> {
  switch (src.kind) {
    case "rev":
      return p4.openDiff(conn, src.file, src.rev);
    case "shelved":
      return p4.openDiffShelved(conn, src.file, src.rev, src.change);
    case "local":
      return p4.openDiffLocal(conn, src.file);
  }
}

/** Open a file's diff in whichever tool applies. Never throws: a failure is
 *  reported through `setNotice`, as every call site did on its own. */
export async function openDiff(
  conn: P4Conn,
  src: DiffSource,
  setNotice: (m: string, ms?: number) => void,
): Promise<void> {
  try {
    if (isUnrealAsset(src.file)) {
      setNotice("Opening Unreal diff…", 15000); // instant feedback; replaced on completion
      const pair = await diffPair(conn, src);
      const mode = await p4.openUnrealDiff(
        conn, pair.left, pair.right, unrealAssetName(src.file), pair.leftLabel, pair.rightLabel,
      );
      setNotice(
        mode === "nocompare"
          ? // Empty counterpart: the asset is new (or gone) in this change, so
            // there is nothing for Unreal's asset diff to compare.
            "No earlier revision of this asset to compare."
          : mode === "remote"
            ? "Diff opened in the running Unreal Editor."
            : "Launching Unreal Editor for the diff — this takes a moment…",
        8000,
      );
    } else if (editor.diffTool === "inapp") {
      await openDiffWindow(await diffPair(conn, src));
    } else {
      await openExternal(conn, src);
    }
  } catch (e) {
    setNotice(String(e), 5000);
  }
}
