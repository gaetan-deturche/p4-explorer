//! Per-asset-type summaries: what a person means by "what changed in this
//! material" — its effective settings and its parameters — rather than the
//! objects the package happens to store them in.
//!
//! A package saves a property only when it differs from the class default, so
//! settings come from the asset registry tags where the editor writes the
//! effective values (blend mode, shading model, domain).

use std::path::PathBuf;

use super::archive::Val;
use super::doc::{leaf, properties, NOISE};
use super::mesh;
use super::package::Package;

/// The summary sections leading the document, or None for an asset type
/// without one. `roots` lead to the engine whose Oodle unpacks mesh sources.
pub fn summary(pkg: &Package, roots: &[PathBuf]) -> Option<Vec<(String, Val)>> {
    let main = pkg.exports.iter().position(|e| e.is_asset)?;
    let class = pkg.export_class(&pkg.exports[main]);
    Some(match leaf(&class) {
        "Material" => material(pkg, main),
        c if c.starts_with("MaterialInstance") => material_instance(pkg, main),
        c if c.starts_with("MaterialFunction") => material_function(pkg, main),
        c if c.ends_with("Blueprint") => blueprint(pkg, main),
        "StaticMesh" => static_mesh(pkg, main, roots),
        "SkeletalMesh" => skeletal_mesh(pkg, main, roots),
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

/// A Blueprint variable as "Name → {Type, Category, Default, …}".
fn variable(v: &Val) -> (String, Val) {
    let name = v.get("VarName").and_then(|n| n.as_str()).unwrap_or("?").to_string();
    let mut m = Vec::new();
    if let Some(ty) = v.get("VarType") {
        m.push(("Type".to_string(), Val::Str(super::doc::type_text(ty))));
    }
    if let Val::Map(fields) = v {
        for (k, f) in fields {
            let keep = match k.as_str() {
                "VarName" | "VarGuid" | "VarType" | "FriendlyName" => false,
                "RepNotifyFunc" => f.as_str() != Some("None"),
                "ReplicationCondition" => f.as_str() != Some("COND_None"),
                "MetaDataArray" => !matches!(f, Val::List(l) if l.is_empty()),
                "DefaultValue" => f.as_str() != Some(""),
                _ => true,
            };
            if keep {
                m.push((k.clone(), f.clone()));
            }
        }
    }
    (name, Val::Map(m))
}

/// The Blueprint asset's own settings and the generated class's defaults
/// (what the Class Defaults panel shows as changed from the parent).
fn blueprint(pkg: &Package, main: usize) -> Vec<(String, Val)> {
    let cdo_name = format!("Default__{}_C", pkg.exports[main].object_name);
    let skip = ["LastEditedDocuments", "UbergraphPages", "FunctionGraphs", "NewVariables"];
    let mut out = vec![("Blueprint".to_string(), saved(pkg, main, &skip))];
    if let Some(Val::List(vars)) = map(properties(pkg, main)).into_iter().find(|(k, _)| k == "NewVariables").map(|(_, v)| v) {
        out.push(("Variables".into(), Val::Map(vars.iter().map(variable).collect())));
    }
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

/// A LOD's source geometry as the summary shows it, or why there is none.
fn lod_geometry(
    lod: &mesh::SourceLod,
    decoded: &Result<Option<std::sync::Arc<mesh::Geometry>>, String>,
    material: &dyn Fn(&str) -> Option<String>,
) -> (bool, Vec<(String, Val)>) {
    match decoded {
        Ok(Some(g)) => (true, mesh::stats(g, material)),
        Err(e) => (false, vec![("Source".into(), Val::Str(format!("unreadable: {e}")))]),
        Ok(None) => (false, vec![("Source".into(), Val::str(mesh::missing_source(lod)))]),
    }
}

/// A source model's settings after its geometry, minus the bulk data pointer and,
/// when the geometry was read, the counts the editor caches from it.
fn lod_settings(out: &mut Vec<(String, Val)>, settings: &Val, decoded: bool, cached: &[&str]) {
    for (k, v) in map(settings.clone()) {
        if k == "StaticMeshDescriptionBulkData" || k == "MeshDescriptionBulkData" || (decoded && cached.contains(&k.as_str())) {
            continue;
        }
        out.push((k, v));
    }
}

/// The export an object property points at.
fn export_at(pkg: &Package, path: Option<&Val>) -> Option<usize> {
    let path = path?.as_str()?;
    (0..pkg.exports.len()).find(|&i| pkg.export_path(i) == path)
}

/// Collision primitives by kind; a convex hull as its size and a fingerprint of
/// its points rather than every point.
fn collision(pkg: &Package, body: Option<usize>) -> Val {
    let Some(body) = body else { return Val::Map(Vec::new()) };
    let mut props = map(properties(pkg, body));
    let mut out = vec![("Complexity".to_string(), take(&mut props, "CollisionTraceFlag").unwrap_or(Val::str("CTF_UseDefault")))];
    if let Some(profile) = props.iter().find(|(k, _)| k == "DefaultInstance").and_then(|(_, v)| v.get("CollisionProfileName")) {
        out.push(("Profile".into(), profile.clone()));
    }
    let geom = map(take(&mut props, "AggGeom").unwrap_or(Val::Map(Vec::new())));
    for (key, label, one) in [
        ("BoxElems", "Boxes", "Box"),
        ("SphereElems", "Spheres", "Sphere"),
        ("SphylElems", "Capsules", "Capsule"),
        ("TaperedCapsuleElems", "Tapered capsules", "Tapered capsule"),
        ("ConvexElems", "Convex hulls", "Convex hull"),
        ("LevelSetElems", "Level sets", "Level set"),
        ("SkinnedLevelSetElems", "Skinned level sets", "Skinned level set"),
    ] {
        let Some(Val::List(elems)) = geom.iter().find(|(k, _)| k == key).map(|(_, v)| v) else { continue };
        if elems.is_empty() {
            continue;
        }
        let shown = elems
            .iter()
            .enumerate()
            .map(|(i, e)| (format!("{one} {i}"), if key == "ConvexElems" { convex_hull(e) } else { e.clone() }))
            .collect();
        out.push((label.into(), Val::Map(shown)));
    }
    out.extend(props.into_iter().filter(|(k, _)| !DERIVED.contains(&k.as_str()) && k != "BodySetupGuid"));
    Val::Map(out)
}

fn convex_hull(e: &Val) -> Val {
    let coord = |p: &Val, k: &str| match p.get(k) {
        Some(Val::Float(f)) => *f,
        _ => 0.0,
    };
    let points: Vec<[f64; 3]> = match e.get("VertexData") {
        Some(Val::List(v)) => v.iter().map(|p| [coord(p, "X"), coord(p, "Y"), coord(p, "Z")]).collect(),
        _ => Vec::new(),
    };
    let mut lo = [f64::MAX; 3];
    let mut hi = [f64::MIN; 3];
    let mut h = sha1_smol::Sha1::new();
    for p in &points {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
            h.update(&((p[k] * 1000.0).round() as i64).to_le_bytes());
        }
    }
    let r = |v: f64| super::diff::scalar(&Val::Float((v * 100.0).round() / 100.0));
    let mut m = vec![("Points".to_string(), Val::Int(points.len() as i64))];
    if !points.is_empty() {
        m.push(("Size".into(), Val::Str(format!("{} × {} × {}", r(hi[0] - lo[0]), r(hi[1] - lo[1]), r(hi[2] - lo[2])))));
        m.push(("Center".into(), Val::Str(format!("{}, {}, {}", r((hi[0] + lo[0]) / 2.0), r((hi[1] + lo[1]) / 2.0), r((hi[2] + lo[2]) / 2.0)))));
        m.push(("Shape".into(), Val::Str(format!("sha1 {}", &h.digest().to_string()[..12]))));
    }
    if let Val::Map(fields) = e {
        m.extend(fields.iter().filter(|(k, _)| k != "VertexData" && k != "ElemBox" && k != "IndexData").cloned());
    }
    Val::Map(m)
}

/// The asset registry tags: what the editor computed at save (built triangle
/// counts, Nanite and distance field sizes).
fn build_stats(pkg: &Package, main: usize, skip: &[&str]) -> Val {
    let mut tags = registry_tags(pkg, main);
    tags.retain(|(k, _)| !skip.contains(&k.as_str()));
    Val::Map(tags)
}

fn static_mesh(pkg: &Package, main: usize, roots: &[PathBuf]) -> Vec<(String, Val)> {
    let mut props = map(properties(pkg, main));
    let slots = mesh::material_slots(pkg, main);
    take(&mut props, "StaticMaterials");
    let material = |group: &str| mesh::slot_material(&slots, group);
    // FMeshSectionInfoMap keys are LOD << 16 | section.
    let mut section_info: Vec<(i64, String, Val)> = Vec::new();
    if let Some(Val::List(entries)) = take(&mut props, "SectionInfoMap").and_then(|m| m.get("Map").cloned()) {
        for e in &entries {
            if let (Some(Val::Int(key)), Some(v)) = (e.get("key"), e.get("value")) {
                section_info.push((key >> 16, format!("Section {}", key & 0xffff), v.clone()));
            }
        }
    }
    take(&mut props, "OriginalSectionInfoMap");
    let mut lods = Vec::new();
    let source = mesh::source_lods(pkg);
    let geometries = mesh::geometries(pkg, &source, roots);
    for (i, (lod, g)) in source.iter().zip(&geometries).enumerate() {
        let (decoded, mut m) = lod_geometry(lod, g, &material);
        lod_settings(&mut m, &lod.settings, decoded, &["CacheMeshDescriptionTrianglesCount", "CacheMeshDescriptionVerticesCount"]);
        let sections: Vec<(String, Val)> = section_info
            .iter()
            .filter(|(l, _, _)| *l == i as i64 && lod.name != mesh::HI_RES)
            .map(|(_, s, v)| (s.clone(), v.clone()))
            .collect();
        if !sections.is_empty() {
            m.push(("Section settings".into(), Val::Map(sections)));
        }
        lods.push((lod.name.clone(), Val::Map(m)));
    }
    take(&mut props, "SourceModels");
    take(&mut props, "HiResSourceModel");
    let materials = slots.iter().map(|(slot, _, mat)| (slot.clone(), Val::Str(mat.clone()))).collect();
    let nanite = take(&mut props, "NaniteSettings").unwrap_or(Val::Map(Vec::new()));
    // UStaticMesh::Serialize writes its BodySetup natively: find the one it owns.
    let body = pkg.exports.iter().position(|e| e.outer_index == main as i32 + 1 && leaf(&pkg.export_class(e)) == "BodySetup");
    let mut sockets: Vec<(String, Val)> = Vec::new();
    if let Some(Val::List(list)) = take(&mut props, "Sockets") {
        for s in &list {
            let Some(e) = export_at(pkg, Some(s)) else { continue };
            let mut p = map(properties(pkg, e));
            let name = take(&mut p, "SocketName").and_then(|n| n.as_str().map(String::from)).unwrap_or(pkg.exports[e].object_name.clone());
            insert_unique(&mut sockets, name, Val::Map(p));
        }
    }
    for k in ["BodySetup", "NavCollision", "AssetImportData", "ThumbnailInfo"] {
        take(&mut props, k);
    }
    let mut out = vec![
        ("LODs".to_string(), Val::Map(lods)),
        ("Materials".into(), Val::Map(materials)),
        ("Nanite".into(), nanite),
        ("Collision".into(), collision(pkg, body)),
    ];
    if !sockets.is_empty() {
        out.push(("Sockets".into(), Val::Map(sockets)));
    }
    out.push(("Settings".into(), Val::Map(props.into_iter().filter(|(k, _)| !DERIVED.contains(&k.as_str())).collect())));
    out.push(("Build stats".into(), build_stats(pkg, main, &["AssetImportData", "StaticMaterials"])));
    out
}

fn skeletal_mesh(pkg: &Package, main: usize, roots: &[PathBuf]) -> Vec<(String, Val)> {
    let mut props = map(properties(pkg, main));
    let native = mesh::skeletal_native(pkg, main);
    let slots = native.as_ref().map(|n| n.materials.clone()).unwrap_or_default();
    let material = |group: &str| mesh::slot_material(&slots, group);
    let info = match take(&mut props, "LODInfo") {
        Some(Val::List(l)) => l,
        _ => Vec::new(),
    };
    let cached = [
        "TriangleCount",
        "VertexCount",
        "Bounds",
        "CachedSkinWeightProfileNames",
        "CachedMorphTargetNames",
        "CachedHasVertexColors",
        "CachedVertexColorGuid",
    ];
    let mut lods = Vec::new();
    let source = mesh::source_lods(pkg);
    let geometries = mesh::geometries(pkg, &source, roots);
    for (i, (lod, g)) in source.iter().zip(&geometries).enumerate() {
        let (decoded, mut m) = lod_geometry(lod, g, &material);
        lod_settings(&mut m, &lod.settings, decoded, &cached);
        if let Some(settings) = info.get(i) {
            m.extend(map(settings.clone()));
        }
        lods.push((lod.name.clone(), Val::Map(m)));
    }
    take(&mut props, "SourceModels");
    let mut out = vec![("LODs".to_string(), Val::Map(lods))];
    let mut skeleton = vec![
        ("Skeleton".to_string(), take(&mut props, "Skeleton").unwrap_or(Val::Null)),
        ("Physics asset".into(), take(&mut props, "PhysicsAsset").unwrap_or(Val::Null)),
    ];
    match &native {
        Ok(n) => {
            out.push(("Materials".into(), Val::Map(n.materials.iter().map(|(slot, _, mat)| (slot.clone(), Val::Str(mat.clone()))).collect())));
            let parent = |p: i32| usize::try_from(p).ok().and_then(|p| n.bones.get(p)).map(|b| b.0.clone()).unwrap_or("(root)".into());
            let bones = n.bones.iter().map(|(name, p)| (name.clone(), Val::Str(parent(*p)))).collect();
            skeleton.push(("Bones (parent)".into(), Val::Map(bones)));
        }
        Err(e) => out.push(("Materials".into(), Val::Str(e.clone()))),
    }
    out.push(("Skeleton".into(), Val::Map(skeleton)));
    let morphs: Vec<Val> = registry_tags(pkg, main)
        .into_iter()
        .find(|(k, _)| k == "MorphTargetNames")
        .and_then(|(_, v)| v.as_str().map(|s| s.split(';').filter(|m| !m.is_empty()).map(Val::str).collect()))
        .unwrap_or_default();
    if !morphs.is_empty() {
        out.push(("Morph targets".into(), Val::List(morphs)));
    }
    take(&mut props, "AssetImportData");
    out.push(("Settings".into(), Val::Map(props.into_iter().filter(|(k, _)| !DERIVED.contains(&k.as_str())).collect())));
    out.push(("Build stats".into(), build_stats(pkg, main, &["AssetImportData", "MorphTargetNames", "MaterialParamNames"])));
    out
}
