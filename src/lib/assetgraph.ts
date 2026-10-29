//! Drawn asset graphs: the shapes Rust sends (one revision of one graph) and
//! where each node and pin lands on the canvas, in graph units (the editor's).

export interface GPin {
  name: string;
  /** "exec", or a pin category ("bool", "real", "object", …; "data" for materials). */
  kind: string;
  value?: string;
}

export type GStatus = "same" | "changed" | "added" | "removed";

export interface GNode {
  id: string;
  label: string;
  x: number;
  y: number;
  shape: "node" | "comment" | "knot";
  w?: number;
  h?: number;
  color?: [number, number, number, number];
  inputs: GPin[];
  outputs: GPin[];
  status: GStatus;
  moved: boolean;
}

export interface GWire {
  from: string;
  fromPin: string;
  to: string;
  toPin: string;
  kind: string;
  status: GStatus;
}

export interface GSide {
  nodes: GNode[];
  wires: GWire[];
}

export interface GraphPair {
  name: string;
  left: GSide | null;
  right: GSide | null;
  changes: number;
}

export const HEADER = 26;
export const ROW = 20;
const CHAR = 6.6; // 11px mono-ish text
const KNOT = 12;

export interface Geom {
  x: number;
  y: number;
  w: number;
  h: number;
  ins: Map<string, [number, number]>;
  outs: Map<string, [number, number]>;
}

/** A node's box and its pins' anchor points. */
export function geom(n: GNode): Geom {
  if (n.shape === "comment") {
    return { x: n.x, y: n.y, w: n.w ?? 400, h: n.h ?? 100, ins: new Map(), outs: new Map() };
  }
  if (n.shape === "knot") {
    const c: [number, number] = [n.x + KNOT / 2, n.y + KNOT / 2];
    const at = (pins: GPin[]) => new Map(pins.map((p) => [p.name, c] as [string, [number, number]]));
    return { x: n.x, y: n.y, w: KNOT, h: KNOT, ins: at(n.inputs), outs: at(n.outputs) };
  }
  const label = (p: GPin) => (p.kind === "exec" && /^(execute|then)$/i.test(p.name) ? "" : p.name);
  const inW = Math.max(0, ...n.inputs.map((p) => (label(p).length + (p.value ? p.value.length + 2 : 0)) * CHAR));
  const outW = Math.max(0, ...n.outputs.map((p) => label(p).length * CHAR));
  const w = Math.max(110, n.label.length * 7.2 + 24, inW + outW + 48);
  const rows = Math.max(n.inputs.length, n.outputs.length, 1);
  const h = HEADER + rows * ROW + 6;
  const pinY = (i: number) => n.y + HEADER + i * ROW + ROW / 2;
  return {
    x: n.x,
    y: n.y,
    w,
    h,
    ins: new Map(n.inputs.map((p, i) => [p.name, [n.x, pinY(i)]] as [string, [number, number]])),
    outs: new Map(n.outputs.map((p, i) => [p.name, [n.x + w, pinY(i)]] as [string, [number, number]])),
  };
}

/** The bounding box of everything drawn on either side. */
export function bounds(sides: (GSide | null)[]): { x: number; y: number; w: number; h: number } | null {
  let x0 = Infinity,
    y0 = Infinity,
    x1 = -Infinity,
    y1 = -Infinity;
  for (const s of sides) {
    for (const n of s?.nodes ?? []) {
      const g = geom(n);
      x0 = Math.min(x0, g.x);
      y0 = Math.min(y0, g.y);
      x1 = Math.max(x1, g.x + g.w);
      y1 = Math.max(y1, g.y + g.h);
    }
  }
  return x0 === Infinity ? null : { x: x0, y: y0, w: x1 - x0, h: y1 - y0 };
}

/** Wire colours by pin kind, after the editor's palette. */
export function kindColor(kind: string): string {
  switch (kind) {
    case "exec":
      return "#e6e6e6";
    case "bool":
      return "#b0262a";
    case "int":
    case "int64":
      return "#1fe3af";
    case "real":
    case "float":
    case "double":
      return "#9ff241";
    case "string":
      return "#f20cb1";
    case "name":
      return "#c88cf0";
    case "text":
      return "#e57fa0";
    case "object":
    case "softobject":
      return "#0ba3ec";
    case "class":
    case "softclass":
      return "#8a5ae6";
    case "struct":
      return "#2c7fe0";
    case "byte":
    case "enum":
      return "#139c7f";
    case "interface":
      return "#e8f070";
    case "delegate":
    case "mcdelegate":
      return "#ff6060";
    default:
      return "#b8b8b8";
  }
}

export const STATUS_COLOR: Record<GStatus, string> = {
  same: "#5a5d64",
  changed: "#5a96e6",
  added: "#5faf5f",
  removed: "#d9873a",
};
