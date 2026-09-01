//! Syntax highlighting for the in-app diff viewer: Shiki (VS Code's grammars +
//! dark-plus/light-plus themes) on the pure-JS regex engine (no WASM). Only the
//! languages listed here are bundled; anything else renders unhighlighted.

import { createHighlighterCore, type HighlighterCore } from "shiki/core";
import { TokenSession, type Tokenizer } from "./tokensession";

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

// --- a tokenizer over one language ------------------------------------------

/** Shiki as `TokenSession` needs it: state in, state out, runs for a stretch of
 *  lines. Both calls take the SAME options as the one-shot path did, plus the
 *  `grammarState` that lets tokenizing resume mid-file. */
export async function makeTokenizer(lang: string, dark: boolean): Promise<Tokenizer | null> {
  try {
    const hl = await core();
    if (!loadedLangs.has(lang)) {
      await hl.loadLanguage((await LANG_LOAD[lang]()) as never);
      loadedLangs.add(lang);
    }
    const theme = dark ? "dark-plus" : "light-plus";
    return {
      stateAfter: (text, from) =>
        hl.getLastGrammarState(text, {
          lang: lang as never,
          theme,
          grammarState: (from ?? undefined) as never,
        }),
      tokens: (text, from) =>
        hl
          .codeToTokensBase(text, {
            lang: lang as never,
            theme,
            grammarState: (from ?? undefined) as never,
          })
          .map((line) => line.map((tk) => ({ content: tk.content, color: tk.color }))),
    };
  } catch {
    return null; // unhighlighted is fine
  }
}

// --- off the main thread ----------------------------------------------------

/** One file being coloured, a window at a time.
 *
 *  The state lives wherever the tokenizing happens — in the worker when there is
 *  one, since a `GrammarState` is a live object that cannot be posted between
 *  threads. Either way the caller sees the same three methods. */
export interface SyntaxSession {
  readonly lineCount: number;
  /** Coloured runs for `count` lines from `from` (0-based). */
  window(from: number, count: number): Promise<TokenRun[][]>;
  close(): void;
}

type Reply = { req: number; lines?: number; runs?: TokenRun[][] };

let worker: Worker | null = null;
let workerDead = false;
let nextReq = 1;
const pending = new Map<number, (r: Reply) => void>();

function ensureWorker(): Worker | null {
  if (worker || workerDead) return worker;
  try {
    worker = new Worker(new URL("./syntax.worker.ts", import.meta.url), { type: "module" });
    worker.onmessage = (e: MessageEvent<Reply>) => {
      const done = pending.get(e.data.req);
      if (!done) return; // a reply nobody is waiting for any more
      pending.delete(e.data.req);
      done(e.data);
    };
    worker.onerror = () => {
      // Whatever is in flight resolves empty rather than hanging forever.
      for (const done of pending.values()) done({ req: 0 });
      pending.clear();
      worker?.terminate();
      worker = null;
      workerDead = true;
    };
  } catch {
    workerDead = true;
  }
  return worker;
}

function ask(w: Worker, msg: Record<string, unknown>): Promise<Reply> {
  const req = nextReq++;
  return new Promise((resolve) => {
    pending.set(req, resolve);
    w.postMessage({ ...msg, req });
  });
}

/** Open a session over `text`. Null when the language is unknown, the file is
 *  too big to bother with, or the grammar failed to load — callers render
 *  uncoloured, as they always did. */
export async function openSyntax(
  text: string,
  lang: string,
  dark: boolean,
): Promise<SyntaxSession | null> {
  if (text.length > MAX_CHARS) return null;
  const w = ensureWorker();
  if (w) {
    const id = nextId++;
    const opened = await ask(w, { k: "open", id, text, lang, dark });
    const lines = opened.lines ?? -1;
    if (lines < 0) return null;
    return {
      lineCount: lines,
      window: async (from, count) => (await ask(w, { k: "win", id, from, count })).runs ?? [],
      close: () => w.postMessage({ k: "close", id, req: 0 }),
    };
  }
  // No worker: the same session, on this thread. Slower to open a big file, but
  // still only a window's worth of colouring per scroll.
  const tk = await makeTokenizer(lang, dark);
  if (!tk) return null;
  const session = new TokenSession(text, tk);
  return {
    lineCount: session.lineCount,
    window: (from, count) => Promise.resolve(session.window(from, count)),
    close: () => {},
  };
}

let nextId = 1;
