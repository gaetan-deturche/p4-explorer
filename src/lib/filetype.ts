//! Perforce file types, in words.
//!
//! A type is a base plus modifiers — `text+C`, `binary+l`, `text+wS3` — and the
//! two halves answer different questions: the base says how p4 treats the
//! CONTENT, each modifier says either how the depot STORES it (no client-side
//! effect at all) or how it BEHAVES in your workspace. That distinction is the
//! whole point of the tooltip: `+C` changes nothing for you, `+w` changes
//! whether a tool can rewrite the file behind your back.

const BASES: Record<string, string> = {
  text: "Text: line-ending translation on sync, diffable, stored as deltas by default.",
  binary: "Binary: no translation, stored whole (compressed) per revision.",
  symlink: "Symbolic link.",
  unicode: "Unicode text: translated to the client's character set.",
  utf8: "UTF-8 text: stored as UTF-8, BOM handled by p4.",
  utf16: "UTF-16 text: stored as UTF-8, delivered as UTF-16.",
  apple: "AppleSingle/Double: Mac resource + data fork pair.",
  resource: "Mac resource fork.",
};

/** How the depot stores it — invisible in the workspace. */
const STORAGE: Record<string, string> = {
  C: "+C: the depot keeps a full compressed copy of every revision instead of deltas. Storage only — no effect on your workspace.",
  D: "+D: the depot keeps deltas in RCS format. This is the default for text.",
  F: "+F: the depot keeps the full file per revision, uncompressed. Storage only.",
  S: "+S: the depot keeps ONLY the head revision — older ones are discarded, so there is no history to go back to.",
};

/** How the file behaves in the workspace, or when opening it. */
const BEHAVIOUR: Record<string, string> = {
  w: "+w: always writable — p4 never sets the read-only flag, so any tool can change it without a checkout. Local edits show up as offline changes, and a sync can overwrite them.",
  l: "+l: exclusive open — only one person at a time can have it checked out. The usual choice for binary assets that cannot be merged.",
  m: "+m: the modification time on disk is preserved from the depot instead of being set to the sync time.",
  x: "+x: the executable bit is set when synced.",
  k: "+k: $Keyword$ lines (Id, Header, Author, Date, Revision…) are expanded on sync — which means the file on disk differs from the archived text by design.",
  ko: "+ko: only $Id$ and $Header$ are expanded.",
  kx: "+kx: keyword expansion, plus the executable bit.",
  X: "+X: an archive trigger delivers the content; the depot does not hold it directly.",
};

/** Split `text+wS3` into its base and its modifiers (`S3` keeps its count). */
export function splitType(type: string): { base: string; mods: string[] } {
  const [base, rest] = type.split("+", 2);
  if (!rest) return { base, mods: [] };
  const mods: string[] = [];
  for (let i = 0; i < rest.length; i++) {
    // `S` may carry a revision count, and `k` a variant letter (ko, kx).
    if (rest[i] === "S") {
      let n = "";
      while (i + 1 < rest.length && /\d/.test(rest[i + 1])) n += rest[++i];
      mods.push("S" + n);
    } else if (rest[i] === "k" && (rest[i + 1] === "o" || rest[i + 1] === "x")) {
      mods.push("k" + rest[++i]);
    } else {
      mods.push(rest[i]);
    }
  }
  return { base, mods };
}

/** The tooltip for a file type: what it is, then one line per modifier.
 *  Unknown types are echoed rather than guessed at. */
export function describeFileType(type: string): string {
  const t = (type ?? "").trim();
  if (!t) return "";
  const { base, mods } = splitType(t);
  const lines: string[] = [t, BASES[base] ?? `Base type "${base}".`];
  for (const m of mods) {
    const key = m.startsWith("S") ? "S" : m;
    const text = STORAGE[key] ?? BEHAVIOUR[key];
    if (!text) {
      lines.push(`+${m}: (modifier not recognised)`);
    } else if (m.length > 1 && key === "S") {
      // +S<n>: the count is the interesting part.
      lines.push(`+${m}: the depot keeps only the newest ${m.slice(1)} revisions; older ones are discarded.`);
    } else {
      lines.push(text);
    }
  }
  return lines.join("\n");
}
