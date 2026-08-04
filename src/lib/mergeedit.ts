//! The result pane of the resolve window: one CodeMirror document over the whole
//! merged file.
//!
//! Why CodeMirror rather than our own editable rows: the caret, undo, dead keys,
//! paste and the line-number gutter are its problem, not ours, and region
//! positions survive arbitrary edits because they live in a RangeSet that
//! CodeMirror maps through every change. Everything the resolve window needs on
//! top of that — the add/drop/conflict bands and the per-conflict toolbar — is a
//! decoration, so none of it can end up in the saved text.

import { EditorState, RangeSet, RangeValue, StateEffect, StateField } from "@codemirror/state";
import {
  Decoration,
  EditorView,
  WidgetType,
  keymap,
  lineNumbers,
  type DecorationSet,
} from "@codemirror/view";
import { defaultKeymap, history, historyKeymap } from "@codemirror/commands";
import { HighlightStyle, syntaxHighlighting } from "@codemirror/language";
import { cpp } from "@codemirror/lang-cpp";
import { tags as t } from "@lezer/highlight";

/** One merge region, as the editor tracks it. */
export interface RegionSpec {
  /** Index into the window's region list. */
  region: number;
  /** "add" | "del" | "vs" | "keep" | "" — drives the band colour and mark. */
  kind: string;
  conflict: boolean;
  /** Text this region contributes; "" for an unsettled conflict. */
  text: string;
}

export interface MergeEditorConfig {
  regions: RegionSpec[];
  /** Called with (region index, new text) whenever the user edits a region. */
  onEdit: (region: number, text: string) => void;
  /** A toolbar button was pressed on a conflict. */
  onTake: (region: number, what: "theirs" | "ours" | "both" | "base") => void;
  onReset: (region: number) => void;
  /** Conflict number to show in the toolbar, 1-based. */
  conflictNumber: (region: number) => number;
  settled: (region: number) => boolean;
}

const MARK: Record<string, string> = { add: "+", del: "-", vs: "!", keep: "=" };

/** Region bounds, mapped through edits by CodeMirror. */
class RegionValue extends RangeValue {
  constructor(readonly spec: RegionSpec) {
    super();
  }
}

const setRegions = StateEffect.define<RangeSet<RegionValue>>();
const setSpacers = StateEffect.define<Map<number, number>>();
/** Bumped by the host when button labels / settled state change. */
const refresh = StateEffect.define<null>();

const regionField = StateField.define<RangeSet<RegionValue>>({
  create: () => RangeSet.empty,
  update(value, tr) {
    for (const e of tr.effects) if (e.is(setRegions)) return e.value;
    return value.map(tr.changes);
  },
});

/** region → blank rows to leave after it, so the side panes stay level. */
const spacerField = StateField.define<Map<number, number>>({
  create: () => new Map(),
  update(value, tr) {
    for (const e of tr.effects) if (e.is(setSpacers)) return e.value;
    return value;
  },
});

/** Bumped so decorations recompute when only host state changed. */
const versionField = StateField.define<number>({
  create: () => 0,
  update(value, tr) {
    for (const e of tr.effects) if (e.is(refresh)) return value + 1;
    return value;
  },
});

/** The toolbar shown above an unresolved conflict. Plain DOM: it is a decoration,
 *  never part of the document. */
class ToolbarWidget extends WidgetType {
  constructor(
    readonly spec: RegionSpec,
    readonly cfg: MergeEditorConfig,
  ) {
    super();
  }
  eq(other: ToolbarWidget) {
    return (
      other.spec.region === this.spec.region &&
      other.cfg.settled(other.spec.region) === this.cfg.settled(this.spec.region)
    );
  }
  toDOM() {
    const bar = document.createElement("div");
    bar.className = "cm-chead";
    const n = document.createElement("span");
    n.className = "cm-cnum";
    n.textContent = `conflict ${this.cfg.conflictNumber(this.spec.region)}`;
    bar.appendChild(n);
    const add = (label: string, run: () => void, title = "") => {
      const b = document.createElement("button");
      b.textContent = label;
      if (title) b.title = title;
      b.onmousedown = (e) => e.preventDefault(); // keep the caret where it is
      b.onclick = run;
      bar.appendChild(b);
      return b;
    };
    add("◀ depot", () => this.cfg.onTake(this.spec.region, "theirs"), "Copy the depot side in");
    add("workspace ▶", () => this.cfg.onTake(this.spec.region, "ours"), "Copy the workspace side in");
    add("both", () => this.cfg.onTake(this.spec.region, "both"), "Depot first, then workspace");
    add("base", () => this.cfg.onTake(this.spec.region, "base"), "Keep the common ancestor");
    if (this.cfg.settled(this.spec.region)) {
      add("reset", () => this.cfg.onReset(this.spec.region), "Back to an undecided conflict");
    }
    return bar;
  }
  ignoreEvent() {
    return false;
  }
}

/** Blank space after a region, so the side panes can stay row-aligned. */
class SpacerWidget extends WidgetType {
  constructor(readonly rows: number) {
    super();
  }
  eq(other: SpacerWidget) {
    return other.rows === this.rows;
  }
  toDOM() {
    const el = document.createElement("div");
    el.className = "cm-spacer";
    el.style.height = `calc(${this.rows} * var(--lh))`;
    return el;
  }
}

function buildDecorations(state: EditorState, cfg: MergeEditorConfig): DecorationSet {
  const deco: ReturnType<typeof Decoration.line>[] = [];
  const ranges: { from: number; value: ReturnType<typeof Decoration.line> }[] = [];
  const set = state.field(regionField);
  const iter = set.iter();
  while (iter.value) {
    const spec = iter.value.spec;
    const from = iter.from;
    const to = Math.max(iter.from, iter.to);
    // Band + mark on every line of the region.
    if (spec.kind) {
      const first = state.doc.lineAt(from).number;
      const last = state.doc.lineAt(to).number;
      for (let n = first; n <= last; n++) {
        const line = state.doc.line(n);
        if (to > from && line.from >= to && n > first) break;
        ranges.push({
          from: line.from,
          value: Decoration.line({
            class: `cm-band cm-band-${spec.kind}`,
            attributes: { "data-mk": MARK[spec.kind] ?? "" },
          }),
        });
      }
    }
    if (spec.conflict) {
      ranges.push({
        from: state.doc.lineAt(from).from,
        value: Decoration.widget({
          widget: new ToolbarWidget(spec, cfg),
          block: true,
          side: -1,
        }),
      });
    }
    const spacer = state.field(spacerField).get(spec.region) ?? 0;
    if (spacer > 0) {
      ranges.push({
        from: state.doc.lineAt(to).to,
        value: Decoration.widget({
          widget: new SpacerWidget(spacer),
          block: true,
          side: 1,
        }),
      });
    }
    iter.next();
  }
  void deco;
  ranges.sort((a, b) => a.from - b.from);
  return Decoration.set(
    ranges.map((r) => r.value.range(r.from)),
    true,
  );
}

/** VS Code dark-plus, the same palette Shiki renders the side panes with, so the
 *  three panes look like one document. */
const darkPlus = HighlightStyle.define([
  { tag: t.keyword, color: "#569cd6" },
  { tag: t.controlKeyword, color: "#c586c0" },
  { tag: t.moduleKeyword, color: "#c586c0" },
  { tag: [t.name, t.deleted, t.character, t.macroName], color: "#9cdcfe" },
  { tag: [t.propertyName], color: "#9cdcfe" },
  { tag: [t.function(t.variableName), t.labelName], color: "#dcdcaa" },
  { tag: [t.typeName, t.className, t.namespace], color: "#4ec9b0" },
  { tag: [t.number, t.bool, t.null], color: "#b5cea8" },
  { tag: [t.string, t.special(t.string), t.regexp], color: "#ce9178" },
  { tag: [t.comment, t.lineComment, t.blockComment], color: "#6a9955", fontStyle: "italic" },
  { tag: [t.meta, t.processingInstruction], color: "#9b9b9b" },
  { tag: t.operator, color: "#d4d4d4" },
  { tag: t.punctuation, color: "#d4d4d4" },
  { tag: t.invalid, color: "#f44747" },
]);

const theme = EditorView.theme(
  {
    "&": { height: "auto", fontSize: "12px", backgroundColor: "transparent", color: "inherit" },
    ".cm-scroller": {
      fontFamily: "var(--mono, ui-monospace, Consolas, monospace)",
      lineHeight: "17.4px", // must equal --lh: the spacer maths assumes it
      overflowX: "auto",
    },
    ".cm-content": { padding: "0" },
    ".cm-line": { position: "relative", padding: "0 6px 0 8px" },
    "&.cm-focused": { outline: "none" },
    ".cm-gutters": {
      backgroundColor: "transparent",
      border: "none",
      color: "var(--text-dim, #999)",
      opacity: "0.55",
    },
    ".cm-lineNumbers .cm-gutterElement": { padding: "0 8px 0 4px", minWidth: "3.2em" },
    ".cm-activeLine": { backgroundColor: "rgba(255,255,255,0.03)" },
    ".cm-activeLineGutter": { backgroundColor: "transparent" },
    ".cm-cursor, .cm-dropCursor": { borderLeftColor: "var(--accent, #d98d3a)" },
    "&.cm-focused .cm-selectionBackground, ::selection": {
      backgroundColor: "rgba(217,141,58,0.25)",
    },
    // The add / drop / conflict bands, and their mark in the line's left padding.
    ".cm-band::before": {
      content: "attr(data-mk)",
      position: "absolute",
      left: "0",
      width: "1em",
      textAlign: "center",
      userSelect: "none",
    },
    ".cm-band-add": { backgroundColor: "rgba(108,195,108,0.15)", borderLeft: "3px solid #5faf5f" },
    ".cm-band-add::before": { color: "#7cc47c" },
    ".cm-band-del": { backgroundColor: "rgba(217,135,58,0.14)", borderLeft: "3px solid #d9873a" },
    ".cm-band-del::before": { color: "#d9873a" },
    ".cm-band-vs": { backgroundColor: "rgba(224,85,90,0.2)", borderLeft: "3px solid #e0555a" },
    ".cm-band-vs::before": { color: "#e0555a" },
    ".cm-band-keep": { backgroundColor: "rgba(180,180,180,0.08)", borderLeft: "3px solid #6d6d6d" },
    ".cm-band-keep::before": { color: "var(--text-dim, #999)" },
    ".cm-chead": {
      display: "flex",
      alignItems: "center",
      gap: "6px",
      minHeight: "24px",
      padding: "2px 6px",
      boxSizing: "border-box",
      background: "rgba(224,85,90,0.16)",
    },
    ".cm-chead button": {
      background: "var(--bg-alt, #1f1f1f)",
      color: "inherit",
      border: "1px solid var(--border, #333)",
      borderRadius: "4px",
      padding: "2px 8px",
      fontSize: "11px",
      cursor: "pointer",
      whiteSpace: "nowrap",
    },
    ".cm-cnum": {
      fontSize: "10px",
      fontWeight: "600",
      color: "var(--text-dim, #999)",
      whiteSpace: "nowrap",
    },
    ".cm-spacer": { pointerEvents: "none" },
  },
  { dark: true },
);

export interface MergeEditor {
  view: EditorView;
  /** Rebuild the document from specs (used when a side is taken, or on reset). */
  setRegions(regions: RegionSpec[]): void;
  /** Blank rows to leave after each region, keyed by region index. */
  setSpacers(rows: Map<number, number>): void;
  /** Re-render decorations after host state (settled, numbering) changed. */
  touch(): void;
  destroy(): void;
}

/** Assemble the document and the region ranges that track it. */
function assemble(regions: RegionSpec[]): { doc: string; set: RangeSet<RegionValue> } {
  const parts: string[] = [];
  const ranges: { value: RegionValue; from: number; to: number }[] = [];
  let at = 0;
  regions.forEach((spec, k) => {
    const text = spec.text;
    const from = at;
    parts.push(text);
    at += text.length;
    if (k < regions.length - 1) {
      parts.push("\n");
      at += 1;
    }
    ranges.push({ value: new RegionValue(spec), from, to: from + text.length });
  });
  return {
    doc: parts.join(""),
    set: RangeSet.of(
      ranges.map((r) => r.value.range(r.from, r.to)),
      true,
    ),
  };
}

export function createMergeEditor(parent: HTMLElement, cfg: MergeEditorConfig): MergeEditor {
  let current = cfg;
  let applying = false; // suppress onEdit while we rewrite the doc ourselves

  const decorations = EditorView.decorations.compute(
    [regionField, spacerField, versionField],
    (state) => buildDecorations(state, current),
  );

  const listener = EditorView.updateListener.of((u) => {
    if (!u.docChanged || applying) return;
    // Report each region's text as it now stands, so the host state follows.
    const set = u.state.field(regionField);
    const iter = set.iter();
    while (iter.value) {
      const spec = iter.value.spec;
      const text = u.state.doc.sliceString(iter.from, Math.max(iter.from, iter.to));
      if (text !== spec.text) current.onEdit(spec.region, text);
      iter.next();
    }
  });

  const built = assemble(cfg.regions);
  const view = new EditorView({
    parent,
    state: EditorState.create({
      doc: built.doc,
      extensions: [
        lineNumbers(),
        history(),
        keymap.of([...defaultKeymap, ...historyKeymap]),
        cpp(),
        syntaxHighlighting(darkPlus),
        theme,
        regionField,
        spacerField,
        versionField,
        decorations,
        listener,
        EditorView.editable.of(true),
      ],
    }),
  });
  view.dispatch({ effects: setRegions.of(built.set) });

  return {
    view,
    setRegions(regions: RegionSpec[]) {
      current = { ...current, regions };
      const next = assemble(regions);
      applying = true;
      view.dispatch({
        changes: { from: 0, to: view.state.doc.length, insert: next.doc },
        effects: setRegions.of(next.set),
      });
      applying = false;
    },
    setSpacers(rows: Map<number, number>) {
      view.dispatch({ effects: setSpacers.of(rows) });
    },
    touch() {
      view.dispatch({ effects: refresh.of(null) });
    },
    destroy() {
      view.destroy();
    },
  };
}
