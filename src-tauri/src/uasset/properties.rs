//! Tagged property reader for uncooked UE5 packages.
//!
//! Uncooked packages serialize every property with a self-describing tag (name,
//! type, size), so properties can be decoded without knowing the C++ class
//! layout. Anything this decoder does not understand is reported as raw bytes:
//! the tag's Size field lets the walk resynchronise, so one unknown type never
//! derails the rest of an object.

use super::archive::{archive_err, decode_err, Error, Reader, Res, Val};
use super::package::*;

// EPropertyTagFlags
const TAG_HAS_ARRAY_INDEX: u8 = 0x01;
const TAG_HAS_PROPERTY_GUID: u8 = 0x02;
const TAG_HAS_PROPERTY_EXTENSIONS: u8 = 0x04;
const TAG_HAS_BINARY_OR_NATIVE_SERIALIZE: u8 = 0x08;
const TAG_BOOL_TRUE: u8 = 0x10;
const TAG_SKIPPED_SERIALIZE: u8 = 0x20;

// EPropertyTagExtension
const EXT_OVERRIDABLE_INFORMATION: u8 = 0x02;

/// A node of FPropertyTypeName: a name plus ordered type parameters.
#[derive(Debug, Clone)]
pub struct TypeName {
    pub name: String,
    pub params: Vec<TypeName>,
}

impl TypeName {
    fn leaf(name: String) -> TypeName {
        TypeName { name, params: Vec::new() }
    }

    fn param(&self, index: usize) -> Option<&TypeName> {
        self.params.get(index)
    }

    fn param_name(&self, index: usize) -> Option<&str> {
        self.param(index).map(|p| p.name.as_str())
    }
}

impl std::fmt::Display for TypeName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.name)?;
        if !self.params.is_empty() {
            let inner: Vec<String> = self.params.iter().map(|p| p.to_string()).collect();
            write!(f, "({})", inner.join(","))?;
        }
        Ok(())
    }
}

pub struct PropertyTag {
    name: String,
    ty: TypeName,
    size: i32,
    array_index: i32,
    bool_val: bool,
    binary_or_native: bool,
    skipped: bool,
}

/// Reads the pre-order node stream that makes up an FPropertyTypeName.
fn read_type_name(ar: &mut Reader) -> Res<TypeName> {
    let name = ar.name()?;
    let inner_count = ar.i32()?;
    if !(0..=64).contains(&inner_count) {
        return archive_err(format!("implausible type name inner count {inner_count}"));
    }
    let mut params = Vec::new();
    for _ in 0..inner_count {
        params.push(read_type_name(ar)?);
    }
    Ok(TypeName { name, params })
}

/// Reads one FPropertyTag. Returns None at the terminating 'None' tag.
fn read_tag(ar: &mut Reader, s: &Summary) -> Res<Option<PropertyTag>> {
    if s.file_version_ue5 >= UE5_PROPERTY_TAG_COMPLETE_TYPE_NAME {
        read_tag_modern(ar)
    } else {
        read_tag_legacy(ar, s)
    }
}

fn read_tag_modern(ar: &mut Reader) -> Res<Option<PropertyTag>> {
    let name = ar.name()?;
    if name == "None" {
        return Ok(None);
    }
    let ty = read_type_name(ar)?;
    let size = ar.i32()?;
    let flags = ar.u8()?;
    let array_index = if flags & TAG_HAS_ARRAY_INDEX != 0 { ar.i32()? } else { 0 };
    if flags & TAG_HAS_PROPERTY_GUID != 0 {
        ar.guid()?;
    }
    if flags & TAG_HAS_PROPERTY_EXTENSIONS != 0 {
        let extensions = ar.u8()?;
        if extensions & EXT_OVERRIDABLE_INFORMATION != 0 {
            ar.u8()?; // OverrideOperation
            ar.bool32()?; // bExperimentalOverridableLogic
        }
    }
    Ok(Some(PropertyTag {
        name,
        ty,
        size,
        array_index,
        binary_or_native: flags & TAG_HAS_BINARY_OR_NATIVE_SERIALIZE != 0,
        skipped: flags & TAG_SKIPPED_SERIALIZE != 0,
        bool_val: flags & TAG_BOOL_TRUE != 0,
    }))
}

/// Pre-5.4 tag layout: partial type info spread over type-specific fields.
fn read_tag_legacy(ar: &mut Reader, s: &Summary) -> Res<Option<PropertyTag>> {
    let name = ar.name()?;
    if name == "None" {
        return Ok(None);
    }
    let type_name = ar.name()?;
    let size = ar.i32()?;
    let array_index = ar.i32()?;
    let mut bool_val = false;
    let mut params = Vec::new();
    match type_name.as_str() {
        "StructProperty" => {
            params.push(TypeName::leaf(ar.name()?));
            if s.file_version_ue4 >= VER_UE4_STRUCT_GUID_IN_PROPERTY_TAG {
                ar.guid()?;
            }
        }
        "BoolProperty" => bool_val = ar.u8()? != 0,
        "ByteProperty" => {
            let enum_name = ar.name()?;
            if enum_name != "None" {
                params.push(TypeName::leaf(enum_name));
            }
        }
        "EnumProperty" => {
            params.push(TypeName::leaf(ar.name()?));
            params.push(TypeName::leaf("ByteProperty".into()));
        }
        "ArrayProperty" | "OptionalProperty" | "SetProperty" => {
            if type_name != "ArrayProperty" || s.file_version_ue4 >= VAR_UE4_ARRAY_PROPERTY_INNER_TAGS {
                params.push(TypeName::leaf(ar.name()?));
            }
        }
        "MapProperty" => {
            params.push(TypeName::leaf(ar.name()?));
            params.push(TypeName::leaf(ar.name()?));
        }
        _ => {}
    }
    // Legacy tags carry no "binary or native" bit; guess from the known-native table.
    let binary_or_native = type_name == "StructProperty" && params.first().is_some_and(|p| is_native_struct(&p.name));
    let ty = TypeName { name: type_name, params };
    if s.file_version_ue4 >= VER_UE4_PROPERTY_GUID_IN_PROPERTY_TAG && ar.u8()? != 0 {
        ar.guid()?;
    }
    if s.file_version_ue5 >= UE5_PROPERTY_TAG_EXTENSION_AND_OVERRIDABLE_SERIALIZATION {
        let extensions = ar.u8()?;
        if extensions & EXT_OVERRIDABLE_INFORMATION != 0 {
            ar.u8()?;
            ar.bool32()?;
        }
    }
    Ok(Some(PropertyTag { name, ty, size, array_index, bool_val, binary_or_native, skipped: false }))
}

// ---------------------------------------------------------------------------
// value decoding

/// Everything a value decoder needs that is not in the byte stream.
pub struct Context<'p> {
    pub package: &'p Package,
    /// Bytes of an undecodable value to keep.
    pub max_raw: usize,
    /// Beyond the Python reader: fingerprint undecodable values in full (so a
    /// change past `max_raw` still shows), decode a few more native structs, and
    /// retry math structs saved single-precision. Off to compare against it.
    pub extended: bool,
    /// Read core math types single-precision whatever the package version says.
    floats: bool,
}

impl<'p> Context<'p> {
    /// Core math types are double-precision from UE5 large world coordinates on.
    fn lwc(&self) -> bool {
        !self.floats && self.package.summary.file_version_ue5 >= UE5_LARGE_WORLD_COORDINATES
    }

    fn resolve_object(&self, index: i32) -> Val {
        Val::Str(self.package.resolve_index(index))
    }

    fn soft_object(&self, ar: &mut Reader) -> Res<Val> {
        let s = &self.package.summary;
        let paths = &self.package.soft_object_paths;
        if s.file_version_ue5 >= UE5_ADD_SOFTOBJECTPATH_LIST && !paths.is_empty() {
            let index = ar.i32()?;
            if index < 0 {
                return Ok(Val::str(""));
            }
            return match paths.get(index as usize) {
                Some(p) => Ok(Val::str(p.clone())),
                None => decode_err(format!("soft object path index {index} out of range")),
            };
        }
        Ok(Val::Str(read_soft_object_path(ar, s)?))
    }
}

fn float(ar: &mut Reader, lwc: bool) -> Res<Val> {
    Ok(Val::Float(if lwc { ar.f64()? } else { ar.f32()? as f64 }))
}

fn floats_named(ar: &mut Reader, names: &[&str], lwc: bool) -> Res<Val> {
    let mut out = Vec::new();
    for n in names {
        out.push((n.to_string(), float(ar, lwc)?));
    }
    Ok(Val::Map(out))
}

fn ints_named(ar: &mut Reader, names: &[&str]) -> Res<Val> {
    let mut out = Vec::new();
    for n in names {
        out.push((n.to_string(), Val::Int(ar.i32()? as i64)));
    }
    Ok(Val::Map(out))
}

const XY: &[&str] = &["X", "Y"];
const XYZ: &[&str] = &["X", "Y", "Z"];
const XYZW: &[&str] = &["X", "Y", "Z", "W"];
const RGBA: &[&str] = &["R", "G", "B", "A"];
const PYR: &[&str] = &["Pitch", "Yaw", "Roll"];

/// FExpressionInput::Serialize - a material graph connection.
fn expression_input(ar: &mut Reader, ctx: &Context) -> Res<Vec<(String, Val)>> {
    let mut out = vec![("Expression".to_string(), ctx.resolve_object(ar.package_index()?))];
    out.push(("OutputIndex".into(), Val::Int(ar.i32()? as i64)));
    out.push(("InputName".into(), Val::Str(ar.name()?)));
    for key in ["Mask", "MaskR", "MaskG", "MaskB", "MaskA"] {
        out.push((key.into(), Val::Int(ar.i32()? as i64)));
    }
    Ok(out)
}

/// FMaterialInput<T>: an expression input plus an inline constant fallback.
fn material_input(ar: &mut Reader, ctx: &Context, constant: fn(&mut Reader) -> Res<Val>) -> Res<Val> {
    let mut value = expression_input(ar, ctx)?;
    value.push(("UseConstant".into(), Val::Bool(ar.bool32()?)));
    value.push(("Constant".into(), constant(ar)?));
    Ok(Val::Map(value))
}

/// Structs that serialize natively (opaque to the tag walk), including the one
/// this reader has no decoder for.
fn is_native_struct(name: &str) -> bool {
    name == "UniqueNetIdRepl" || NATIVE_DECODED.contains(&name)
}

const NATIVE_DECODED: &[&str] = &[
    "Vector", "Vector3d", "Vector3f", "Vector2D", "Vector2d", "Vector2f", "Vector4", "Vector4d", "Vector4f",
    "Quat", "Quat4d", "Quat4f", "Plane", "Rotator", "Rotator3d", "Rotator3f", "LinearColor", "Color",
    "IntPoint", "IntVector", "IntVector2", "IntVector4", "UintVector2", "IntRect", "Guid", "DateTime",
    "Timespan", "FrameNumber", "FrameRate", "Box", "Box2D", "Sphere", "Matrix", "Matrix44f",
    "TopLevelAssetPath", "SoftObjectPath", "SoftClassPath", "ExpressionInput", "MaterialAttributesInput",
    "ScalarMaterialInput", "ShadingModelMaterialInput", "SubstrateMaterialInput", "StrataMaterialInput",
    "VectorMaterialInput", "Vector2MaterialInput", "ColorMaterialInput",
];

/// The decoder of a native struct, or None when there is none.
fn read_native_struct(name: &str, ar: &mut Reader, ctx: &Context) -> Option<Res<Val>> {
    let lwc = ctx.lwc();
    let f32s = |ar: &mut Reader, names: &[&str]| floats_named(ar, names, false);
    let r = match name {
        "Vector" | "Vector3d" => floats_named(ar, XYZ, lwc),
        "Vector3f" => f32s(ar, XYZ),
        "Vector2D" | "Vector2d" => floats_named(ar, XY, lwc),
        "Vector2f" => f32s(ar, XY),
        "Vector4" | "Vector4d" | "Quat" | "Quat4d" | "Plane" => floats_named(ar, XYZW, lwc),
        "Vector4f" | "Quat4f" => f32s(ar, XYZW),
        "Rotator" | "Rotator3d" => floats_named(ar, PYR, lwc),
        "Rotator3f" => f32s(ar, PYR),
        "LinearColor" => f32s(ar, RGBA),
        "Color" => (|| {
            let (b, g, r, a) = (ar.u8()?, ar.u8()?, ar.u8()?, ar.u8()?);
            Ok(Val::map(vec![
                ("B", Val::Int(b as i64)),
                ("G", Val::Int(g as i64)),
                ("R", Val::Int(r as i64)),
                ("A", Val::Int(a as i64)),
            ]))
        })(),
        "IntPoint" | "IntVector2" => ints_named(ar, XY),
        "IntVector" => ints_named(ar, XYZ),
        "IntVector4" => ints_named(ar, XYZW),
        "UintVector2" => (|| Ok(Val::map(vec![("X", Val::Int(ar.u32()? as i64)), ("Y", Val::Int(ar.u32()? as i64))])))(),
        "IntRect" => (|| Ok(Val::map(vec![("Min", ints_named(ar, XY)?), ("Max", ints_named(ar, XY)?)])))(),
        "Guid" => ar.guid().map(Val::Str),
        "DateTime" | "Timespan" => ar.i64().map(Val::Int),
        "FrameNumber" => ar.i32().map(|v| Val::Int(v as i64)),
        "FrameRate" => ints_named(ar, &["Numerator", "Denominator"]),
        "Box" | "Box2D" => (|| {
            let axes = if name == "Box" { XYZ } else { XY };
            let min = floats_named(ar, axes, lwc)?;
            let max = floats_named(ar, axes, lwc)?;
            Ok(Val::map(vec![("Min", min), ("Max", max), ("IsValid", Val::Bool(ar.u8()? != 0))]))
        })(),
        "Sphere" => (|| Ok(Val::map(vec![("Center", floats_named(ar, XYZ, lwc)?), ("W", float(ar, lwc)?)])))(),
        "Matrix" | "Matrix44f" => (|| {
            let wide = lwc && name == "Matrix";
            let mut rows = Vec::new();
            for _ in 0..4 {
                let mut row = Vec::new();
                for _ in 0..4 {
                    row.push(float(ar, wide)?);
                }
                rows.push(Val::List(row));
            }
            Ok(Val::List(rows))
        })(),
        "TopLevelAssetPath" => (|| Ok(Val::Str(format!("{}.{}", ar.name()?, ar.name()?))))(),
        "SoftObjectPath" | "SoftClassPath" => ctx.soft_object(ar),
        "ExpressionInput" | "MaterialAttributesInput" => expression_input(ar, ctx).map(Val::Map),
        "ScalarMaterialInput" => material_input(ar, ctx, |ar| Ok(Val::Float(ar.f32()? as f64))),
        "ShadingModelMaterialInput" | "SubstrateMaterialInput" | "StrataMaterialInput" => {
            material_input(ar, ctx, |ar| Ok(Val::Int(ar.u32()? as i64)))
        }
        "VectorMaterialInput" => material_input(ar, ctx, |ar| floats_named(ar, XYZ, false)),
        "Vector2MaterialInput" => material_input(ar, ctx, |ar| floats_named(ar, XY, false)),
        "ColorMaterialInput" => material_input(ar, ctx, |ar| floats_named(ar, RGBA, false)),
        "DeprecateSlateVector2D" if ctx.extended => f32s(ar, XY),
        // FRichCurveKey::Serialize: three enum bytes, then six floats.
        "RichCurveKey" if ctx.extended => (|| {
            let pick = |names: &[&str], v: u8| Val::Str(names.get(v as usize).map(|s| s.to_string()).unwrap_or(v.to_string()));
            let interp = pick(&["RCIM_Linear", "RCIM_Constant", "RCIM_Cubic", "RCIM_None"], ar.u8()?);
            let tangent = pick(&["RCTM_Auto", "RCTM_User", "RCTM_Break", "RCTM_None", "RCTM_SmartAuto"], ar.u8()?);
            let weight =
                pick(&["RCTWM_WeightedNone", "RCTWM_WeightedArrive", "RCTWM_WeightedLeave", "RCTWM_WeightedBoth"], ar.u8()?);
            let mut m = vec![("InterpMode", interp), ("TangentMode", tangent), ("TangentWeightMode", weight)];
            for key in ["Time", "Value", "ArriveTangent", "ArriveTangentWeight", "LeaveTangent", "LeaveTangentWeight"] {
                m.push((key, Val::Float(ar.f32()? as f64)));
            }
            Ok(Val::map(m))
        })(),
        "SimpleCurveKey" if ctx.extended => f32s(ar, &["Time", "Value"]),
        "EdGraphPinType" if ctx.extended => super::graph::read_pin_type(ar, ctx.package),
        // FGameplayTagContainer::Serialize: the tags as a plain FName array.
        "GameplayTagContainer" if ctx.extended => (|| {
            let count = bounded(ar, 1 << 16, "gameplay tag")?;
            let mut tags = Vec::new();
            for _ in 0..count {
                tags.push(Val::Str(ar.name()?));
            }
            Ok(Val::List(tags))
        })(),
        _ => return None,
    };
    Some(r)
}

const OBJECT_READERS: &[&str] = &[
    "ObjectProperty", "ClassProperty", "WeakObjectProperty", "InterfaceProperty", "ObjectPtrProperty",
    "ClassPtrProperty",
];

fn bounded(ar: &mut Reader, max: i32, what: &str) -> Res<usize> {
    let count = ar.i32()?;
    if count < 0 || count > max {
        return decode_err(format!("implausible {what} count {count}"));
    }
    Ok(count as usize)
}

/// Decodes one value of the given type. Fails with `Decode` when unsupported.
///
/// `tag` is the FPropertyTag this value belongs to, when the value sits directly
/// under one. Its 'binary or native serialize' bit says whether a struct is
/// opaque or a nested tagged property list, which no lookup table can tell us
/// reliably.
fn read_value(ar: &mut Reader, ty: &TypeName, ctx: &Context, depth: u32, tag: Option<&PropertyTag>) -> Res<Val> {
    if depth > 24 {
        return decode_err("nesting too deep".into());
    }
    let kind = ty.name.as_str();
    match kind {
        "Int8Property" => return Ok(Val::Int(ar.i8()? as i64)),
        "Int16Property" => return Ok(Val::Int(ar.i16()? as i64)),
        "IntProperty" => return Ok(Val::Int(ar.i32()? as i64)),
        "Int64Property" => return Ok(Val::Int(ar.i64()?)),
        "UInt16Property" => return Ok(Val::Int(ar.u16()? as i64)),
        "UInt32Property" => return Ok(Val::Int(ar.u32()? as i64)),
        "UInt64Property" => return Ok(Val::UInt(ar.u64()?)),
        "FloatProperty" => return Ok(Val::Float(ar.f32()? as f64)),
        "DoubleProperty" => return Ok(Val::Float(ar.f64()?)),
        "NameProperty" => return Ok(Val::Str(ar.name()?)),
        "StrProperty" | "Utf8StrProperty" | "AnsiStrProperty" | "VerseStringProperty" => {
            return Ok(Val::Str(ar.fstring()?))
        }
        // Inside a container a bool occupies one byte; standalone bools live in the tag.
        "BoolProperty" => return Ok(Val::Bool(ar.u8()? != 0)),
        "ByteProperty" => {
            return match ty.param_name(0) {
                Some(e) if e != "None" => Ok(Val::Str(ar.name()?)),
                _ => Ok(Val::Int(ar.u8()? as i64)),
            }
        }
        "EnumProperty" => return Ok(Val::Str(ar.name()?)),
        "LazyObjectProperty" => return Ok(Val::Str(ar.guid()?)),
        "SoftObjectProperty" | "SoftClassProperty" => return ctx.soft_object(ar),
        "TextProperty" => return read_text(ar, ctx.package),
        "StructProperty" => return read_struct(ar, ty, ctx, depth, tag.map(|t| t.binary_or_native)),
        _ if OBJECT_READERS.contains(&kind) => return Ok(ctx.resolve_object(ar.package_index()?)),
        _ => {}
    }
    match kind {
        "ArrayProperty" => {
            let Some(inner) = ty.param(0) else { return decode_err("array without inner type".into()) };
            let count = bounded(ar, 1 << 22, "array")?;
            let mut out = Vec::with_capacity(count.min(4096));
            for _ in 0..count {
                out.push(read_value(ar, inner, ctx, depth + 1, None)?);
            }
            Ok(Val::List(out))
        }
        "SetProperty" => {
            let Some(inner) = ty.param(0) else { return decode_err("set without inner type".into()) };
            let removed = ar.i32()?;
            for _ in 0..removed.max(0) {
                read_value(ar, inner, ctx, depth + 1, None)?;
            }
            let count = bounded(ar, 1 << 22, "set")?;
            let mut out = Vec::with_capacity(count.min(4096));
            for _ in 0..count {
                out.push(read_value(ar, inner, ctx, depth + 1, None)?);
            }
            Ok(Val::List(out))
        }
        "MapProperty" => {
            let (Some(key_type), Some(value_type)) = (ty.param(0), ty.param(1)) else {
                return decode_err("map without key/value type".into());
            };
            let removed = ar.i32()?;
            for _ in 0..removed.max(0) {
                read_value(ar, key_type, ctx, depth + 1, None)?;
            }
            let count = bounded(ar, 1 << 22, "map")?;
            let mut out = Vec::with_capacity(count.min(4096));
            for _ in 0..count {
                let key = read_value(ar, key_type, ctx, depth + 1, None)?;
                let value = read_value(ar, value_type, ctx, depth + 1, None)?;
                out.push(Val::map(vec![("key", key), ("value", value)]));
            }
            Ok(Val::List(out))
        }
        "OptionalProperty" => {
            let Some(inner) = ty.param(0) else { return decode_err("optional without inner type".into()) };
            if ar.u8()? != 0 {
                read_value(ar, inner, ctx, depth + 1, None)
            } else {
                Ok(Val::Null)
            }
        }
        "DelegateProperty" => {
            let object = ctx.resolve_object(ar.package_index()?);
            Ok(Val::map(vec![("object", object), ("function", Val::Str(ar.name()?))]))
        }
        "MulticastDelegateProperty" | "MulticastInlineDelegateProperty" | "MulticastSparseDelegateProperty" => {
            let count = bounded(ar, 1 << 16, "delegate")?;
            let mut out = Vec::new();
            for _ in 0..count {
                let object = ctx.resolve_object(ar.package_index()?);
                out.push(Val::map(vec![("object", object), ("function", Val::Str(ar.name()?))]));
            }
            Ok(Val::List(out))
        }
        "FieldPathProperty" => {
            let count = bounded(ar, 1 << 12, "field path")?;
            let mut path = Vec::new();
            for _ in 0..count {
                path.push(Val::Str(ar.name()?));
            }
            let owner = ctx.resolve_object(ar.package_index()?);
            Ok(Val::map(vec![("path", Val::List(path)), ("owner", owner)]))
        }
        _ => decode_err(format!("unsupported property type {kind}")),
    }
}

/// A struct is either native-serialized (opaque) or a nested tagged property list.
///
/// `native` comes from the property tag when available; None means 'unknown',
/// in which case the known-native table decides.
fn read_struct(ar: &mut Reader, ty: &TypeName, ctx: &Context, depth: u32, native: Option<bool>) -> Res<Val> {
    let struct_name = ty.param_name(0).unwrap_or("<unnamed>");
    if native == Some(false) {
        return Ok(read_tagged_properties(ar, ctx, depth + 1, None));
    }
    if let Some(r) = read_native_struct(struct_name, ar, ctx) {
        return r;
    }
    if native == Some(true) {
        return decode_err(format!("no decoder for native struct {struct_name}"));
    }
    Ok(read_tagged_properties(ar, ctx, depth + 1, None))
}

/// Reads tags until the terminating 'None' tag (or `end`).
fn read_tagged_properties(ar: &mut Reader, ctx: &Context, depth: u32, end: Option<usize>) -> Val {
    let mut out: Vec<(String, Val)> = Vec::new();
    loop {
        if end.is_some_and(|e| ar.tell() >= e) {
            break;
        }
        let tag = match read_tag(ar, &ctx.package.summary) {
            Ok(Some(t)) => t,
            Ok(None) => break,
            Err(error) => {
                // Only reachable when the property block's extent is unknown (pre-5.3
                // packages) or an undecodable value desynced the stream.
                out.push(("__error".into(), Val::Str(format!("stopped at offset {}: {error}", ar.tell()))));
                break;
            }
        };
        let key = if tag.array_index == 0 { tag.name.clone() } else { format!("{}[{}]", tag.name, tag.array_index) };
        match read_tagged_value(ar, &tag, ctx, depth) {
            Ok(v) => set(&mut out, key, v),
            Err(error) => {
                out.push(("__error".into(), Val::Str(format!("stopped at {} (offset {}): {error}", tag.name, ar.tell()))));
                break;
            }
        }
    }
    Val::Map(out)
}

/// Insert or overwrite, as a Python dict assignment does.
fn set(out: &mut Vec<(String, Val)>, key: String, value: Val) {
    match out.iter_mut().find(|(k, _)| *k == key) {
        Some(slot) => slot.1 = value,
        None => out.push((key, value)),
    }
}

/// Decodes one tagged value, falling back to raw bytes and resyncing on Size.
fn read_tagged_value(ar: &mut Reader, tag: &PropertyTag, ctx: &Context, depth: u32) -> Res<Val> {
    let start = ar.tell();
    if tag.size < 0 || start + tag.size as usize > ar.data.len() {
        // A size that cannot fit the buffer means the stream desynced before this tag.
        return archive_err(format!("tag {} has out-of-range size {} at offset {start}", tag.name, tag.size));
    }
    let size = tag.size as usize;
    if tag.skipped {
        ar.seek((start + size) as i64)?;
        return Ok(Val::map(vec![("__skipped", Val::Bool(true))]));
    }
    if tag.ty.name == "BoolProperty" && size == 0 {
        return Ok(Val::Bool(tag.bool_val));
    }
    let decoded = read_value(ar, &tag.ty, ctx, depth, Some(tag)).and_then(|v| {
        let consumed = ar.tell() - start;
        if size != 0 && consumed != size {
            return decode_err(format!("read {consumed} bytes, tag says {size}"));
        }
        Ok(v)
    });
    let decoded = match decoded {
        Err(_) if ctx.extended && !ctx.floats && is_lwc_struct(&tag.ty) => {
            // A math struct saved single-precision under a double-precision tag type.
            ar.seek(start as i64)?;
            let floats = Context { package: ctx.package, max_raw: ctx.max_raw, extended: true, floats: true };
            read_value(ar, &tag.ty, &floats, depth, Some(tag)).and_then(|v| {
                let consumed = ar.tell() - start;
                if consumed != size {
                    return decode_err(format!("read {consumed} bytes, tag says {size}"));
                }
                Ok(v)
            })
        }
        other => other,
    };
    match decoded {
        Ok(v) => Ok(v),
        Err(error) => {
            let all = &ar.data[start..start + size];
            ar.seek((start + size) as i64)?;
            let raw = &all[..size.min(ctx.max_raw)];
            let mut out = vec![
                ("__type", Val::Str(tag.ty.to_string())),
                ("__size", Val::Int(size as i64)),
                ("__reason", Val::Str(error.to_string())),
                ("__raw", Val::Str(hex(raw))),
            ];
            if ctx.extended && size > raw.len() {
                out.push(("__sha1", Val::Str(sha1_smol::Sha1::from(all).digest().to_string())));
            }
            Ok(Val::map(out))
        }
    }
}

fn is_lwc_struct(ty: &TypeName) -> bool {
    ty.name == "StructProperty"
        && matches!(
            ty.param_name(0),
            Some("Vector" | "Vector2D" | "Vector4" | "Quat" | "Plane" | "Rotator" | "Box" | "Box2D" | "Sphere" | "Matrix")
        )
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// Custom version of Dev-Editor, needed to know whether an empty FText stores a
// culture-invariant string (Engine/Source/Runtime/Core/Private/UObject/DevObjectVersion.cpp).
const EDITOR_OBJECT_VERSION_GUID: &str = "E4B068EDF49442E9A231DA0B2E46BB41";
const EDITOR_CULTURE_INVARIANT_TEXT_KEY_STABILITY: i32 = 32;

/// EFormatArgumentType, serialized by index
fn format_argument(ar: &mut Reader, package: &Package) -> Res<Val> {
    let argument_type = ar.i8()?;
    match argument_type {
        0 => Ok(Val::Int(ar.i64()?)),
        1 | 5 => Ok(Val::UInt(ar.u64()?)), // UInt, Gender stored as UInt
        2 => Ok(Val::Float(ar.f32()? as f64)),
        3 => Ok(Val::Float(ar.f64()?)),
        4 => read_text(ar, package),
        _ => decode_err(format!("unsupported FText format argument type {argument_type}")),
    }
}

/// FText::SerializeText - flags, history type, then a history-specific payload.
pub fn read_text(ar: &mut Reader, package: &Package) -> Res<Val> {
    ar.u32()?; // Flags
    let history_type = ar.i8()?;
    match history_type {
        -1 => {
            // ETextHistoryType::None
            if package.summary.custom_version(EDITOR_OBJECT_VERSION_GUID) >= EDITOR_CULTURE_INVARIANT_TEXT_KEY_STABILITY
                && ar.bool32()?
            {
                return Ok(Val::Str(ar.fstring()?));
            }
            Ok(Val::str(""))
        }
        0 => {
            // Base
            let namespace = ar.fstring()?;
            let key = ar.fstring()?;
            let source = ar.fstring()?;
            if !source.is_empty() || (namespace.is_empty() && key.is_empty()) {
                return Ok(Val::Str(source));
            }
            Ok(Val::map(vec![("namespace", Val::Str(namespace)), ("key", Val::Str(key))]))
        }
        1 | 2 => {
            // NamedFormat, OrderedFormat
            let format_text = read_text(ar, package)?;
            let count = bounded(ar, 1 << 16, "FText argument")?;
            let arguments = if history_type == 1 {
                let mut named = Vec::new();
                for _ in 0..count {
                    let name = ar.fstring()?;
                    let value = format_argument(ar, package)?;
                    set(&mut named, name, value);
                }
                Val::Map(named)
            } else {
                let mut ordered = Vec::new();
                for _ in 0..count {
                    ordered.push(format_argument(ar, package)?);
                }
                Val::List(ordered)
            };
            Ok(Val::map(vec![("format", format_text), ("arguments", arguments)]))
        }
        _ => decode_err(format!("unsupported FText history type {history_type}")),
    }
}

/// Reads the tagged property block of one export.
pub fn read_export_properties(package: &Package, export: &ObjectExport, max_raw: usize, extended: bool) -> Val {
    let data = package.export_data(export);
    let mut ar = Reader::new(data, &package.names);
    let ctx = Context { package, max_raw, extended, floats: false };
    let start = export.script_start.max(0) as usize;
    let end = if export.script_end > 0 { Some(export.script_end as usize) } else { None };
    if start >= data.len() {
        return Val::Map(Vec::new());
    }
    if package.summary.file_version_ue5 >= UE5_SCRIPT_SERIALIZATION_OFFSET && export.script_end == 0 {
        // The export map records an empty script range: this object serializes only
        // native data (e.g. FLandscapeHeightmapTextureEdgeFixup), no tagged properties.
        return Val::Map(Vec::new());
    }
    let head = (|| -> Res<()> {
        ar.seek(start as i64)?;
        // UStruct::SerializeVersionedTaggedProperties writes a control byte at the object
        // root (not for nested structs) from PROPERTY_TAG_EXTENSION_AND_OVERRIDABLE_SERIALIZATION on.
        if package.summary.file_version_ue5 >= UE5_PROPERTY_TAG_EXTENSION_AND_OVERRIDABLE_SERIALIZATION {
            let control = ar.u8()?;
            if control & 0x02 != 0 {
                ar.u8()?; // EOverriddenPropertyOperation
            }
        }
        Ok(())
    })();
    if let Err(Error::Archive(e) | Error::Decode(e)) = head {
        return Val::map(vec![("__error", Val::Str(e))]);
    }
    read_tagged_properties(&mut ar, &ctx, 0, end)
}
