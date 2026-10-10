//! Standalone executables: a copy of `novo` with a program's source files
//! appended. At startup `novo` looks for such a payload at the end of its own
//! executable and, if there is one, runs that program.
//!
//! Layout of the payload: the entry file's path, then for each file its path
//! and text, each string as a little-endian u64 length and UTF-8 bytes; then
//! the payload's length as a u64 and the 8-byte magic marker.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const MAGIC: &[u8; 8] = b"NOVOPKG1";

/// Where bundled files appear to live, for paths in messages and imports.
pub const ROOT: &str = "<bundle>";

pub struct Bundle {
    pub entry: PathBuf,
    pub files: HashMap<PathBuf, String>,
}

/// Writes `exe` with the given files appended; `files` maps paths relative to
/// a common directory to their text.
pub fn write(
    exe: &Path,
    out: &Path,
    entry: &str,
    files: &[(String, String)],
) -> std::io::Result<()> {
    let mut payload = Vec::new();
    let mut put = |s: &str| {
        payload.extend_from_slice(&(s.len() as u64).to_le_bytes());
        payload.extend_from_slice(s.as_bytes());
    };
    put(entry);
    for (path, text) in files {
        put(path);
        put(text);
    }
    let mut binary = std::fs::read(exe)?;
    if let Some(start) = payload_start(&binary) {
        binary.truncate(start);
    }
    binary.extend_from_slice(&payload);
    binary.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    binary.extend_from_slice(MAGIC);
    let mut file = std::fs::File::create(out)?;
    file.write_all(&binary)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

/// Where the payload of an executable that has one begins.
fn payload_start(binary: &[u8]) -> Option<usize> {
    let n = binary.len();
    if n < 16 || &binary[n - 8..] != MAGIC {
        return None;
    }
    let len = u64::from_le_bytes(binary[n - 16..n - 8].try_into().ok()?) as usize;
    (n - 16).checked_sub(len)
}

/// The program bundled into the running executable, if any.
pub fn current() -> Option<Bundle> {
    let mut file = std::fs::File::open(std::env::current_exe().ok()?).ok()?;
    let size = file.seek(SeekFrom::End(0)).ok()?;
    if size < 16 {
        return None;
    }
    file.seek(SeekFrom::End(-16)).ok()?;
    let mut tail = [0u8; 16];
    file.read_exact(&mut tail).ok()?;
    if &tail[8..] != MAGIC {
        return None;
    }
    let len = u64::from_le_bytes(tail[..8].try_into().ok()?);
    file.seek(SeekFrom::Start(size.checked_sub(16 + len)?))
        .ok()?;
    let mut payload = vec![0u8; len as usize];
    file.read_exact(&mut payload).ok()?;
    parse(&payload)
}

fn parse(mut payload: &[u8]) -> Option<Bundle> {
    let mut next = || -> Option<String> {
        let len = u64::from_le_bytes(payload.get(..8)?.try_into().ok()?) as usize;
        let text = std::str::from_utf8(payload.get(8..8 + len)?)
            .ok()?
            .to_string();
        payload = &payload[8 + len..];
        Some(text)
    };
    let root = Path::new(ROOT);
    let entry = root.join(next()?);
    let mut files = HashMap::new();
    while let Some(path) = next() {
        files.insert(root.join(path), next()?);
    }
    Some(Bundle { entry, files })
}

/// Paths of `files` relative to their deepest common directory.
pub fn relative_paths(files: &[PathBuf]) -> Vec<String> {
    let common: Vec<_> = match files.first() {
        Some(first) => {
            let mut common: Vec<_> = first
                .parent()
                .map(|p| p.components().collect())
                .unwrap_or_default();
            for f in &files[1..] {
                let parts: Vec<_> = f
                    .parent()
                    .map(|p| p.components().collect())
                    .unwrap_or_default();
                let shared = common
                    .iter()
                    .zip(&parts)
                    .take_while(|(a, b)| a == b)
                    .count();
                common.truncate(shared);
            }
            common
        }
        None => Vec::new(),
    };
    files
        .iter()
        .map(|f| {
            f.components()
                .skip(common.len())
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/")
        })
        .collect()
}
