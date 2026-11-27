//! Sequencer data: the native structs a MovieScene's sections save (curve
//! channels, frame ranges), and the animations of a Widget Blueprint or a
//! sequence read back out of a package as tracks, sections and keys.
//!
//! Layouts follow TMovieSceneCurveChannelImpl::Serialize
//! (Engine/Source/Runtime/MovieScene/Private/Channels/MovieSceneCurveChannelImpl.cpp)
//! and FMovieSceneFrameRange / TRange<FFrameNumber> serialization.

use std::collections::HashMap;

use serde::Serialize;

use super::archive::{decode_err, Reader, Res, Val};
use super::diff::LABEL;
use super::doc::{leaf, properties};
use super::package::Package;

const FORTNITE_MAIN_OBJECT_VERSION: &str = "601D1886AC644F84AA16D3DE0DEAC7D6";
const FORTNITE_MAIN_SERIALIZE_FLOAT_CHANNEL_SHOW_CURVE: i32 = 53;

/// UMovieScene's TickResolution when none is saved (GetLegacyConversionFrameRate).
const DEFAULT_TICK_RESOLUTION: f64 = 60000.0;

const INTERP: &[&str] = &["Linear", "Constant", "Cubic", "None"];
const TANGENT: &[&str] = &["Auto", "User", "Break", "None", "SmartAuto"];
const EXTRAPOLATION: &[&str] = &["Cycle", "CycleWithOffset", "Oscillate", "Linear", "Constant", "None"];
/// ERichCurveExtrapolation::RCCE_Constant, a channel's default.
const EXTRAP_CONSTANT: u8 = 4;

fn pick(names: &[&str], v: u8) -> String {
    names.get(v as usize).map(|s| s.to_string()).unwrap_or(v.to_string())
}

/// "0.25", "1.2", "0": seconds as a person reads them.
pub fn seconds(t: f64) -> String {
    let r = (t * 1000.0).round() / 1000.0;
    format!("{}", if r == 0.0 { 0.0 } else { r })
}

fn number(v: f64) -> String {
    super::diff::scalar(&Val::Float(v))
}

/// FMovieSceneFloatChannel / FMovieSceneDoubleChannel, saved whole: extrapolation,
/// the key times and values as raw arrays, the default, and the channel's own
/// tick resolution (which turns frame numbers into seconds).
pub fn read_channel(ar: &mut Reader, package: &Package, double: bool) -> Res<Val> {
    let pre = ar.u8()?;
    let post = ar.u8()?;
    let time_size = ar.i32()?;
    let count = ar.i32()?;
    if time_size != 4 || !(0..=1 << 20).contains(&count) {
        return decode_err(format!("unexpected channel time array ({time_size} x {count})"));
    }
    let mut frames = Vec::with_capacity(count as usize);
    for _ in 0..count {
        frames.push(ar.i32()?);
    }
    let value_size = ar.i32()?;
    let value_count = ar.i32()?;
    let value_bytes = if double { 8 } else { 4 };
    // Value, then FMovieSceneTangentData (four floats and a weight mode), then the interp and tangent modes.
    let tangent_at = value_bytes as usize;
    let interp_at = tangent_at + 20;
    if value_count != count || (value_size as usize) < interp_at + 2 {
        return decode_err(format!("unexpected channel value array ({value_size} x {value_count})"));
    }
    let mut raw = Vec::with_capacity(count as usize);
    for _ in 0..count {
        raw.push(ar.bytes(value_size as usize)?);
    }
    let default = if double { ar.f64()? } else { ar.f32()? as f64 };
    let has_default = ar.bool32()?;
    let numerator = ar.i32()?;
    let denominator = ar.i32()?;
    if package.summary.custom_version(FORTNITE_MAIN_OBJECT_VERSION) >= FORTNITE_MAIN_SERIALIZE_FLOAT_CHANNEL_SHOW_CURVE {
        ar.bool32()?; // bShowCurve
    }
    let rate = if numerator > 0 && denominator > 0 { numerator as f64 / denominator as f64 } else { DEFAULT_TICK_RESOLUTION };
    let f32_at = |b: &[u8], at: usize| f32::from_le_bytes(b[at..at + 4].try_into().unwrap()) as f64;
    let mut keys = Vec::with_capacity(raw.len());
    for (frame, b) in frames.iter().zip(&raw) {
        let value = if double { f64::from_le_bytes(b[..8].try_into().unwrap()) } else { f32_at(b, 0) };
        let time = *frame as f64 / rate;
        let interp = pick(INTERP, b[interp_at]);
        let mut k = vec![
            (LABEL, Val::Str(format!("{} s = {}", seconds(time), number(value)))),
            ("Time", Val::Float(time)),
            ("Value", Val::Float(value)),
            ("Interp", Val::Str(interp.clone())),
        ];
        if interp == "Cubic" {
            k.push(("Tangent mode", Val::Str(pick(TANGENT, b[interp_at + 1]))));
            k.push(("Arrive tangent", Val::Float(f32_at(b, tangent_at))));
            k.push(("Leave tangent", Val::Float(f32_at(b, tangent_at + 4))));
        }
        keys.push(Val::map(k));
    }
    let mut m = vec![
        (LABEL, Val::Str(format!("{} key{}", keys.len(), if keys.len() == 1 { "" } else { "s" }))),
        ("Keys", Val::List(keys)),
    ];
    if has_default {
        m.push(("Default", Val::Float(default)));
    }
    if pre != EXTRAP_CONSTANT {
        m.push(("Pre-infinity", Val::Str(pick(EXTRAPOLATION, pre))));
    }
    if post != EXTRAP_CONSTANT {
        m.push(("Post-infinity", Val::Str(pick(EXTRAPOLATION, post))));
    }
    Ok(Val::map(m))
}

/// FMovieSceneFrameRange: two TRangeBound<FFrameNumber>, each a bound type
/// (Exclusive, Inclusive, Open) and a frame number.
pub fn read_frame_range(ar: &mut Reader) -> Res<Val> {
    let bound = |ar: &mut Reader| -> Res<Val> {
        let kind = ar.u8()?;
        let frame = ar.i32()?;
        Ok(match kind {
            0 => Val::map(vec![("Frame", Val::Int(frame as i64)), ("Inclusive", Val::Bool(false))]),
            1 => Val::map(vec![("Frame", Val::Int(frame as i64)), ("Inclusive", Val::Bool(true))]),
            _ => Val::str("open"),
        })
    };
    let lower = bound(ar)?;
    let upper = bound(ar)?;
    Ok(Val::map(vec![("Lower", lower), ("Upper", upper)]))
}

fn is_channel(v: &Val) -> bool {
    matches!(v, Val::Map(m) if m.first().is_some_and(|(k, _)| k == LABEL) && matches!(v.get("Keys"), Some(Val::List(_))))
}

#[derive(Serialize, Clone, PartialEq, Debug)]
pub struct Key {
    pub t: f64,
    pub v: f64,
    pub interp: String,
}

/// One animated value of a section ("DistortionAmount", "Tint.Red"; "" for the section's only one).
#[derive(Clone, Debug)]
pub struct Channel {
    pub name: String,
    pub val: Val,
    pub keys: Vec<Key>,
}

#[derive(Clone, Debug)]
pub struct Section {
    /// Seconds; None for an open bound.
    pub start: Option<f64>,
    pub end: Option<f64>,
    pub channels: Vec<Channel>,
}

#[derive(Clone, Debug)]
pub struct Track {
    /// "IMG_Bkg › Render Opacity", "MPC_UIDisplacement".
    pub label: String,
    pub class: String,
    /// What the track targets beyond its label (an MPC, a property path).
    pub extra: Vec<(String, Val)>,
    pub sections: Vec<Section>,
}

#[derive(Clone, Debug)]
pub struct Anim {
    pub name: String,
    pub start: Option<f64>,
    pub end: Option<f64>,
    pub tracks: Vec<Track>,
}

fn map(v: Val) -> Vec<(String, Val)> {
    match v {
        Val::Map(m) => m,
        _ => Vec::new(),
    }
}

fn unique(names: &mut Vec<String>, name: String) -> String {
    let mut k = name.clone();
    let mut n = 2;
    while names.contains(&k) {
        k = format!("{name} #{n}");
        n += 1;
    }
    names.push(k.clone());
    k
}

/// A saved FFrameRate as frames per second; a tagged one saves only the fields
/// that differ from 60000/1.
fn frame_rate(v: Option<&Val>) -> f64 {
    let field = |k: &str, d: i64| match v.and_then(|v| v.get(k)) {
        Some(Val::Int(i)) if *i > 0 => *i,
        _ => d,
    };
    field("Numerator", DEFAULT_TICK_RESOLUTION as i64) as f64 / field("Denominator", 1) as f64
}

fn bound_seconds(b: Option<&Val>, rate: f64) -> Option<f64> {
    match b?.get("Frame") {
        Some(Val::Int(f)) => Some(*f as f64 / rate),
        _ => None,
    }
}

/// Every channel under a section's properties, named by where it sits.
fn channels(v: &Val, name: &str, out: &mut Vec<Channel>) {
    if is_channel(v) {
        let keys = match v.get("Keys") {
            Some(Val::List(l)) => l
                .iter()
                .map(|k| Key {
                    t: match k.get("Time") {
                        Some(Val::Float(t)) => *t,
                        _ => 0.0,
                    },
                    v: match k.get("Value") {
                        Some(Val::Float(v)) => *v,
                        _ => 0.0,
                    },
                    interp: k.get("Interp").and_then(|i| i.as_str()).unwrap_or("").to_string(),
                })
                .collect(),
            _ => Vec::new(),
        };
        out.push(Channel { name: name.to_string(), val: v.clone(), keys });
        return;
    }
    let join = |field: &str| {
        // "FloatCurve" -> "", "RedCurve" -> "Red", "ParameterCurve" (beside a ParameterName) -> ""
        let field = field.strip_suffix("Curve").unwrap_or(field);
        let field = if matches!(field, "Float" | "Double" | "Parameter") { "" } else { field };
        match (name.is_empty(), field.is_empty()) {
            (_, true) => name.to_string(),
            (true, false) => field.to_string(),
            (false, false) => format!("{name}.{field}"),
        }
    };
    match v {
        Val::Map(m) => {
            for (k, child) in m {
                channels(child, &join(k), out);
            }
        }
        Val::List(items) => {
            for (i, item) in items.iter().enumerate() {
                let item_name = match item.get("ParameterName").and_then(|n| n.as_str()) {
                    Some(p) if name.ends_with("NamesAndCurves") || name.is_empty() => p.to_string(),
                    Some(p) => format!("{name}.{p}"),
                    None => format!("{name}[{i}]"),
                };
                channels(item, &item_name, out);
            }
        }
        _ => {}
    }
}

fn section(pkg: &Package, index: usize, rate: f64) -> Section {
    let props = properties(pkg, index);
    let range = props.get("SectionRange");
    let start = bound_seconds(range.and_then(|r| r.get("Lower")), rate);
    let end = bound_seconds(range.and_then(|r| r.get("Upper")), rate);
    let mut found = Vec::new();
    channels(&props, "", &mut found);
    Section { start, end, channels: found }
}

fn track(pkg: &Package, index: usize, binding: Option<&str>, rate: f64, by_path: &HashMap<String, usize>) -> Track {
    let class = leaf(&pkg.export_class(&pkg.exports[index])).to_string();
    let props = map(properties(pkg, index));
    let get = |k: &str| props.iter().find(|(n, _)| n == k).map(|(_, v)| v);
    let what = get("DisplayName")
        .and_then(|v| v.as_str())
        .map(String::from)
        .or_else(|| get("PropertyBinding").and_then(|b| b.get("PropertyName")).and_then(|n| n.as_str()).map(String::from))
        .unwrap_or_else(|| class.trim_start_matches("MovieScene").trim_end_matches("Track").to_string());
    let label = match binding {
        Some(b) => format!("{b} › {what}"),
        None => what,
    };
    let mut extra = Vec::new();
    for k in ["MPC", "ParameterCollection"] {
        if let Some(v) = get(k) {
            extra.push((k.to_string(), v.clone()));
        }
    }
    if let Some(path) = get("PropertyBinding").and_then(|b| b.get("PropertyPath")) {
        if path.as_str() != get("PropertyBinding").and_then(|b| b.get("PropertyName")).and_then(|n| n.as_str()) {
            extra.push(("Property path".into(), path.clone()));
        }
    }
    let sections = match get("Sections") {
        Some(Val::List(l)) => l
            .iter()
            .filter_map(|s| s.as_str().and_then(|p| by_path.get(p)))
            .map(|&i| section(pkg, i, rate))
            .collect(),
        _ => Vec::new(),
    };
    Track { label, class, extra, sections }
}

fn movie_scene(pkg: &Package, name: String, index: usize, by_path: &HashMap<String, usize>) -> Anim {
    let props = map(properties(pkg, index));
    let get = |k: &str| props.iter().find(|(n, _)| n == k).map(|(_, v)| v);
    let rate = frame_rate(get("TickResolution"));
    let range = get("PlaybackRange");
    let mut tracks = Vec::new();
    let add = |paths: Option<&Val>, binding: Option<&str>, tracks: &mut Vec<Track>| {
        if let Some(Val::List(l)) = paths {
            for p in l.iter().filter_map(|p| p.as_str()) {
                if let Some(&i) = by_path.get(p) {
                    tracks.push(track(pkg, i, binding, rate, by_path));
                }
            }
        }
    };
    // Tracks bound to nothing (an MPC, a camera cut), then each binding's.
    for k in ["Tracks", "MasterTracks", "CameraCutTrack"] {
        match get(k) {
            Some(v @ Val::List(_)) => add(Some(v), None, &mut tracks),
            Some(Val::Str(p)) => add(Some(&Val::List(vec![Val::str(p)])), None, &mut tracks),
            _ => {}
        }
    }
    // 5.8 no longer saves FMovieSceneBinding::BindingName: the possessable or spawnable with its guid names it.
    let mut bound: HashMap<&str, &str> = HashMap::new();
    for k in ["Possessables", "Spawnables"] {
        if let Some(Val::List(l)) = get(k) {
            for p in l {
                if let (Some(g), Some(n)) = (p.get("Guid").and_then(|g| g.as_str()), p.get("Name").and_then(|n| n.as_str())) {
                    bound.insert(g, n);
                }
            }
        }
    }
    if let Some(Val::List(bindings)) = get("ObjectBindings") {
        for b in bindings {
            let who = b
                .get("BindingName")
                .and_then(|n| n.as_str())
                .or_else(|| b.get("ObjectGuid").and_then(|g| g.as_str()).and_then(|g| bound.get(g).copied()))
                .unwrap_or("?");
            add(b.get("Tracks"), Some(who), &mut tracks);
        }
    }
    let mut labels = Vec::new();
    for t in &mut tracks {
        t.label = unique(&mut labels, t.label.clone());
    }
    Anim {
        name,
        start: bound_seconds(range.and_then(|r| r.get("Lower")), rate),
        end: bound_seconds(range.and_then(|r| r.get("Upper")), rate),
        tracks,
    }
}

/// The package's animations: a Widget Blueprint's (not the generated class's
/// copies), or a sequence asset's own MovieScene.
pub fn read(pkg: &Package) -> Vec<Anim> {
    let Some(main) = pkg.exports.iter().position(|e| e.is_asset) else { return Vec::new() };
    let by_path: HashMap<String, usize> = (0..pkg.exports.len()).map(|i| (pkg.export_path(i), i)).collect();
    let mut owners: Vec<(String, usize)> = Vec::new();
    for (i, e) in pkg.exports.iter().enumerate() {
        let is_anim = leaf(&pkg.export_class(e)) == "WidgetAnimation" && e.outer_index == main as i32 + 1;
        if is_anim || i == main {
            let props = properties(pkg, i);
            if let Some(ms) = props.get("MovieScene").and_then(|p| p.as_str()).and_then(|p| by_path.get(p)) {
                let name = props.get("DisplayLabel").and_then(|n| n.as_str()).unwrap_or(&e.object_name).to_string();
                owners.push((name, *ms));
            }
        }
    }
    let mut names = Vec::new();
    owners.into_iter().map(|(name, ms)| movie_scene(pkg, unique(&mut names, name), ms, &by_path)).collect()
}

fn range_text(start: Option<f64>, end: Option<f64>) -> String {
    match (start, end) {
        (None, None) => "infinite".into(),
        (s, e) => format!(
            "{} – {} s",
            s.map(seconds).unwrap_or("−∞".into()),
            e.map(seconds).unwrap_or("∞".into())
        ),
    }
}

/// The animations as the summary shows them: track by track, keys in seconds.
pub fn summary(anims: &[Anim]) -> Val {
    let mut out = Vec::new();
    for a in anims {
        let mut m = vec![("Length".to_string(), Val::Str(range_text(a.start, a.end)))];
        for t in &a.tracks {
            let kind = t.class.trim_start_matches("MovieScene").trim_end_matches("Track");
            let mut tm: Vec<(String, Val)> = vec![(LABEL.into(), Val::str(kind))];
            tm.extend(t.extra.iter().cloned());
            let one = t.sections.len() == 1;
            for (i, s) in t.sections.iter().enumerate() {
                let mut sm: Vec<(String, Val)> = vec![("Range".into(), Val::Str(range_text(s.start, s.end)))];
                for c in &s.channels {
                    sm.push((if c.name.is_empty() { "Value".into() } else { c.name.clone() }, c.val.clone()));
                }
                if one {
                    tm.extend(sm);
                } else {
                    tm.push((format!("Section {i}"), Val::Map(sm)));
                }
            }
            m.push((t.label.clone(), Val::Map(tm)));
        }
        out.push((a.name.clone(), Val::Map(m)));
    }
    Val::Map(out)
}

#[derive(Serialize, Clone, PartialEq, Debug)]
pub struct LaneSide {
    /// [start, end] in seconds, null for an open bound.
    pub sections: Vec<(Option<f64>, Option<f64>)>,
    pub keys: Vec<Key>,
}

/// One row of the timeline: a track's channel, on either side.
#[derive(Serialize, Debug)]
pub struct Lane {
    pub track: String,
    pub channel: String,
    pub status: super::diff::Status,
    pub left: Option<LaneSide>,
    pub right: Option<LaneSide>,
}

#[derive(Serialize, Debug)]
pub struct AnimPair {
    pub name: String,
    pub status: super::diff::Status,
    pub left: Option<(Option<f64>, Option<f64>)>,
    pub right: Option<(Option<f64>, Option<f64>)>,
    pub lanes: Vec<Lane>,
}

/// A track's rows: one per channel name over all its sections, or a single
/// keyless row when nothing in it is a curve (an event or visibility track).
fn lanes(t: &Track) -> Vec<(String, LaneSide)> {
    let ranges: Vec<(Option<f64>, Option<f64>)> = t.sections.iter().map(|s| (s.start, s.end)).collect();
    let mut out: Vec<(String, LaneSide)> = Vec::new();
    for s in &t.sections {
        for c in &s.channels {
            match out.iter_mut().find(|(n, _)| *n == c.name) {
                Some((_, lane)) => lane.keys.extend(c.keys.iter().cloned()),
                None => out.push((c.name.clone(), LaneSide { sections: ranges.clone(), keys: c.keys.clone() })),
            }
        }
    }
    if out.is_empty() {
        out.push((String::new(), LaneSide { sections: ranges, keys: Vec::new() }));
    }
    out
}

fn status(l: bool, r: bool, same: bool) -> super::diff::Status {
    use super::diff::Status;
    match (l, r) {
        (true, false) => Status::Removed,
        (false, true) => Status::Added,
        _ if same => Status::Same,
        _ => Status::Changed,
    }
}

/// Both sides' animations matched by name, tracks by label, channels by name.
pub fn pair(left: &[Anim], right: &[Anim]) -> Vec<AnimPair> {
    let mut names: Vec<&str> = left.iter().map(|a| a.name.as_str()).collect();
    for a in right {
        if !names.contains(&a.name.as_str()) {
            names.push(&a.name);
        }
    }
    names
        .into_iter()
        .map(|name| {
            let l = left.iter().find(|a| a.name == name);
            let r = right.iter().find(|a| a.name == name);
            let side_lanes = |a: Option<&Anim>| -> Vec<(String, String, LaneSide)> {
                a.map(|a| a.tracks.iter().flat_map(|t| lanes(t).into_iter().map(|(c, s)| (t.label.clone(), c, s))).collect())
                    .unwrap_or_default()
            };
            let (ll, rl) = (side_lanes(l), side_lanes(r));
            let mut out: Vec<Lane> = Vec::new();
            for (track, channel, side) in &ll {
                let other = rl.iter().find(|(t, c, _)| t == track && c == channel).map(|(_, _, s)| s.clone());
                let st = status(true, other.is_some(), other.as_ref() == Some(side));
                out.push(Lane { track: track.clone(), channel: channel.clone(), status: st, left: Some(side.clone()), right: other });
            }
            for (track, channel, side) in rl {
                if out.iter().any(|x| x.track == track && x.channel == channel) {
                    continue;
                }
                // After the last row of the same track, else at the end.
                let at = out.iter().rposition(|x| x.track == track).map(|p| p + 1).unwrap_or(out.len());
                out.insert(at, Lane { track, channel, status: super::diff::Status::Added, left: None, right: Some(side) });
            }
            let range = |a: Option<&Anim>| a.map(|a| (a.start, a.end));
            let same = l.is_some() && r.is_some() && range(l) == range(r) && out.iter().all(|x| x.status == super::diff::Status::Same);
            AnimPair { name: name.to_string(), status: status(l.is_some(), r.is_some(), same), left: range(l), right: range(r), lanes: out }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seconds_drop_trailing_noise() {
        assert_eq!(seconds(0.25), "0.25");
        assert_eq!(seconds(1.2000001), "1.2");
        assert_eq!(seconds(-0.0), "0");
    }

    #[test]
    fn a_removed_track_pairs_as_a_removed_lane() {
        let key = Key { t: 0.0, v: 1.0, interp: "Cubic".into() };
        let section = |keys: Vec<Key>| Section {
            start: Some(0.0),
            end: Some(1.0),
            channels: vec![Channel { name: String::new(), val: Val::Null, keys }],
        };
        let track = |label: &str, keys: Vec<Key>| Track { label: label.into(), class: String::new(), extra: Vec::new(), sections: vec![section(keys)] };
        let anim = |tracks: Vec<Track>| Anim { name: "Fade".into(), start: Some(0.0), end: Some(1.0), tracks };
        let l = [anim(vec![track("MPC", vec![key.clone()]), track("Img › Opacity", vec![key.clone()])])];
        let r = [anim(vec![track("Img › Opacity", vec![key])])];
        let p = pair(&l, &r);
        assert_eq!(p[0].status, super::super::diff::Status::Changed);
        let st: Vec<_> = p[0].lanes.iter().map(|x| (x.track.as_str(), x.status)).collect();
        use super::super::diff::Status;
        assert_eq!(st, vec![("MPC", Status::Removed), ("Img › Opacity", Status::Same)]);
    }
}
