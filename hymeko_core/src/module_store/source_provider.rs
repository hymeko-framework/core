use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;

fn mix64(mut x: u64) -> u64 {
    // egyszerű, gyors keverés (SplitMix64-szerű)
    x ^= x >> 30;
    x = x.wrapping_mul(0xbf58476d1ce4e5b9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94d049bb133111eb);
    x ^= x >> 31;
    x
}

pub trait SourceProvider {
    fn canonicalize(&self, p: &Path) -> Result<PathBuf, String>;
    fn read(&mut self, p: &Path) -> Result<std::sync::Arc<str>, String>;
    fn cwd(&self) -> Option<PathBuf> {
        None
    }
    fn version(&self, path: &Path) -> Result<u64, String>;
}

pub struct StdFsProvider {
    cache: HashMap<PathBuf, std::sync::Arc<str>>,
}

impl StdFsProvider {
    pub fn new() -> Self {
        Self {
            cache: Default::default(),
        }
    }

    pub fn cwd(&self) -> Option<PathBuf> {
        std::env::current_dir().ok()
    }
}

impl SourceProvider for StdFsProvider {
    fn canonicalize(&self, p: &Path) -> Result<PathBuf, String> {
        let absolute = if p.is_absolute() {
            p.to_path_buf()
        } else {
            std::env::current_dir().map_err(|e| e.to_string())?.join(p)
        };
        // Validate traversal at the filesystem root, but let the filesystem resolve
        // symlinks before parent components; lexical collapse must not change that.
        normalize_path(&absolute)?;
        std::fs::canonicalize(p).map_err(|e| e.to_string())
    }

    fn read(&mut self, p: &Path) -> Result<std::sync::Arc<str>, String> {
        if let Some(s) = self.cache.get(p) {
            return Ok(s.clone());
        }
        let s = std::fs::read_to_string(p).map_err(|e| e.to_string())?;
        let a: std::sync::Arc<str> = s.into();
        self.cache.insert(p.to_path_buf(), a.clone());
        Ok(a)
    }

    fn version(&self, path: &Path) -> Result<u64, String> {
        let md = std::fs::metadata(path).map_err(|e| e.to_string())?;
        let len = md.len();

        let mtime_ns: u128 = match md.modified() {
            Ok(t) => match t.duration_since(UNIX_EPOCH) {
                Ok(d) => d.as_nanos(),
                Err(_) => 0,
            },
            Err(_) => 0,
        };

        // 128bit -> két u64
        let lo = (mtime_ns & 0xFFFF_FFFF_FFFF_FFFF) as u64;
        let hi = (mtime_ns >> 64) as u64;

        // mix(mtime_hi, mtime_lo, len)
        let v = mix64(hi) ^ mix64(lo) ^ mix64(len);
        Ok(v)
    }
}

/// Sources held in memory, keyed by their logical path, for targets and tests without a disk.
///
/// A logical path is spelled the same on every target: `/` between segments, `\` read as a
/// separator too, `.` and empty segments dropped, `a/../b` collapsed, and a leading root or drive
/// kept. A `..` that would leave the root is an error, as in [`StdFsProvider`]. A browser page, a
/// Windows test and a Linux service therefore name one file one way, and a relative import that
/// the module store resolves as `dir/../other/x.hymeko` reaches the key inserted as
/// `other/x.hymeko`.
///
/// Versions are content hashes, so replacing a source invalidates what was compiled from it. A
/// missing source is reported with the paths that were provided, and `read_count` counts reads
/// per key for tests of the store's caching.
#[derive(Debug, Clone, Default)]
pub struct MemProvider {
    files: HashMap<String, Arc<str>>,
    reads: Arc<Mutex<HashMap<String, usize>>>,
}

impl MemProvider {
    pub fn insert_file(&mut self, path: impl Into<PathBuf>, content: impl Into<Arc<str>>) {
        let path = path.into();
        let key = logical_path(&path).unwrap_or_else(|_| slashed(&path));
        self.files.insert(key, content.into());
    }

    pub fn with_file(mut self, path: impl Into<PathBuf>, content: impl Into<Arc<str>>) -> Self {
        self.insert_file(path, content);
        self
    }

    /// The logical paths this provider serves, sorted.
    pub fn paths(&self) -> Vec<&str> {
        let mut out: Vec<&str> = self.files.keys().map(String::as_str).collect();
        out.sort_unstable();
        out
    }

    pub fn read_count(&self, p: impl AsRef<Path>) -> usize {
        let m = self.reads.lock().unwrap();
        logical_path(p.as_ref())
            .ok()
            .and_then(|key| m.get(&key).copied())
            .unwrap_or(0)
    }

    fn missing(&self, key: &str) -> String {
        let provided = self.paths();
        if provided.is_empty() {
            format!("file not found: {key}; no sources were provided")
        } else {
            format!("file not found: {key}; provided: {}", provided.join(", "))
        }
    }
}

impl SourceProvider for MemProvider {
    fn canonicalize(&self, p: &Path) -> Result<PathBuf, String> {
        logical_path(p).map(PathBuf::from)
    }

    fn read(&mut self, p: &Path) -> Result<Arc<str>, String> {
        let key = logical_path(p)?;
        {
            let mut m = self.reads.lock().unwrap();
            *m.entry(key.clone()).or_insert(0) += 1;
        }
        self.files
            .get(&key)
            .cloned()
            .ok_or_else(|| self.missing(&key))
    }

    fn version(&self, path: &Path) -> Result<u64, String> {
        let key = logical_path(path)?;
        let s = self.files.get(&key).ok_or_else(|| self.missing(&key))?;

        // content-hash → u64 version
        let h = blake3::hash(s.as_bytes());
        let b = h.as_bytes();
        Ok(u64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }
}

/// The logical spelling [`MemProvider`] keys by. `\` becomes `/` before the components are read,
/// so a backslash separates on every target rather than only on Windows.
fn logical_path(path: &Path) -> Result<String, String> {
    normalize_path(Path::new(&slashed(path))).map(|normalized| slashed(&normalized))
}

fn slashed(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn normalize_path(path: &Path) -> Result<PathBuf, String> {
    normalize_components(path.components())
        .map_err(|reason| format!("{reason}: {}", path.display()))
}

pub(crate) fn normalize_components<'a>(
    components: impl Iterator<Item = std::path::Component<'a>>,
) -> Result<PathBuf, &'static str> {
    use std::path::Component;
    let mut normalized = PathBuf::new();
    for component in components {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !matches!(
                    normalized.components().next_back(),
                    Some(Component::Normal(_))
                ) {
                    return Err("path escapes provider root");
                }
                normalized.pop();
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    Ok(normalized)
}
