//! Per-asset-type summaries: what a person means by "what changed in this
//! material" — its effective settings and its parameters — rather than the
//! objects the package happens to store them in.
//!
//! A package saves a property only when it differs from the class default, so
//! settings come from the asset registry tags where the editor writes the
//! effective values (blend mode, shading model, domain).

use super::archive::Val;
use super::doc::{leaf, properties, NOISE};
use super::package::Package;

/// The summary sections leading the document, or None for an asset type
/// without one.
pub fn summary(pkg: &Package) -> Option<Vec<(String, Val)>> {
    let main = pkg.exports.iter().position(|e| e.is_asset)?;
    let class = pkg.export_class(&pkg.exports[main]);
    Some(match leaf(&class) {
        "Material" => material(pkg, main),
        c if c.starts_with("MaterialInstance") => material_instance(pkg, main),
        c if c.starts_with("MaterialFunction") => material_function(pkg, main),
        c if c.ends_with("Blueprint") => blueprint(pkg, main),
        _ => vec![("Properties".into(), saved(pkg, main, &[]))],
    })
}

/// What the object saved, minus derived data and `skip`.
fn saved(pkg: &Package, index: usize, skip: &[&str]) -> Val {
    Val::Map(
        map(properties(pkg, index))
            .into_iter()
            .filter(|(k, _)| !skip.contains(&k.as_str()) && !DERIVED.contains(&k.as_str()))
            .collect(),
    )
}

/// The Blueprint asset's own settings and the generated class's defaults
/// (what the Class Defaults panel shows as changed from the parent).
fn blueprint(pkg: &Package, main: usize) -> Vec<(String, Val)> {
    let cdo_name = format!("Default__{}_C", pkg.exports[main].object_name);
    let mut out = vec![("Blueprint".to_string(), saved(pkg, main, &["LastEditedDocuments", "UbergraphPages", "FunctionGraphs"]))];
    let cdo = pkg.exports.iter().position(|e| e.outer_index == 0 && e.object_name == cdo_name);
    if let Some(cdo) = cdo {
        out.push(("Class defaults".into(), saved(pkg, cdo, &["UberGraphFrame"])));
    }
    // Component templates: the inherited ones the class defaults own, and the
    // ones its construction script adds (`<Name>_GEN_VARIABLE`).
    let mut components: Vec<(String, Val)> = Vec::new();
    for (i, e) in pkg.exports.iter().enumerate() {
        let name = if cdo.is_some_and(|c| e.outer_index == c as i32 + 1) {
            e.object_name.clone()
        } else if let Some(n) = e.object_name.strip_suffix("_GEN_VARIABLE") {
            n.to_string()
        } else {
            continue;
        };
        let props = saved(pkg, i, &[]);
        if !matches!(&props, Val::Map(m) if m.is_empty()) {
            insert_unique(&mut components, name, props);
        }
    }
    if !components.is_empty() {
        out.push(("Components".into(), Val::Map(components)));
    }
    out
}

fn map(v: Val) -> Vec<(String, Val)> {
    match v {
        Val::Map(m) => m,
        _ => Vec::new(),
    }
}

fn take(m: &mut Vec<(String, Val)>, key: &str) -> Option<Val> {
    let at = m.iter().position(|(k, _)| k == key)?;
    Some(m.remove(at).1)
}

/// The asset registry tags of the main object, minus its class.
fn registry_tags(pkg: &Package, main: usize) -> Vec<(String, Val)> {
    let name = &pkg.exports[main].object_name;
    let entry = pkg
        .asset_data
        .iter()
        .find(|a| a.object_path.rsplit('.').next() == Some(name.as_str()))
        .or(pkg.asset_data.first());
    entry
        .map(|a| a.tags.iter().map(|(k, v)| (k.clone(), Val::str(v))).collect())
        .unwrap_or_default()
}

/// Cached or derived data the editor rebuilds on save: in "All data", not in a
/// summary of what someone set.
const DERIVED: &[&str] = &[
    "ReferencedTextureGuids",
    "ReferencedTextures",
    "TextureStreamingData",
    "TextureStreamingDataVersion",
    "bHasStaticPermutationResource",
    "CachedExpressionData",
    "EditorOnlyData",
];

/// Effective settings first, then whatever else the asset saved that is not
/// one of them, derived data, or `skip`.
fn settings(pkg: &Package, main: usize, skip: &[&str]) -> Val {
    let mut out = registry_tags(pkg, main);
    for (k, v) in map(properties(pkg, main)) {
        if skip.contains(&k.as_str()) || DERIVED.contains(&k.as_str()) || out.iter().any(|(t, _)| *t == k) {
            continue;
        }
        out.push((k, v));
    }
    Val::Map(out)
}

/// Each parameter expression in the package, keyed by parameter name.
fn parameter_expressions(pkg: &Package) -> Val {
    let mut out: Vec<(String, Val)> = Vec::new();
    for (i, exp) in pkg.exports.iter().enumerate() {
        let class = pkg.export_class(exp);
        let kind = leaf(&class);
        let Some(ty) = kind.strip_prefix("MaterialExpression") else { continue };
        if !ty.contains("Parameter") {
            continue;
        }
        let mut props = map(properties(pkg, i));
        let name = take(&mut props, "ParameterName").and_then(|n| n.as_str().map(String::from)).unwrap_or(exp.object_name.clone());
        for internal in ["Material", "Function", "ParameterId"] {
            take(&mut props, internal);
        }
        // A collection parameter names its value inside an MPC: the MPC is part of its identity.
        let key = match props.iter().find(|(k, _)| k == "Collection").and_then(|(_, v)| v.as_str()) {
            Some(mpc) => format!("{}.{name}", leaf(mpc)),
            None => name,
        };
        let mut m = vec![("Type".to_string(), Val::str(ty))];
        m.extend(props);
        insert_unique(&mut out, key, Val::Map(m));
    }
    Val::Map(out)
}

fn insert_unique(out: &mut Vec<(String, Val)>, key: String, v: Val) {
    let mut k = key.clone();
    let mut n = 2;
    while out.iter().any(|(e, _)| *e == k) {
        k = format!("{key} #{n}");
        n += 1;
    }
    out.push((k, v));
}

fn material(pkg: &Package, main: usize) -> Vec<(String, Val)> {
    vec![
        ("Settings".into(), settings(pkg, main, &["ExpressionCollection"])),
        ("Parameters".into(), parameter_expressions(pkg)),
    ]
}

fn material_function(pkg: &Package, main: usize) -> Vec<(String, Val)> {
    let mut inputs: Vec<(String, Val)> = Vec::new();
    let mut outputs: Vec<(String, Val)> = Vec::new();
    for (i, exp) in pkg.exports.iter().enumerate() {
        let class = pkg.export_class(exp);
        let (is_input, name_key) = match leaf(&class) {
            "MaterialExpressionFunctionInput" => (true, "InputName"),
            "MaterialExpressionFunctionOutput" => (false, "OutputName"),
            _ => continue,
        };
        let mut props = map(properties(pkg, i));
        let name = take(&mut props, name_key).and_then(|n| n.as_str().map(String::from)).unwrap_or(exp.object_name.clone());
        for k in ["Id", "Function", "Material", "bCollapsed", "A", "Preview"] {
            take(&mut props, k);
        }
        if is_input && !props.iter().any(|(k, _)| k == "InputType") {
            props.insert(0, ("InputType".into(), Val::str("FunctionInput_Vector3"))); // the class default
        }
        insert_unique(if is_input { &mut inputs } else { &mut outputs }, name, Val::Map(props));
    }
    let skip = ["DependentFunctionExpressionCandidates", "CombinedInputTypes", "CombinedOutputTypes"];
    vec![
        ("Settings".into(), settings(pkg, main, &skip)),
        ("Inputs".into(), Val::Map(inputs)),
        ("Outputs".into(), Val::Map(outputs)),
        ("Parameters".into(), parameter_expressions(pkg)),
    ]
}

/// "Name", or "Name (layer 2)" for a parameter of a layer or blend.
fn parameter_key(entry: &Val) -> String {
    let info = entry.get("ParameterInfo");
    let name = info.and_then(|i| i.get("Name")).and_then(|n| n.as_str()).unwrap_or("?").to_string();
    match info.and_then(|i| i.get("Association")).and_then(|a| a.as_str()) {
        Some("LayerParameter") | Some("BlendParameter") => {
            let index = info.and_then(|i| i.get("Index")).map(super::diff::scalar).unwrap_or_default();
            format!("{name} (layer {index})")
        }
        _ => name,
    }
}

/// What an override entry sets: the value alone when that is all there is.
fn value_of(entry: &Val, value_key: &str) -> Val {
    match entry {
        Val::Map(m) => {
            let unused_atlas = |k: &str, v: &Val| {
                k == "AtlasData" && v.get("bIsUsedAsAtlasPosition") != Some(&Val::Bool(true))
            };
            let mut v: Vec<(String, Val)> = m
                .iter()
                .filter(|(k, v)| k != "ParameterInfo" && !NOISE.contains(&k.as_str()) && !unused_atlas(k, v))
                .cloned()
                .collect();
            // The value alone when that is all there is.
            if v.len() == 1 && v[0].0 == value_key {
                return v.remove(0).1;
            }
            Val::Map(v)
        }
        other => other.clone(),
    }
}

fn material_instance(pkg: &Package, main: usize) -> Vec<(String, Val)> {
    let mut props = map(properties(pkg, main));
    // Every `*ParameterValues` array: Scalar, Vector, Texture, … and any a
    // project adds (this fork has LineArchetypeParameterValues).
    let mut parameters: Vec<(String, Val)> = Vec::new();
    let arrays: Vec<String> = props
        .iter()
        .filter(|(k, v)| k.ends_with("ParameterValues") && matches!(v, Val::List(_)))
        .map(|(k, _)| k.clone())
        .collect();
    for array in arrays {
        let Some(Val::List(entries)) = take(&mut props, &array) else { continue };
        for e in &entries {
            insert_unique(&mut parameters, parameter_key(e), value_of(e, "ParameterValue"));
        }
    }
    // Static switches: runtime data on the instance, component masks in its editor-only object.
    let mut statics: Vec<(String, Val)> = Vec::new();
    let mut add_statics = |block: Option<Val>| {
        for (_, list) in block.map(map).unwrap_or_default() {
            if let Val::List(entries) = list {
                for e in &entries {
                    // An overridden switch is just its value.
                    let v = match value_of(e, "Value") {
                        Val::Map(f) if f.iter().any(|(k, v)| k == "bOverride" && *v == Val::Bool(true)) => {
                            f.into_iter().find(|(k, _)| k == "Value").map(|(_, v)| v).unwrap_or(Val::Null)
                        }
                        other => other,
                    };
                    insert_unique(&mut statics, parameter_key(e), v);
                }
            }
        }
    };
    add_statics(take(&mut props, "StaticParametersRuntime"));
    if let Some(editor) = props.iter().find(|(k, _)| k == "EditorOnlyData").and_then(|(_, v)| v.as_str()) {
        if let Some(i) = (0..pkg.exports.len()).find(|&i| pkg.export_path(i) == editor) {
            add_statics(take(&mut map(properties(pkg, i)), "StaticParameters"));
        }
    }
    let parent = take(&mut props, "Parent").unwrap_or(Val::Null);
    let overrides = take(&mut props, "BasePropertyOverrides").unwrap_or(Val::Map(Vec::new()));
    take(&mut props, "EditorOnlyData");
    let mut settings = registry_tags(pkg, main);
    settings.retain(|(k, _)| k != "Parent");
    settings.extend(props.into_iter().filter(|(k, _)| !DERIVED.contains(&k.as_str())));
    vec![
        ("Parent".into(), parent),
        ("Settings".into(), Val::Map(settings)),
        ("Property overrides".into(), overrides),
        ("Parameters".into(), Val::Map(parameters)),
        ("Static switches".into(), Val::Map(statics)),
    ]
}
