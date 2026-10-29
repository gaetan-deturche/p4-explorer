//! Minimal FArchive-compatible binary reader for UE package files, and the
//! ordered value tree everything decodes into.

use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};

/// Why a read failed. `Archive`: the stream itself is unusable at this point
/// (past the end, implausible count); `Decode`: a value this reader does not
/// understand, which callers fall back from to raw bytes.
#[derive(Debug, Clone)]
pub enum Error {
    Archive(String),
    Decode(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Archive(m) | Error::Decode(m) => f.write_str(m),
        }
    }
}

pub type Res<T> = Result<T, Error>;

pub fn archive_err<T>(msg: String) -> Res<T> {
    Err(Error::Archive(msg))
}

pub fn decode_err<T>(msg: String) -> Res<T> {
    Err(Error::Decode(msg))
}

/// A decoded value. Maps keep their order: properties are compared and shown
/// in the order the package wrote them.
#[derive(Debug, Clone, PartialEq)]
pub enum Val {
    Null,
    Bool(bool),
    Int(i64),
    UInt(u64),
    Float(f64),
    Str(String),
    List(Vec<Val>),
    Map(Vec<(String, Val)>),
}

impl Val {
    pub fn str(s: impl Into<String>) -> Val {
        Val::Str(s.into())
    }

    pub fn map(entries: Vec<(&str, Val)>) -> Val {
        Val::Map(entries.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }

    pub fn get(&self, key: &str) -> Option<&Val> {
        match self {
            Val::Map(m) => m.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Val::Str(s) => Some(s),
            _ => None,
        }
    }
}

impl Serialize for Val {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Val::Null => s.serialize_none(),
            Val::Bool(b) => s.serialize_bool(*b),
            Val::Int(i) => s.serialize_i64(*i),
            Val::UInt(u) => s.serialize_u64(*u),
            Val::Float(f) => s.serialize_f64(*f),
            Val::Str(t) => s.serialize_str(t),
            Val::List(items) => {
                let mut seq = s.serialize_seq(Some(items.len()))?;
                for item in items {
                    seq.serialize_element(item)?;
                }
                seq.end()
            }
            Val::Map(entries) => {
                let mut map = s.serialize_map(Some(entries.len()))?;
                for (k, v) in entries {
                    map.serialize_entry(k, v)?;
                }
                map.end()
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct FName {
    pub text: String,
    pub number: i32,
}

impl std::fmt::Display for FName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // UE stores Number as (instance + 1); 0 means "no number suffix".
        if self.number == 0 {
            f.write_str(&self.text)
        } else {
            write!(f, "{}_{}", self.text, self.number as i64 - 1)
        }
    }
}

/// Little-endian reader over a byte buffer, mirroring FArchive operators.
pub struct Reader<'a> {
    pub data: &'a [u8],
    pos: usize,
    names: &'a [String],
}

macro_rules! scalar {
    ($name:ident, $t:ty) => {
        pub fn $name(&mut self) -> Res<$t> {
            let raw = self.take(std::mem::size_of::<$t>())?;
            Ok(<$t>::from_le_bytes(raw.try_into().unwrap()))
        }
    };
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8], names: &'a [String]) -> Self {
        Reader { data, pos: 0, names }
    }

    pub fn tell(&self) -> usize {
        self.pos
    }

    pub fn seek(&mut self, pos: i64) -> Res<()> {
        if pos < 0 || pos as usize > self.data.len() {
            return archive_err(format!("seek out of range: {pos} (size {})", self.data.len()));
        }
        self.pos = pos as usize;
        Ok(())
    }

    pub fn skip(&mut self, count: usize) -> Res<()> {
        self.seek((self.pos + count) as i64)
    }

    fn take(&mut self, count: usize) -> Res<&'a [u8]> {
        if self.pos + count > self.data.len() {
            return archive_err(format!(
                "read past end: want {count} at {} (size {})",
                self.pos,
                self.data.len()
            ));
        }
        let out = &self.data[self.pos..self.pos + count];
        self.pos += count;
        Ok(out)
    }

    scalar!(u8, u8);
    scalar!(i8, i8);
    scalar!(u16, u16);
    scalar!(i16, i16);
    scalar!(u32, u32);
    scalar!(i32, i32);
    scalar!(u64, u64);
    scalar!(i64, i64);
    scalar!(f32, f32);
    scalar!(f64, f64);

    /// FArchive serializes bool as int32.
    pub fn bool32(&mut self) -> Res<bool> {
        Ok(self.i32()? != 0)
    }

    pub fn bytes(&mut self, count: usize) -> Res<&'a [u8]> {
        self.take(count)
    }

    /// FString: int32 length incl. terminator, negative means UTF-16.
    pub fn fstring(&mut self) -> Res<String> {
        let length = self.i32()? as i64;
        if length == 0 {
            return Ok(String::new());
        }
        if length < 0 {
            let raw = self.take((-length * 2) as usize)?;
            let units: Vec<u16> = raw.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            return Ok(String::from_utf16_lossy(&units).trim_end_matches('\0').to_string());
        }
        let raw = self.take(length as usize)?;
        Ok(String::from_utf8_lossy(raw).trim_end_matches('\0').to_string())
    }

    pub fn guid(&mut self) -> Res<String> {
        // One 16-byte read: a guid cut by the end of the buffer must not move the cursor.
        let raw = self.take(16)?;
        let word = |i: usize| u32::from_le_bytes(raw[i * 4..i * 4 + 4].try_into().unwrap());
        Ok(format!("{:08X}{:08X}{:08X}{:08X}", word(0), word(1), word(2), word(3)))
    }

    pub fn fname(&mut self) -> Res<FName> {
        let index = self.i32()?;
        let number = self.i32()?;
        let text = if index >= 0 && (index as usize) < self.names.len() {
            self.names[index as usize].clone()
        } else {
            format!("<name#{index}>")
        };
        Ok(FName { text, number })
    }

    /// The FName as text, the way every caller wants it.
    pub fn name(&mut self) -> Res<String> {
        Ok(self.fname()?.to_string())
    }

    /// FPackageIndex: >0 export (1-based), <0 import (-1-based), 0 null.
    pub fn package_index(&mut self) -> Res<i32> {
        self.i32()
    }

    pub fn count(&mut self, max: i32) -> Res<usize> {
        let count = self.i32()?;
        if count < 0 || count > max {
            return archive_err(format!("implausible array count {count} at {}", self.pos - 4));
        }
        Ok(count as usize)
    }

    pub fn engine_version(&mut self) -> Res<String> {
        let major = self.u16()?;
        let minor = self.u16()?;
        let patch = self.u16()?;
        let changelist = self.u32()?;
        let branch = self.fstring()?;
        Ok(format!("{major}.{minor}.{patch} CL {changelist} {branch}").trim_end().to_string())
    }
}
