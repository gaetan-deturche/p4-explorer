//! Syntax highlighting for the in-app diff viewer: Shiki (VS Code's grammars +
//! dark-plus/light-plus themes) on the pure-JS regex engine (no WASM). Only the
//! languages listed here are bundled; anything else renders unhighlighted.

import { createHighlighterCore, type HighlighterCore } from "shiki/core";

/** One colored run of a highlighted line (concat of a line's runs = the line). */
export interface TokenRun {
  content: string;
  color?: string;
}

// Extension → Shiki language id. UE-flavored extras: .usf/.ush are HLSL,
// .uproject/.uplugin are JSON.
const EXT_LANG: Record<string, string> = {
  c: "c",
  h: "cpp",
  cpp: "cpp",
  cc: "cpp",
  cxx: "cpp",
  hpp: "cpp",
  inl: "cpp",
  cs: "csharp",
  rs: "rust",
  ts: "typescript",
  tsx: "tsx",
  js: "javascript",
  jsx: "jsx",
  json: "json",
  uproject: "json",
  uplugin: "json",
  py: "python",
  lua: "lua",
  hlsl: "hlsl",
  usf: "hlsl",
  ush: "hlsl",
  glsl: "glsl",
  ini: "ini",
  cfg: "ini",
  xml: "xml",
  html: "html",
  css: "css",
  md: "markdown",
  yaml: "yaml",
  yml: "yaml",
  toml: "toml",
  sh: "bash",
  bash: "bash",
  ps1: "powershell",
  bat: "bat",
  cmd: "bat",
  svelte: "svelte",
  sql: "sql",
};

// Loaders for the bundled grammars (vite splits each into its own lazy chunk).
const LANG_LOAD: Record<string, () => Promise<unknown>> = {
  c: () => import("@shikijs/langs/c"),
  cpp: () => import("@shikijs/langs/cpp"),
  csharp: () => import("@shikijs/langs/csharp"),
  rust: () => import("@shikijs/langs/rust"),
  typescript: () => import("@shikijs/langs/typescript"),
  tsx: () => import("@shikijs/langs/tsx"),
  javascript: () => import("@shikijs/langs/javascript"),
  jsx: () => import("@shikijs/langs/jsx"),
  json: () => import("@shikijs/langs/json"),
  python: () => import("@shikijs/langs/python"),
  lua: () => import("@shikijs/langs/lua"),
  hlsl: () => import("@shikijs/langs/hlsl"),
  glsl: () => import("@shikijs/langs/glsl"),
  ini: () => import("@shikijs/langs/ini"),
  xml: () => import("@shikijs/langs/xml"),
  html: () => import("@shikijs/langs/html"),
  css: () => import("@shikijs/langs/css"),
  markdown: () => import("@shikijs/langs/markdown"),
  yaml: () => import("@shikijs/langs/yaml"),
  toml: () => import("@shikijs/langs/toml"),
  bash: () => import("@shikijs/langs/bash"),
  powershell: () => import("@shikijs/langs/powershell"),
  bat: () => import("@shikijs/langs/bat"),
  svelte: () => import("@shikijs/langs/svelte"),
  sql: () => import("@shikijs/langs/sql"),
};

/** The Shiki language for a file name, or null when we don't bundle one. */
export function langForFile(name: string): string | null {
  const dot = name.lastIndexOf(".");
  if (dot < 0) return null;
  const lang = EXT_LANG[name.slice(dot + 1).toLowerCase()];
  return lang && LANG_LOAD[lang] ? lang : null;
}

let corePromise: Promise<HighlighterCore> | null = null;
const loadedLangs = new Set<string>();

// Oniguruma (WASM — full grammar fidelity; the JS engine drops rules it can't
// convert, e.g. C++ comments), falling back to the JS engine if WASM fails.
async function makeEngine() {
  try {
    const { createOnigurumaEngine } = await import("shiki/engine/oniguruma");
    return await createOnigurumaEngine(import("shiki/wasm"));
  } catch {
    const { createJavaScriptRegexEngine } = await import("shiki/engine/javascript");
    return createJavaScriptRegexEngine({ forgiving: true });
  }
}

function core(): Promise<HighlighterCore> {
  corePromise ??= makeEngine().then((engine) =>
    createHighlighterCore({
      themes: [import("@shikijs/themes/dark-plus"), import("@shikijs/themes/light-plus")],
      langs: [],
      engine,
    }),
  );
  return corePromise;
}

const MAX_CHARS = 2_000_000; // skip highlighting on huge files (diff still works)

/** Tokenize `text` into per-line colored runs, or null (too big / lang failed).
 *  Line i of the result colors line i+1 of `text` (after \r\n normalization —
 *  matching linediff's splitLines). */
export async function tokenizeLines(
  text: string,
  lang: string,
  dark: boolean,
): Promise<TokenRun[][] | null> {
  if (text.length > MAX_CHARS) return null;
  try {
    const hl = await core();
    if (!loadedLangs.has(lang)) {
      await hl.loadLanguage((await LANG_LOAD[lang]()) as never);
      loadedLangs.add(lang);
    }
    const normalized = text.replace(/\r/g, ""); // tokens must align with splitLines
    const lines = hl.codeToTokensBase(normalized, {
      lang: lang as never,
      theme: dark ? "dark-plus" : "light-plus",
    });
    return lines.map((line) => line.map((t) => ({ content: t.content, color: t.color })));
  } catch {
    return null; // unhighlighted is fine
  }
}
