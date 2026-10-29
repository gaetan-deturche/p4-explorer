//! Graphs to draw: each node where the user placed it, its pins, and the wires
//! between them — a Blueprint graph or a material graph, one side at a time —
//! then the two sides paired and every node and wire marked as added, removed,
//! changed or moved.

use std::collections::{HashMap, HashSet};

use serde::Serialize;

use super::archive::Val;
use super::doc::{leaf, raw_properties};
use super::graph::GraphNode;
use super::graphs::{expression_label, is_exec, k2_label};
use super::package::Package;

/// Name of the one graph a material or material function has.
pub const MATERIAL_GRAPH: &str = "Material graph";

#[derive(Serialize, Clone, Debug)]
pub struct Pin {
    pub name: String,
    /// "exec", or the pin category ("bool", "real", "object", …) that colours it.
    pub kind: String,
    /// An unlinked input's own value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Node {
    /// The node's object name: the same node on both sides.
    pub id: String,
    pub label: String,
    pub x: f64,
    pub y: f64,
    /// "node", "comment" (a box with a size) or "knot" (a reroute dot).
    pub shape: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub w: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub h: Option<f64>,
    /// A comment box's colour, linear RGBA.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<[f64; 4]>,
    pub inputs: Vec<Pin>,
    pub outputs: Vec<Pin>,
    pub status: &'static str,
    pub moved: bool,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Wire {
    pub from: String,
    pub from_pin: String,
    pub to: String,
    pub to_pin: String,
    pub kind: String,
    pub status: &'static str,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct Side {
    pub nodes: Vec<Node>,
    pub wires: Vec<Wire>,
}

#[derive(Serialize, Debug)]
pub struct GraphPair {
    pub name: String,
    pub left: Option<Side>,
    pub right: Option<Side>,
    /// Nodes and wires that differ.
    pub changes: usize,
}

fn num(v: &Val, key: &str) -> f64 {
    match v.get(key) {
        Some(Val::Int(i)) => *i as f64,
        Some(Val::Float(f)) => *f,
        _ => 0.0,
    }
}

fn text(v: &Val, key: &str) -> Option<String> {
    v.get(key).and_then(|s| s.as_str()).filter(|s| !s.is_empty() && *s != "None").map(String::from)
}

fn color(v: &Val, key: &str) -> Option<[f64; 4]> {
    let c = v.get(key)?;
    Some([num(c, "R"), num(c, "G"), num(c, "B"), num(c, "A")])
}

fn node(id: String, label: String, x: f64, y: f64) -> Node {
    Node {
        id,
        label,
        x,
        y,
        shape: "node",
        w: None,
        h: None,
        color: None,
        inputs: Vec::new(),
        outputs: Vec::new(),
        status: "same",
        moved: false,
    }
}

/// Every graph of the package that can be drawn, by name.
pub fn sides(pkg: &Package, nodes: &[GraphNode]) -> Vec<(String, Side)> {
    let main = pkg.exports.iter().position(|e| e.is_asset);
    let class = main.map(|m| leaf(&pkg.export_class(&pkg.exports[m])).to_string()).unwrap_or_default();
    if class == "Material" || class.starts_with("MaterialFunction") {
        return vec![(MATERIAL_GRAPH.to_string(), material_side(pkg, main.unwrap()))];
    }
    blueprint_sides(pkg, nodes)
}

fn blueprint_sides(pkg: &Package, nodes: &[GraphNode]) -> Vec<(String, Side)> {
    let mut out: Vec<(String, Side)> = Vec::new();
    for n in nodes {
        let outer = pkg.exports[n.export].outer_index;
        let graph = if outer > 0 { pkg.exports[(outer - 1) as usize].object_name.clone() } else { "?".into() };
        let props = raw_properties(pkg, n.export);
        let id = leaf(&n.path).to_string();
        let kind = leaf(&n.class);
        let mut g = node(id.clone(), k2_label(&n.class, &props, &graph), num(&props, "NodePosX"), num(&props, "NodePosY"));
        if kind == "EdGraphNode_Comment" {
            g.shape = "comment";
            g.label = text(&props, "NodeComment").unwrap_or_default();
            g.w = Some(match num(&props, "NodeWidth") { w if w > 0.0 => w, _ => 400.0 });
            g.h = Some(match num(&props, "NodeHeight") { h if h > 0.0 => h, _ => 100.0 });
            g.color = color(&props, "CommentColor").or(Some([1.0, 1.0, 1.0, 1.0]));
        } else if kind == "K2Node_Knot" {
            g.shape = "knot";
        }
        let mut wires = Vec::new();
        for pin in n.pins.iter().flatten() {
            if pin.flag("hidden") {
                continue;
            }
            let k = if is_exec(pin) {
                "exec".to_string()
            } else {
                pin.ty.get("PinCategory").and_then(|c| c.as_str()).unwrap_or("").to_string()
            };
            let output = pin.direction == Val::str("Output");
            let value = (!output && pin.linked_to.is_empty())
                .then(|| {
                    if !pin.default_value.is_empty() {
                        Some(pin.default_value.clone())
                    } else if pin.default_object != "None" {
                        Some(leaf(&pin.default_object).to_string())
                    } else {
                        None
                    }
                })
                .flatten();
            let name = match pin.rest("friendly_name").and_then(|f| f.as_str()) {
                Some(f) if !f.is_empty() => f.to_string(),
                _ => pin.name.clone(),
            };
            let p = Pin { name: name.clone(), kind: k.clone(), value };
            if output {
                g.outputs.push(p);
                for l in &pin.linked_to {
                    wires.push((pin.name.clone(), name.clone(), leaf(&l.node).to_string(), l.pin.clone().unwrap_or_default(), k.clone()));
                }
            } else {
                g.inputs.push(p);
            }
        }
        let side = match out.iter().position(|(name, _)| *name == graph) {
            Some(i) => &mut out[i].1,
            None => {
                out.push((graph.clone(), Side::default()));
                &mut out.last_mut().unwrap().1
            }
        };
        side.nodes.push(g);
        // Wire ends name pins by display name; links name the target by its
        // internal name, mapped once every node of the graph is known.
        for (_, from_pin, to, to_pin, kind) in wires {
            side.wires.push(Wire { from: id.clone(), from_pin, to, to_pin, kind, status: "same" });
        }
    }
    // Links point at internal pin names; show the display name the node drew.
    let names: HashMap<(String, String), String> = nodes
        .iter()
        .flat_map(|n| {
            n.pins.iter().flatten().map(move |p| {
                let shown = match p.rest("friendly_name").and_then(|f| f.as_str()) {
                    Some(f) if !f.is_empty() => f.to_string(),
                    _ => p.name.clone(),
                };
                ((leaf(&n.path).to_string(), p.name.clone()), shown)
            })
        })
        .collect();
    for (_, side) in &mut out {
        for w in &mut side.wires {
            if let Some(s) = names.get(&(w.to.clone(), w.to_pin.clone())) {
                w.to_pin = s.clone();
            }
        }
    }
    out
}

/// An FExpressionInput's source: (expression path, output index).
fn source(input: &Val) -> Option<(String, i64)> {
    let expr = text(input, "Expression")?;
    let index = match input.get("OutputIndex") {
        Some(Val::Int(i)) => (*i).max(0),
        _ => 0,
    };
    Some((expr, index))
}

/// Inputs of one expression: (pin name, source when linked). Lists of named
/// inputs (function calls, Custom nodes) list every input, linked or not.
fn expression_inputs(props: &Val) -> Vec<(String, Option<(String, i64)>)> {
    let mut out = Vec::new();
    let Val::Map(m) = props else { return out };
    for (k, v) in m {
        if v.get("Expression").is_some() {
            if let Some(s) = source(v) {
                out.push((k.clone(), Some(s)));
            }
            continue;
        }
        if let Val::List(items) = v {
            for it in items {
                let Some(input) = it.get("Input") else { continue };
                if input.get("Expression").is_none() {
                    continue;
                }
                let name = text(input, "InputName").or(text(it, "InputName")).unwrap_or_else(|| "?".into());
                out.push((name, source(input)));
            }
        }
    }
    out
}

fn material_side(pkg: &Package, main: usize) -> Side {
    let mut side = Side::default();
    let mut out_names: HashMap<String, Vec<String>> = HashMap::new(); // node id -> named outputs
    let mut inputs: Vec<(String, Vec<(String, Option<(String, i64)>)>)> = Vec::new();
    for (i, e) in pkg.exports.iter().enumerate() {
        let class = pkg.export_class(e);
        let kind = leaf(&class);
        if !kind.starts_with("MaterialExpression") {
            continue;
        }
        let props = raw_properties(pkg, i);
        let id = e.object_name.clone();
        let mut g = node(
            id.clone(),
            expression_label(&class, &props),
            num(&props, "MaterialExpressionEditorX"),
            num(&props, "MaterialExpressionEditorY"),
        );
        match kind {
            "MaterialExpressionComment" => {
                g.shape = "comment";
                g.label = text(&props, "Text").unwrap_or_default();
                g.w = Some(match num(&props, "SizeX") { w if w > 0.0 => w, _ => 400.0 });
                g.h = Some(match num(&props, "SizeY") { h if h > 0.0 => h, _ => 100.0 });
                g.color = color(&props, "CommentColor").or(Some([1.0, 1.0, 1.0, 1.0]));
            }
            "MaterialExpressionReroute" => g.shape = "knot",
            _ => {}
        }
        if let Some(Val::List(outs)) = props.get("FunctionOutputs") {
            let names = outs.iter().map(|o| o.get("Output").and_then(|x| text(x, "OutputName")).unwrap_or_default()).collect();
            out_names.insert(id.clone(), names);
        }
        let ins = expression_inputs(&props);
        g.inputs = ins.iter().map(|(n, _)| Pin { name: n.clone(), kind: "data".into(), value: None }).collect();
        inputs.push((id, ins));
        side.nodes.push(g);
    }
    // The material's own result node: its pins live on the editor-only data.
    let main_props = raw_properties(pkg, main);
    if let Some(editor) = text(&main_props, "EditorOnlyData") {
        if let Some(i) = (0..pkg.exports.len()).find(|&i| pkg.export_path(i) == editor) {
            let ins = expression_inputs(&raw_properties(pkg, i));
            if !ins.is_empty() {
                let mut g = node("Result".into(), pkg.exports[main].object_name.clone(), num(&main_props, "EditorX"), num(&main_props, "EditorY"));
                g.inputs = ins.iter().map(|(n, _)| Pin { name: n.clone(), kind: "data".into(), value: None }).collect();
                inputs.push(("Result".into(), ins));
                side.nodes.push(g);
            }
        }
    }
    // Wires, and the outputs each source needs.
    let mut used: HashMap<String, HashSet<i64>> = HashMap::new();
    for (to, ins) in &inputs {
        for (pin, src) in ins {
            let Some((path, index)) = src else { continue };
            let from = leaf(path).to_string();
            used.entry(from.clone()).or_default().insert(*index);
            let names = out_names.get(&from);
            let from_pin = names.and_then(|n| n.get(*index as usize).cloned()).unwrap_or_else(|| out_name(*index));
            side.wires.push(Wire { from, from_pin, to: to.clone(), to_pin: pin.clone(), kind: "data".into(), status: "same" });
        }
    }
    for n in &mut side.nodes {
        let names = out_names.get(&n.id);
        let count = match names {
            Some(v) => v.len(),
            None => used.get(&n.id).and_then(|u| u.iter().max()).map(|m| *m as usize + 1).unwrap_or(1),
        };
        if n.shape == "comment" || n.id == "Result" {
            continue;
        }
        n.outputs = (0..count)
            .map(|i| Pin {
                name: names.and_then(|v| v.get(i).cloned()).unwrap_or_else(|| out_name(i as i64)),
                kind: "data".into(),
                value: None,
            })
            .collect();
    }
    side
}

fn out_name(index: i64) -> String {
    if index == 0 { String::new() } else { format!("out{index}") }
}

/// Pair the two sides' graphs by name and mark what differs. `node_vals`
/// gives each side's text form of a node, which decides "changed".
pub fn pair(
    left: Vec<(String, Side)>,
    right: Vec<(String, Side)>,
    left_vals: &dyn Fn(&str, &str) -> Option<Val>,
    right_vals: &dyn Fn(&str, &str) -> Option<Val>,
) -> Vec<GraphPair> {
    let mut names: Vec<String> = left.iter().map(|(n, _)| n.clone()).collect();
    for (n, _) in &right {
        if !names.contains(n) {
            names.push(n.clone());
        }
    }
    let mut left: HashMap<String, Side> = left.into_iter().collect();
    let mut right: HashMap<String, Side> = right.into_iter().collect();
    names
        .into_iter()
        .map(|name| {
            let mut l = left.remove(&name);
            let mut r = right.remove(&name);
            let mut changes = 0;
            let lpos: HashMap<String, (f64, f64)> =
                l.iter().flat_map(|s| s.nodes.iter().map(|n| (n.id.clone(), (n.x, n.y)))).collect();
            let rpos: HashMap<String, (f64, f64)> =
                r.iter().flat_map(|s| s.nodes.iter().map(|n| (n.id.clone(), (n.x, n.y)))).collect();
            for n in l.iter_mut().flat_map(|s| s.nodes.iter_mut()) {
                match rpos.get(&n.id) {
                    None => {
                        n.status = "removed";
                        changes += 1;
                    }
                    Some(p) => {
                        n.moved = *p != (n.x, n.y);
                        if left_vals(&name, &n.id) != right_vals(&name, &n.id) {
                            n.status = "changed";
                        }
                    }
                }
            }
            for n in r.iter_mut().flat_map(|s| s.nodes.iter_mut()) {
                match lpos.get(&n.id) {
                    None => {
                        n.status = "added";
                        changes += 1;
                    }
                    Some(p) => {
                        n.moved = *p != (n.x, n.y);
                        if left_vals(&name, &n.id) != right_vals(&name, &n.id) {
                            n.status = "changed";
                            changes += 1;
                        }
                    }
                }
            }
            let key = |w: &Wire| (w.from.clone(), w.from_pin.clone(), w.to.clone(), w.to_pin.clone());
            let lw: HashSet<_> = l.iter().flat_map(|s| s.wires.iter().map(key)).collect();
            let rw: HashSet<_> = r.iter().flat_map(|s| s.wires.iter().map(key)).collect();
            for w in l.iter_mut().flat_map(|s| s.wires.iter_mut()) {
                if !rw.contains(&key(w)) {
                    w.status = "removed";
                    changes += 1;
                }
            }
            for w in r.iter_mut().flat_map(|s| s.wires.iter_mut()) {
                if !lw.contains(&key(w)) {
                    w.status = "added";
                    changes += 1;
                }
            }
            GraphPair { name, left: l, right: r, changes }
        })
        .collect()
}
