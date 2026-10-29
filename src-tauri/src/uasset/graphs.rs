//! Graphs as lists: each node of a Blueprint or material graph with a readable
//! label, its settings, and what its pins connect to — so a diff reads
//! "Condition ← Get IsAlive › IsAlive" instead of pin GUIDs and node positions.
//!
//! Wiring is shown from where it is read: an exec pin lists where execution
//! goes next, a data input lists where its value comes from. Plain reroutes
//! (material Reroute, Blueprint Knot) are followed through and not listed.
//! Labels are not made unique: "Branch #3" would renumber whenever another
//! Branch comes or goes, and the row key already names the node.

use std::collections::HashMap;

use super::archive::Val;
use super::doc::{leaf, properties};
use super::graph::{GraphNode, Pin};
use super::package::Package;

use super::diff::LABEL;

/// Layout and bookkeeping on graph nodes: a moved or re-guided node is not a change.
const NODE_NOISE: &[&str] = &[
    "NodePosX",
    "NodePosY",
    "NodeWidth",
    "NodeHeight",
    "NodeGuid",
    "bCommentBubblePinned",
    "bCommentBubbleVisible",
    "bCommentBubbleMakeVisible",
    "bHasCompilerMessage",
    "ErrorType",
    "ErrorMsg",
    "MemberGuid",
    "Material",
    "Function",
    "bCollapsed",
    "bRealtimePreview",
    "bNeedToUpdatePreview",
    "bShowOutputNameOnPin",
    "bIsParameterExpression",
    "Outputs",
    "FunctionOutputs",
    "ExpressionInputId",
    "ParameterId",
];

fn strip(v: Val) -> Val {
    match v {
        Val::Map(m) => Val::Map(
            m.into_iter().filter(|(k, _)| !NODE_NOISE.contains(&k.as_str())).map(|(k, v)| (k, strip(v))).collect(),
        ),
        Val::List(l) => Val::List(l.into_iter().map(strip).collect()),
        other => other,
    }
}

fn str_at<'a>(v: &'a Val, path: &[&str]) -> Option<&'a str> {
    let mut cur = v;
    for key in path {
        cur = cur.get(key)?;
    }
    cur.as_str().filter(|s| !s.is_empty() && *s != "None")
}

fn one_or_many(mut items: Vec<String>) -> Option<Val> {
    match items.len() {
        0 => None,
        1 => Some(Val::Str(items.remove(0))),
        _ => Some(Val::List(items.into_iter().map(Val::Str).collect())),
    }
}

// ---------------------------------------------------------------------------
// Blueprints

pub fn k2_label(class: &str, props: &Val, graph_name: &str) -> String {
    let kind = leaf(class);
    let member = |k: &str| str_at(props, &[k, "MemberName"]).map(String::from);
    let leaf_of = |k: &str| str_at(props, &[k]).map(|s| leaf(s).trim_end_matches("_C").to_string());
    let short = kind.strip_prefix("K2Node_").unwrap_or(kind);
    match short {
        "CallParentFunction" => format!("Parent: {}", member("FunctionReference").unwrap_or_default()),
        "VariableGet" => format!("Get {}", member("VariableReference").unwrap_or_default()),
        "VariableSet" => format!("Set {}", member("VariableReference").unwrap_or_default()),
        "Event" => format!(
            "Event {}",
            str_at(props, &["CustomFunctionName"]).map(String::from).or(member("EventReference")).unwrap_or_default()
        ),
        "CustomEvent" => format!("Custom event {}", str_at(props, &["CustomFunctionName"]).unwrap_or("")),
        "FunctionEntry" => format!("Entry {graph_name}"),
        "FunctionResult" => format!("Return {graph_name}"),
        "IfThenElse" => "Branch".into(),
        "ExecutionSequence" => "Sequence".into(),
        "Self" => "Self".into(),
        "Select" => "Select".into(),
        "MacroInstance" => format!("Macro {}", str_at(props, &["MacroGraphReference", "MacroGraph"]).map(leaf).unwrap_or("")),
        "DynamicCast" | "ClassDynamicCast" => format!("Cast to {}", leaf_of("TargetType").unwrap_or_default()),
        "MakeStruct" | "BreakStruct" | "SetFieldsInStruct" => {
            format!("{} {}", &short[..short.len() - 6], leaf_of("StructType").unwrap_or_default())
        }
        "SwitchEnum" => format!("Switch on {}", leaf_of("Enum").unwrap_or_default()),
        _ if kind == "EdGraphNode_Comment" => format!("Comment: {}", str_at(props, &["NodeComment"]).unwrap_or("")),
        _ => match member("FunctionReference") {
            Some(f) => format!("Call {f}"),
            // Latent / async action nodes are named after the factory function they call.
            None => match str_at(props, &["ProxyFactoryFunctionName"]) {
                Some(f) => format!("Call {f}"),
                None => short.to_string(),
            },
        },
    }
}

pub fn is_exec(pin: &Pin) -> bool {
    pin.ty.get("PinCategory").and_then(|c| c.as_str()) == Some("exec")
}

/// What one pin shows, or None when it holds nothing worth listing.
fn k2_pin(pin: &Pin, target: &dyn Fn(&str, &str) -> String) -> Option<Val> {
    if pin.flag("hidden") {
        return None;
    }
    let output = pin.direction == Val::str("Output");
    let links: Vec<String> = pin
        .linked_to
        .iter()
        .map(|l| target(&l.node, l.pin.as_deref().unwrap_or("?")))
        .collect();
    if is_exec(pin) {
        return if output { one_or_many(links.into_iter().map(|l| format!("→ {l}")).collect()) } else { None };
    }
    if output {
        return None;
    }
    if !links.is_empty() {
        return one_or_many(links.into_iter().map(|l| format!("← {l}")).collect());
    }
    if !pin.default_value.is_empty() {
        return Some(Val::str(&pin.default_value));
    }
    if pin.default_object != "None" {
        return Some(Val::Str(pin.default_object.clone()));
    }
    match pin.rest("default_text_value") {
        Some(t) if *t != Val::str("") => Some(t.clone()),
        _ => None,
    }
}

/// Every Blueprint graph (event graph, functions, macros), node by node.
pub fn blueprint_graphs(pkg: &Package, nodes: &[GraphNode]) -> Val {
    let props: HashMap<usize, Val> = nodes.iter().map(|n| (n.export, properties(pkg, n.export))).collect();
    let graph_of = |n: &GraphNode| pkg.exports[n.export].outer_index;
    let graph_name = |outer: i32| if outer > 0 { pkg.exports[(outer - 1) as usize].object_name.clone() } else { "?".into() };

    // Labels, unique within each graph; knots are followed through, not shown.
    let mut labels: HashMap<String, String> = HashMap::new();
    let mut by_graph: HashMap<i32, HashMap<String, String>> = HashMap::new();
    let mut knots: HashMap<String, &GraphNode> = HashMap::new();
    for n in nodes {
        if leaf(&n.class) == "K2Node_Knot" {
            knots.insert(n.path.clone(), n);
            continue;
        }
        let l = k2_label(&n.class, &props[&n.export], &graph_name(graph_of(n)));
        by_graph.entry(graph_of(n)).or_default().insert(n.path.clone(), l);
    }
    for g in by_graph.into_values() {
        labels.extend(g);
    }
    // "Label › pin", through any knots on the way.
    fn resolve(node: &str, pin: &str, input_side: bool, labels: &HashMap<String, String>, knots: &HashMap<String, &GraphNode>, depth: u8) -> Vec<String> {
        if let Some(knot) = knots.get(node).filter(|_| depth < 32) {
            // Through a knot: continue on its other side.
            let want = if input_side { "Input" } else { "Output" };
            let mut out = Vec::new();
            for p in knot.pins.iter().flatten().filter(|p| p.direction == Val::str(want)) {
                for l in &p.linked_to {
                    out.extend(resolve(&l.node, l.pin.as_deref().unwrap_or("?"), input_side, labels, knots, depth + 1));
                }
            }
            return out;
        }
        vec![format!("{} › {pin}", labels.get(node).cloned().unwrap_or_else(|| leaf(node).to_string()))]
    }

    let mut graphs: Vec<(String, Vec<(String, Val)>)> = Vec::new();
    for n in nodes {
        let Some(label) = labels.get(&n.path) else { continue };
        let mut m: Vec<(String, Val)> = vec![(LABEL.into(), Val::str(label))];
        for pin in n.pins.iter().flatten() {
            // An input's source sits behind knots on the input side; an exec
            // output's target behind knots on the output side.
            let input_side = pin.direction != Val::str("Output");
            let target = |node: &str, p: &str| resolve(node, p, input_side, &labels, &knots, 0).join(", ");
            if let Some(v) = k2_pin(pin, &target) {
                m.push((pin.name.clone(), v));
            }
        }
        if let Val::Map(p) = strip(props[&n.export].clone()) {
            for (k, mut v) in p {
                // Make/Break struct nodes list every member; only the shown pins matter.
                if k == "ShowPinForProperties" {
                    let Val::List(items) = &v else { continue };
                    let shown = items
                        .iter()
                        .filter(|it| it.get("bShowPin") == Some(&Val::Bool(true)))
                        .filter_map(|it| it.get("PropertyName").cloned())
                        .collect();
                    v = Val::List(shown);
                }
                if !m.iter().any(|(e, _)| *e == k) {
                    m.push((k, v));
                }
            }
        }
        let g = graph_name(graph_of(n));
        let slot = match graphs.iter().position(|(name, _)| *name == g) {
            Some(i) => i,
            None => {
                graphs.push((g, Vec::new()));
                graphs.len() - 1
            }
        };
        graphs[slot].1.push((leaf(&n.path).to_string(), Val::Map(m)));
    }
    Val::Map(graphs.into_iter().map(|(g, nodes)| (g, Val::Map(nodes))).collect())
}

// ---------------------------------------------------------------------------
// Materials and material functions

pub fn expression_label(class: &str, props: &Val) -> String {
    let ty = leaf(class).strip_prefix("MaterialExpression").unwrap_or(leaf(class)).to_string();
    let s = |k: &str| str_at(props, &[k]);
    let number = |k: &str| props.get(k).map(super::diff::scalar);
    match ty.as_str() {
        "CollectionParameter" => {
            format!("MPC {}.{}", s("Collection").map(leaf).unwrap_or("?"), s("ParameterName").unwrap_or("?"))
        }
        t if t.contains("Parameter") && s("ParameterName").is_some() => format!("{t} '{}'", s("ParameterName").unwrap()),
        "MaterialFunctionCall" => format!("Call {}", s("MaterialFunction").map(leaf).unwrap_or("?")),
        "TextureSample" | "TextureObject" => format!("{ty} {}", s("Texture").map(leaf).unwrap_or("")),
        "Custom" => format!("Custom '{}'", s("Description").unwrap_or("Custom")),
        "Comment" => format!("Comment: {}", s("Text").unwrap_or("")),
        "FunctionInput" => format!("Input '{}'", s("InputName").unwrap_or("In")),
        "FunctionOutput" => format!("Output '{}'", s("OutputName").unwrap_or("Result")),
        "NamedRerouteDeclaration" => format!("Named reroute '{}'", s("Name").unwrap_or("?")),
        "Constant" => format!("Constant {}", number("R").unwrap_or("0".into())),
        "SceneTexture" => format!("SceneTexture {}", s("SceneTextureId").unwrap_or("")),
        _ => ty,
    }
}

/// Every expression of a material or function (plus the material's own
/// result pins), node by node.
pub fn material_graph(pkg: &Package, main: usize) -> Val {
    let mut exprs: Vec<(usize, String, Val)> = Vec::new(); // export, path, props
    for (i, e) in pkg.exports.iter().enumerate() {
        if leaf(&pkg.export_class(e)).starts_with("MaterialExpression") {
            exprs.push((i, pkg.export_path(i), properties(pkg, i)));
        }
    }
    let class_of = |i: usize| pkg.export_class(&pkg.exports[i]);
    let mut labels: HashMap<String, String> = HashMap::new();
    let mut reroutes: HashMap<String, Val> = HashMap::new();
    let mut named: HashMap<String, String> = HashMap::new(); // declaration path -> label
    for (i, path, props) in &exprs {
        let class = class_of(*i);
        if leaf(&class) == "MaterialExpressionReroute" {
            reroutes.insert(path.clone(), props.clone());
            continue;
        }
        let l = expression_label(&class, props);
        if leaf(&class) == "MaterialExpressionNamedRerouteDeclaration" {
            named.insert(path.clone(), l.clone());
        }
        labels.insert(path.clone(), l);
    }
    // A named reroute usage reads as the declaration it stands for.
    for (i, path, props) in &exprs {
        if leaf(&class_of(*i)) == "MaterialExpressionNamedRerouteUsage" {
            let decl = str_at(props, &["Declaration"]).and_then(|d| named.get(d)).cloned();
            labels.insert(path.clone(), format!("Use {}", decl.unwrap_or_else(|| "named reroute".into())));
        }
    }
    // An FExpressionInput as "Source label", through plain reroutes.
    let link = |input: &Val| -> Option<String> {
        let mut cur = input.clone();
        for _ in 0..64 {
            let source = str_at(&cur, &["Expression"])?.to_string();
            let output = match cur.get("OutputIndex") {
                Some(Val::Int(n)) if *n > 0 => format!(" ›out{n}"),
                _ => String::new(),
            };
            match reroutes.get(&source).and_then(|r| r.get("Input")) {
                Some(next) => cur = next.clone(),
                None => return Some(format!("← {}{output}", labels.get(&source).cloned().unwrap_or_else(|| leaf(&source).to_string()))),
            }
        }
        None
    };
    // Inputs read as links; lists of named inputs (function calls, Custom
    // nodes) as one link per input name.
    let node = |label: String, props: Val| -> Val {
        let mut m: Vec<(String, Val)> = vec![(LABEL.into(), Val::Str(label))];
        let Val::Map(p) = strip(props) else { return Val::Map(m) };
        for (k, v) in p {
            if v.get("Expression").is_some() {
                if let Some(l) = link(&v) {
                    m.push((k, Val::Str(l)));
                }
                continue;
            }
            if let Val::List(items) = &v {
                let inputs: Vec<(String, Option<String>)> = items
                    .iter()
                    .filter_map(|it| {
                        let input = it.get("Input")?;
                        let name = str_at(input, &["InputName"]).or(str_at(it, &["InputName"])).unwrap_or("?").to_string();
                        Some((name, link(input)))
                    })
                    .collect();
                if !inputs.is_empty() && inputs.len() == items.len() {
                    for (name, l) in inputs {
                        if let Some(l) = l {
                            m.push((name, Val::Str(l)));
                        }
                    }
                    continue;
                }
            }
            m.push((k, v));
        }
        Val::Map(m)
    };

    let mut out: Vec<(String, Val)> = Vec::new();
    // The material's result pins live on its editor-only data object.
    let editor = str_at(&properties(pkg, main), &["EditorOnlyData"]).map(String::from);
    if let Some(i) = editor.and_then(|e| (0..pkg.exports.len()).find(|&i| pkg.export_path(i) == e)) {
        let result = node(pkg.exports[main].object_name.clone(), properties(pkg, i));
        if let Val::Map(m) = &result {
            // Only the pins: everything else on that object is bookkeeping.
            let pins: Vec<(String, Val)> =
                m.iter().filter(|(k, v)| k == LABEL || v.as_str().is_some_and(|s| s.starts_with('←'))).cloned().collect();
            if pins.len() > 1 {
                out.push(("Result".into(), Val::Map(pins)));
            }
        }
    }
    for (_, path, props) in exprs {
        if let Some(l) = labels.get(&path).cloned() {
            out.push((leaf(&path).to_string(), node(l, props)));
        }
    }
    Val::Map(out)
}
