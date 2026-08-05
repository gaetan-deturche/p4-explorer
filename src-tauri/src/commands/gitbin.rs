//! Git's binary-patch encoding (`GIT binary patch` / `literal N` sections):
//! the whole file, zlib-deflated, base85-coded, 52 payload bytes per line with
//! a leading length character. Matching git's format exactly — rather than
//! inventing one — means a patch Auger exports applies with `git apply`, and a
//! `git diff --binary` patch applies in Auger.

use std::io::{Read, Write};

/// Git's base85 alphabet (not ASCII85 — the order matters).
const B85: &[u8; 85] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz!#$%&()*+-;<=>?@^_`{|}~";

fn b85_index(c: u8) -> Option<u32> {
    B85.iter().position(|&b| b == c).map(|i| i as u32)
}

/// One literal section's body: `data` deflated then base85-coded, 52 bytes per
/// line, each line prefixed with its byte count ('A'=1..'Z'=26, 'a'=27..'z'=52).
pub(crate) fn encode_literal_body(data: &[u8]) -> String {
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    let _ = z.write_all(data);
    let deflated = z.finish().unwrap_or_default();

    let mut out = String::new();
    for chunk in deflated.chunks(52) {
        let n = chunk.len() as u8;
        out.push(if n <= 26 { (b'A' + n - 1) as char } else { (b'a' + n - 27) as char });
        for four in chunk.chunks(4) {
            // Big-endian 32-bit group, zero-padded on the right like git.
            let mut v: u32 = 0;
            for i in 0..4 {
                v = (v << 8) | u32::from(*four.get(i).unwrap_or(&0));
            }
            let mut digits = [0u8; 5];
            for d in digits.iter_mut().rev() {
                *d = B85[(v % 85) as usize];
                v /= 85;
            }
            out.push_str(std::str::from_utf8(&digits).unwrap_or_default());
        }
        out.push('\n');
    }
    out
}

/// Git's blob id for `data` — what the `index` line carries. `git apply`
/// verifies the OLD one against the file it patches (probed: zeros are rejected
/// for an existing file), so these have to be real.
pub(crate) fn blob_sha(data: &[u8]) -> String {
    let mut h = sha1_smol::Sha1::new();
    h.update(format!("blob {}\0", data.len()).as_bytes());
    h.update(data);
    h.digest().to_string()
}

/// A full section for one file, in the exact shape `git diff --binary` emits
/// (minus the optional reverse hunk, which `git apply` doesn't need — probed).
/// `old_sha` None marks an added file. `path` is what lands after `a/` / `b/`.
pub(crate) fn encode_section(path: &str, data: &[u8], old_sha: Option<&str>) -> String {
    let new_sha = blob_sha(data);
    let header = match old_sha {
        // Modified: full index line with a trailing mode.
        Some(old) => format!("index {old}..{new_sha} 100644\n"),
        // Added: mode goes on its own line, the old id is all-zeros.
        None => format!("new file mode 100644\nindex {}..{new_sha}\n", "0".repeat(40)),
    };
    format!(
        "diff --git a/{path} b/{path}\n{header}GIT binary patch\nliteral {}\n{}\n",
        data.len(),
        encode_literal_body(data)
    )
}

/// Decode the base85 lines of a `literal <size>` section back to the file's
/// bytes. `lines` are the coded lines (without the `literal` header), `size`
/// the announced inflated length — mismatches are corruption, not tolerance.
pub(crate) fn decode_literal(size: usize, lines: &[&str]) -> Result<Vec<u8>, String> {
    let mut deflated: Vec<u8> = Vec::new();
    for (ln, line) in lines.iter().enumerate() {
        let bytes = line.as_bytes();
        let Some(&len_c) = bytes.first() else {
            return Err(format!("binary patch line {} is empty", ln + 1));
        };
        let want = match len_c {
            b'A'..=b'Z' => (len_c - b'A' + 1) as usize,
            b'a'..=b'z' => (len_c - b'a' + 27) as usize,
            _ => return Err(format!("binary patch line {}: bad length character", ln + 1)),
        };
        let coded = &bytes[1..];
        if coded.len() != want.div_ceil(4) * 5 {
            return Err(format!("binary patch line {}: wrong length", ln + 1));
        }
        let mut decoded: Vec<u8> = Vec::with_capacity(want.div_ceil(4) * 4);
        for five in coded.chunks(5) {
            let mut v: u32 = 0;
            for &c in five {
                let d = b85_index(c)
                    .ok_or_else(|| format!("binary patch line {}: bad base85 character", ln + 1))?;
                // Git rejects overflow here too; a wrapped value is corruption.
                v = v
                    .checked_mul(85)
                    .and_then(|x| x.checked_add(d))
                    .ok_or_else(|| format!("binary patch line {}: base85 overflow", ln + 1))?;
            }
            decoded.extend_from_slice(&v.to_be_bytes());
        }
        deflated.extend_from_slice(&decoded[..want]);
    }
    let mut out = Vec::with_capacity(size);
    let mut inflater = flate2::read::ZlibDecoder::new(deflated.as_slice());
    inflater
        .read_to_end(&mut out)
        .map_err(|e| format!("binary patch does not inflate: {e}"))?;
    if out.len() != size {
        return Err(format!(
            "binary patch announced {size} bytes but inflated to {}",
            out.len()
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(len: usize) -> Vec<u8> {
        // Non-text bytes with structure, like a real asset: not all-zero, not random.
        (0..len).map(|i| ((i * 37) ^ (i >> 3)) as u8).collect()
    }

    #[test]
    fn round_trips_arbitrary_bytes() {
        for len in [0usize, 1, 3, 4, 5, 51, 52, 53, 104, 4096, 70001] {
            let data = sample(len);
            let body = encode_literal_body(&data);
            let lines: Vec<&str> = body.lines().collect();
            let back = decode_literal(len, &lines).expect("decode");
            assert_eq!(back, data, "len {len}");
        }
    }

    #[test]
    fn lines_carry_at_most_52_bytes_with_git_length_chars() {
        let body = encode_literal_body(&sample(120)); // deflates to something small
        for line in body.lines() {
            let c = line.as_bytes()[0];
            assert!(c.is_ascii_alphabetic(), "length char");
            let n = if c.is_ascii_uppercase() { c - b'A' + 1 } else { c - b'a' + 27 };
            assert!(n as usize <= 52);
            assert_eq!(line.len() - 1, (n as usize).div_ceil(4) * 5);
        }
    }

    #[test]
    fn announced_size_is_enforced() {
        let data = sample(64);
        let body = encode_literal_body(&data);
        let lines: Vec<&str> = body.lines().collect();
        assert!(decode_literal(63, &lines).is_err());
        assert!(decode_literal(65, &lines).is_err());
    }

    #[test]
    fn corrupt_characters_are_rejected() {
        let body = encode_literal_body(&sample(40));
        let mut lines: Vec<String> = body.lines().map(String::from).collect();
        lines[0].replace_range(1..2, "\""); // not in git's base85 alphabet
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        assert!(decode_literal(40, &refs).is_err());
    }

    #[test]
    fn section_has_the_git_shape() {
        let s = encode_section("Curiosity/main/Content/X.uasset", &sample(10), None);
        let mut l = s.lines();
        assert_eq!(
            l.next().unwrap(),
            "diff --git a/Curiosity/main/Content/X.uasset b/Curiosity/main/Content/X.uasset"
        );
        assert_eq!(l.next().unwrap(), "new file mode 100644");
        assert!(l.next().unwrap().starts_with("index 0000000000000000000000000000000000000000.."));
        assert_eq!(l.next().unwrap(), "GIT binary patch");
        assert_eq!(l.next().unwrap(), "literal 10");
        assert!(s.ends_with("\n\n"));

        let old = "d".repeat(40);
        let m = encode_section("x.bin", &sample(10), Some(&old));
        assert!(m.lines().nth(1).unwrap().starts_with("index dddd"));
        assert!(m.lines().nth(1).unwrap().ends_with(" 100644"));
    }

    #[test]
    fn blob_sha_matches_git() {
        // `printf 'hello' | git hash-object --stdin`
        assert_eq!(blob_sha(b"hello"), "b6fc4c620b67d95f953a5c1c1230aaab5db5a1b0");
    }

    /// The whole point of using git's format: `git apply` must accept what we
    /// emit, for both a modified file and an added one. Skips silently where
    /// git isn't installed.
    #[test]
    fn git_apply_accepts_our_sections() {
        let git_ok = std::process::Command::new("git")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !git_ok {
            return;
        }
        let dir = std::env::temp_dir().join(format!("auger-gitbin-apply-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| {
            std::process::Command::new("git").current_dir(&dir).args(args).output().unwrap()
        };
        assert!(git(&["init", "-q"]).status.success());

        let old = sample(2600);
        let new = sample(3100).iter().map(|b| b ^ 0x5A).collect::<Vec<u8>>();
        std::fs::write(dir.join("asset.bin"), &old).unwrap();

        // Modified file: old sha must be the blob of what's on disk.
        let mut patch = encode_section("asset.bin", &new, Some(&blob_sha(&old)));
        // Added file: no pre-image.
        patch.push_str(&encode_section("fresh.bin", &old, None));
        std::fs::write(dir.join("both.patch"), &patch).unwrap();

        let out = git(&["apply", "both.patch"]);
        assert!(
            out.status.success(),
            "git apply refused our patch: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(std::fs::read(dir.join("asset.bin")).unwrap(), new);
        assert_eq!(std::fs::read(dir.join("fresh.bin")).unwrap(), old);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
