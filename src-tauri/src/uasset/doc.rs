//! A package as one comparable tree: what two revisions of an asset are diffed
//! on. Keys are stable across saves (object paths, pin names, property names),
//! and what moves on every save for no reason (file offsets, saved hashes,
//! name-table order) is left out.

use super::archive::Val;
use super::graph::{read_graph, GraphNode, Pin};
use super::package::{flag_names, Package, OBJECT_FLAGS, PACKAGE_FLAGS};
use super::properties::read_export_properties;

/// Bytes of an undecodable value kept for display; the rest is fingerprinted.
const MAX_RAW: usize = 48;

/// Where the package's raw contents go when an asset-type summary leads.
pub const ALL_DATA: &str = "All data";
pub const GRAPHS: &str = "Graphs";

fn text_graphs(pkg: &Package, graph: &[GraphNode]) -> Option<Val> {
    let main = pkg.exports.iter().position(|e| e.is_asset)?;
    let class = leaf(&pkg.export_class(&pkg.exports[main])).to_string();
    if class == "Material" || class.starts_with("MaterialFunction") {
        let nodes = super::graphs::material_graph(pkg, main);
        return Some(Val::Map(vec![(super::canvas::MATERIAL_GRAPH.into(), nodes)]));
    }
    (!graph.is_empty()).then(|| super::graphs::blueprint_graphs(pkg, graph))
}

/// A graph node's text form inside a document, for the drawn graph's "changed".
pub fn graph_node<'d>(doc: &'d Val, graph: &str, node: &str) -> Option<&'d Val> {
    let root = doc.get(ALL_DATA).unwrap_or(doc);
    root.get(GRAPHS)?.get(graph)?.get(node)
}

pub fn document(pkg: &Package) -> Val {
    let graph = read_graph(pkg);
    let mut raw: Vec<(String, Val)> = Vec::new();
    raw.push(("Package".into(), package_info(pkg)));
    if !pkg.asset_data.is_empty() {
        raw.push(("Asset registry".into(), asset_registry(pkg)));
    }
    raw.push(("Dependencies".into(), dependencies(pkg)));
    // The graphs as text: what a node's details panel shows, and the "changed"
    // test of the drawn graphs.
    if let Some(g) = text_graphs(pkg, &graph) {
        raw.push((GRAPHS.into(), g));
    }
    raw.push(("Objects".into(), objects(pkg, &graph)));
    match super::summary::summary(pkg) {
        Some(mut top) => {
            top.push((ALL_DATA.into(), Val::Map(raw)));
            Val::Map(top)
        }
        None => Val::Map(raw),
    }
}

/// Saved on every edit for no reason a reader cares about, or graph layout that
/// belongs to a graph view rather than a property list.
pub const NOISE: &[&str] = &[
    "StateId",
    "LightingGuid",
    "ParameterStateId",
    "ExpressionGUID",
    "MaterialExpressionGuid",
    "EditorX",
    "EditorY",
    "MaterialExpressionEditorX",
    "MaterialExpressionEditorY",
    "ThumbnailInfo",
];

pub fn strip_noise(v: Val) -> Val {
    match v {
        Val::Map(m) => Val::Map(
            m.into_iter().filter(|(k, _)| !NOISE.contains(&k.as_str())).map(|(k, v)| (k, strip_noise(v))).collect(),
        ),
        Val::List(l) => Val::List(l.into_iter().map(strip_noise).collect()),
        other => other,
    }
}

/// One export's tagged properties, as the diff shows them.
pub fn properties(pkg: &Package, index: usize) -> Val {
    strip_noise(raw_properties(pkg, index))
}

/// The same with layout kept (node positions), for drawing a graph.
pub fn raw_properties(pkg: &Package, index: usize) -> Val {
    fold_undecoded(read_export_properties(pkg, &pkg.exports[index], MAX_RAW, true))
}

/// Material graph nodes: their wiring is a graph view's job, and their
/// parameters already lead the material's summary.
fn is_material_graph_object(class: &str) -> bool {
    let leaf = leaf(class);
    leaf.starts_with("MaterialExpression") || leaf.starts_with("MaterialGraph")
}

fn package_info(pkg: &Package) -> Val {
    let s = &pkg.summary;
    Val::map(vec![
        ("Saved by", Val::str(&s.saved_by_engine_version)),
        ("Versions", Val::Str(format!("UE4 {} / UE5 {} / licensee {}", s.file_version_ue4, s.file_version_ue5, s.file_version_licensee))),
        ("Flags", Val::Str(flag_names(s.package_flags, PACKAGE_FLAGS).join(" "))),
    ])
}

fn asset_registry(pkg: &Package) -> Val {
    Val::Map(
        pkg.asset_data
            .iter()
            .map(|a| {
                let mut m = vec![("Class".to_string(), Val::str(&a.class))];
                m.extend(a.tags.iter().map(|(k, v)| (k.clone(), Val::str(v))));
                (a.object_path.clone(), Val::Map(m))
            })
            .collect(),
    )
}

/// Order-free lists, sorted so that only membership changes show.
fn sorted(mut items: Vec<String>) -> Val {
    items.sort();
    items.dedup();
    Val::List(items.into_iter().map(Val::Str).collect())
}

fn dependencies(pkg: &Package) -> Val {
    let imports = (0..pkg.imports.len())
        .filter(|&i| pkg.imports[i].class_name != "Package")
        .map(|i| format!("{} {}", pkg.imports[i].class_name, pkg.import_path(i)))
        .collect();
    let hard = pkg
        .imports
        .iter()
        .filter(|i| i.outer_index == 0 && i.class_name == "Package")
        .map(|i| i.object_name.clone())
        .collect();
    let mut m = vec![("Hard packages", sorted(hard)), ("Imported objects", sorted(imports))];
    if !pkg.soft_package_references.is_empty() {
        m.push(("Soft packages", sorted(pkg.soft_package_references.clone())));
    }
    if !pkg.soft_object_paths.is_empty() {
        m.push(("Soft object paths", sorted(pkg.soft_object_paths.clone())));
    }
    Val::map(m)
}

pub fn leaf(path: &str) -> &str {
    path.rsplit([':', '.']).next().unwrap_or(path)
}

/// A node's path inside its asset ("EventGraph:K2Node_CallFunction_3").
fn node_name(path: &str) -> &str {
    path.split_once('.').map(|(_, rest)| rest).unwrap_or(path)
}

/// "exec", "object<Actor>", "float[Array]" — a pin type as one line.
fn pin_type(pin: &Pin) -> String {
    type_text(&pin.ty)
}

/// The same, for a decoded FEdGraphPinType.
pub fn type_text(ty: &Val) -> String {
    let get = |k: &str| ty.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let category = get("PinCategory");
    let mut sub = get("PinSubCategory");
    let obj = get("PinSubCategoryObject");
    if !obj.is_empty() && obj != "None" {
        sub = leaf(&obj).to_string();
    }
    let mut text = if sub.is_empty() || sub == "None" { category } else { format!("{category}<{sub}>") };
    let container = get("ContainerType");
    if !container.is_empty() && container != "None" {
        text = format!("{text}[{container}]");
    }
    text
}

fn pin_val(pin: &Pin) -> Val {
    let mut m = vec![
        ("Direction", pin.direction.clone()),
        ("Type", Val::Str(pin_type(pin))),
    ];
    if !pin.default_value.is_empty() {
        m.push(("Default", Val::str(&pin.default_value)));
    }
    if pin.default_object != "None" {
        m.push(("Default object", Val::str(&pin.default_object)));
    }
    if let Some(t) = pin.rest("default_text_value") {
        if *t != Val::str("") {
            m.push(("Default text", t.clone()));
        }
    }
    let links: Vec<Val> = pin
        .linked_to
        .iter()
        .map(|l| Val::Str(format!("{}.{}", node_name(&l.node), l.pin.as_deref().unwrap_or("<unknown>"))))
        .collect();
    if !links.is_empty() {
        m.push(("Linked to", Val::List(links)));
    }
    for (flag, label) in [
        ("hidden", "Hidden"),
        ("not_connectable", "Not connectable"),
        ("advanced_view", "Advanced"),
        ("orphaned", "Orphaned"),
        ("default_value_ignored", "Default ignored"),
    ] {
        if pin.flag(flag) {
            m.push((label, Val::Bool(true)));
        }
    }
    Val::map(m)
}

fn objects(pkg: &Package, graph: &[GraphNode]) -> Val {
    let by_export: std::collections::HashMap<usize, &GraphNode> = graph.iter().map(|n| (n.export, n)).collect();
    let mut out = Vec::new();
    for (index, exp) in pkg.exports.iter().enumerate() {
        let class = pkg.export_class(exp);
        if is_material_graph_object(&class) {
            continue;
        }
        let mut m: Vec<(String, Val)> = vec![("Class".into(), Val::Str(class))];
        if exp.super_index != 0 {
            m.push(("Super".into(), Val::Str(pkg.resolve_index(exp.super_index))));
        }
        let flags = flag_names(exp.object_flags, OBJECT_FLAGS);
        if !flags.is_empty() {
            m.push(("Flags".into(), Val::Str(flags.join(" "))));
        }
        let props = properties(pkg, index);
        if !matches!(&props, Val::Map(p) if p.is_empty()) {
            m.push(("Properties".into(), props));
        }
        let node = by_export.get(&index);
        match node.and_then(|n| n.pins.as_ref()) {
            Some(pins) => {
                let mut named: Vec<(String, Val)> = Vec::new();
                for pin in pins {
                    let mut key = pin.name.clone();
                    let mut n = 2;
                    while named.iter().any(|(k, _)| *k == key) {
                        key = format!("{} #{n}", pin.name);
                        n += 1;
                    }
                    named.push((key, pin_val(pin)));
                }
                m.push(("Pins".into(), Val::Map(named)));
            }
            // The pins ARE the native data of a graph node; anything else
            // (mesh data, bulk payloads) can only be compared as bytes.
            None => {
                if let Some(native) = native_data(pkg, exp) {
                    m.push(("Native data".into(), native));
                }
            }
        }
        out.push((pkg.export_path(index), Val::Map(m)));
    }
    Val::Map(out)
}

/// An undecodable value as one line — "StateTreeReference · 75 bytes · sha1 …" —
/// instead of its raw/size/type/reason fields: one row that changes when any
/// of its bytes do.
fn fold_undecoded(v: Val) -> Val {
    match v {
        Val::Map(m) if m.iter().any(|(k, _)| k == "__raw") => {
            let field = |key: &str| m.iter().find(|(k, _)| k == key).map(|(_, v)| v);
            let ty = field("__type").and_then(|t| t.as_str()).unwrap_or("?");
            // "StructProperty(StateTreeReference(/Script/StateTreeModule))" -> "StateTreeReference"
            let short = ty
                .strip_prefix("StructProperty(")
                .map(|rest| rest.split(['(', ')']).next().unwrap_or(rest))
                .unwrap_or(ty);
            let size = match field("__size") {
                Some(Val::Int(n)) => *n,
                _ => 0,
            };
            let raw = field("__raw").and_then(|r| r.as_str()).unwrap_or("");
            let print = match field("__sha1").and_then(|s| s.as_str()) {
                Some(sha) => format!("sha1 {}", &sha[..12.min(sha.len())]),
                None => raw.to_string(),
            };
            Val::Str(format!("{short} · {size} bytes · {print}"))
        }
        Val::Map(m) => Val::Map(m.into_iter().map(|(k, v)| (k, fold_undecoded(v))).collect()),
        Val::List(l) => Val::List(l.into_iter().map(fold_undecoded).collect()),
        other => other,
    }
}

fn native_data(pkg: &Package, exp: &super::package::ObjectExport) -> Option<Val> {
    let data = pkg.export_data(exp);
    let start = if exp.script_end > 0 { (exp.script_end as usize).min(data.len()) } else { 0 };
    let tail = &data[start..];
    if tail.is_empty() {
        return None;
    }
    let digest = sha1_smol::Sha1::from(tail).digest().to_string();
    Some(Val::Str(format!("{} bytes, sha1 {}", tail.len(), &digest[..12])))
}

/// The package's first thumbnail as a data URL, when it is an image a browser
/// can show (thumbnails are stored PNG- or JPEG-compressed).
pub fn thumbnail_url(pkg: &Package) -> Option<String> {
    let t = pkg.thumbnails.first()?;
    let bytes = pkg.thumbnail_bytes(t);
    let mime = if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        "image/png"
    } else if bytes.starts_with(&[0xFF, 0xD8]) {
        "image/jpeg"
    } else {
        return None;
    };
    Some(format!("data:{mime};base64,{}", base64(bytes)))
}

fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The asset's class, for the window title ("Blueprint", "Material").
pub fn main_class(pkg: &Package) -> String {
    pkg.exports
        .iter()
        .find(|e| e.is_asset)
        .map(|e| leaf(&pkg.export_class(e)).to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::base64;

    #[test]
    fn base64_matches_the_standard_alphabet_and_padding() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }
}
