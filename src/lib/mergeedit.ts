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
  /** Where each region actually starts in the editor, in px from the content top,
   *  the total content height, and how many lines each region owns. The side panes
   *  size themselves from this, so no assumption about line heights or widget
   *  sizes can drift, and the spacer maths uses real line counts. */
  onGeometry?: (tops: number[], total: number, rows: number[]) => void;
}

const MARK: Record<string, string> = { add: "+", del: "-", vs: "!", keep: "=" };
/** The conflict toolbar's height, mirrored by the strip the side panes reserve. */
const TOOLBAR_H = 24;

/** Region bounds, mapped through edits by CodeMirror.
 *
 *  startSide/endSide decide where text typed exactly AT a boundary lands. With
 *  the defaults, typing at the start of a region put the text before it: those
 *  lines then belonged to no region, so the region never saw them — the side
 *  panes were never padded for them and the conflict stayed unsettled. -1/+1
 *  makes both edges absorb insertions, so typing anywhere in a region — including
 *  into an empty one — is that region's text. */
class RegionValue extends RangeValue {
  startSide = -1;
  endSide = 1;
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
  const specs: { from: number; to: number; spec: RegionSpec }[] = [];
  const iter = state.field(regionField).iter();
  while (iter.value) {
    specs.push({ from: iter.from, to: Math.max(iter.from, iter.to), spec: iter.value.spec });
    iter.next();
  }
  const ranges: { from: number; value: ReturnType<typeof Decoration.line> }[] = [];

  // Band every line, assigning it to the last region that starts at or before it.
  // Editing across a boundary can leave a line outside every mapped range, and a
  // line with no band reads as though its colour was lost.
  let at = 0;
  for (let n = 1; n <= state.doc.lines; n++) {
    const line = state.doc.line(n);
    while (at + 1 < specs.length && specs[at + 1].from <= line.from) at++;
    const owner = specs[at];
    if (!owner || owner.from > line.from) continue;
    const kind = owner.spec.kind;
    if (!kind) continue;
    ranges.push({
      from: line.from,
      value: Decoration.line({
        class: `cm-band cm-band-${kind}`,
        attributes: { "data-mk": MARK[kind] ?? "" },
      }),
    });
  }

  for (const { from, to, spec } of specs) {
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
      // Where the blank rows go. For a region with text: after its last line. For
      // an emptied one, `to` sits inside the line it now SHARES with the next
      // region, and lineAt(to).to would put the rows after that line — inside the
      // next region, which is why the conflict's own box never grew. Such a region
      // takes its rows at its start instead, right below its toolbar.
      const empty = to <= from;
      ranges.push({
        from: empty ? state.doc.lineAt(from).from : state.doc.lineAt(to).to,
        value: Decoration.widget({
          widget: new SpacerWidget(spacer),
          block: true,
          side: empty ? -1 : 1,
        }),
      });
    }
  }

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
    // Left padding leaves room for the mark, so the code is not flush against it.
    ".cm-line": { position: "relative", padding: "0 6px 0 calc(1em + 7px)" },
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
      left: "1px",
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
    // Exactly 24px, matching the strip the side panes reserve for it: a taller
    // toolbar would make the editor outgrow the rows it spans, and the grid would
    // spread the difference over every row.
    ".cm-chead": {
      display: "flex",
      alignItems: "center",
      gap: "6px",
      height: `${TOOLBAR_H}px`,
      padding: "0 6px",
      boxSizing: "border-box",
      overflow: "hidden",
      background: "rgba(224,85,90,0.16)",
    },
    ".cm-chead button": {
      background: "var(--bg-alt, #1f1f1f)",
      color: "inherit",
      border: "1px solid var(--border, #333)",
      borderRadius: "4px",
      padding: "0 7px",
      height: "18px",
      lineHeight: "16px",
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

  /** Read the real position of every region from the rendered editor. A conflict
   *  starts at its toolbar, whose position is measured rather than derived: the
   *  widget's offset from the line is CodeMirror's business, not ours. */
  const measure = () => {
    if (!current.onGeometry) return;
    const contentTop = view.contentDOM.getBoundingClientRect().top;
    // Toolbars in DOM order match conflict regions in document order.
    const bars = Array.from(view.contentDOM.querySelectorAll(".cm-chead"));
    let bar = 0;
    const tops: number[] = [];
    const rows: number[] = [];
    const iter = view.state.field(regionField).iter();
    while (iter.value) {
      const spec = iter.value.spec;
      const to = Math.max(iter.from, iter.to);
      const el = spec.conflict ? bars[bar++] : undefined;
      tops[spec.region] = el
        ? el.getBoundingClientRect().top - contentTop
        : view.lineBlockAt(iter.from).top;
      // Rows the region owns. An empty range owns a row only when it sits on a
      // blank line of its own (the placeholder of an untouched conflict); once its
      // text is deleted it shares a line with a neighbour and owns nothing, and
      // the side panes need a spacer to show their proposals.
      const line = view.state.doc.lineAt(iter.from);
      rows[spec.region] =
        iter.from === to
          ? line.from === iter.from && line.length === 0
            ? 1
            : 0
          : view.state.doc.lineAt(to).number - line.number + 1;
      iter.next();
    }
    current.onGeometry(tops, view.contentHeight, rows);
  };

  const listener = EditorView.updateListener.of((u) => {
    if (u.docChanged || u.geometryChanged) queueMicrotask(measure);
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
  requestAnimationFrame(measure);

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
      requestAnimationFrame(measure);
    },
    touch() {
      view.dispatch({ effects: refresh.of(null) });
    },
    destroy() {
      view.destroy();
    },
  };
}
