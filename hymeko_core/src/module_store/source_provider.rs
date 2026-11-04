use std::collections::{BTreeMap, HashMap};
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

    /// The source's length in bytes when the provider knows it without reading the source, so a
    /// bounded reader can refuse an oversized one before loading it. `None` means unknown.
    fn byte_len(&self, _path: &Path) -> Option<u64> {
        None
    }
}

/// A provider lent to a compilation, so the caller still holds it afterwards: what a
/// [`ProjectProvider`] captured, or a [`MemProvider`]'s read counts.
impl<P: SourceProvider + ?Sized> SourceProvider for &mut P {
    fn canonicalize(&self, p: &Path) -> Result<PathBuf, String> {
        (**self).canonicalize(p)
    }

    fn read(&mut self, p: &Path) -> Result<Arc<str>, String> {
        (**self).read(p)
    }

    fn cwd(&self) -> Option<PathBuf> {
        (**self).cwd()
    }

    fn version(&self, path: &Path) -> Result<u64, String> {
        (**self).version(path)
    }

    fn byte_len(&self, path: &Path) -> Option<u64> {
        (**self).byte_len(path)
    }
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

    fn byte_len(&self, path: &Path) -> Option<u64> {
        match self.cache.get(path) {
            Some(text) => Some(text.len() as u64),
            None => std::fs::metadata(path).ok().map(|md| md.len()),
        }
    }
}

/// Sources held in memory, keyed by their logical path, for targets and tests without a disk.
///
/// A logical path is spelled the same on every target: `/` between segments, `\` read as a
/// separator too, `.` and empty segments dropped, `a/../b` collapsed, a Windows extended-length
/// `\\?\` prefix dropped, and a leading root or drive kept. A `..` that would leave the root is an
/// error, as in [`StdFsProvider`]. A browser page, a
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
        Ok(content_version(s))
    }

    fn byte_len(&self, path: &Path) -> Option<u64> {
        let key = logical_path(path).ok()?;
        self.files.get(&key).map(|text| text.len() as u64)
    }
}

/// In-memory sources over a base provider: a source held here shadows the base's, and every
/// other source is read from the base. An editor's unsaved buffers over the file system are the
/// case it is for.
///
/// A source is keyed by the base's canonical path where the base can canonicalize it, so a buffer
/// and the module store name a file the same way however the base spells paths. A file that
/// exists only here (a buffer never saved) keeps its logical spelling, and the module store
/// reaches it through the same fallback.
pub struct OverlayProvider<B> {
    memory: MemProvider,
    base: B,
}

impl<B: SourceProvider> OverlayProvider<B> {
    pub fn new(base: B) -> Self {
        Self {
            memory: MemProvider::default(),
            base,
        }
    }

    pub fn insert_file(&mut self, path: impl AsRef<Path>, content: impl Into<Arc<str>>) {
        let path = path.as_ref();
        let key = self
            .canonicalize(path)
            .unwrap_or_else(|_| path.to_path_buf());
        self.memory.insert_file(key, content);
    }

    pub fn with_file(mut self, path: impl AsRef<Path>, content: impl Into<Arc<str>>) -> Self {
        self.insert_file(path, content);
        self
    }

    /// The sources held in memory.
    pub fn memory(&self) -> &MemProvider {
        &self.memory
    }

    pub fn base(&self) -> &B {
        &self.base
    }
}

impl<B: SourceProvider> SourceProvider for OverlayProvider<B> {
    fn canonicalize(&self, p: &Path) -> Result<PathBuf, String> {
        self.base
            .canonicalize(p)
            .or_else(|_| self.memory.canonicalize(p))
    }

    fn read(&mut self, p: &Path) -> Result<Arc<str>, String> {
        let key = self.canonicalize(p)?;
        if self.memory.byte_len(&key).is_some() {
            self.memory.read(&key)
        } else {
            self.base.read(&key)
        }
    }

    fn cwd(&self) -> Option<PathBuf> {
        self.base.cwd()
    }

    fn version(&self, path: &Path) -> Result<u64, String> {
        let key = self.canonicalize(path)?;
        self.memory
            .version(&key)
            .or_else(|_| self.base.version(&key))
    }

    fn byte_len(&self, path: &Path) -> Option<u64> {
        let key = self.canonicalize(path).ok()?;
        self.memory
            .byte_len(&key)
            .or_else(|| self.base.byte_len(&key))
    }
}

/// Bounds on a set of sources: how many files, and how many bytes of text in all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceLimits {
    pub files: usize,
    pub bytes: usize,
}

/// The sources one compilation reads from a project directory, captured as they are read.
///
/// Only sources under the root are admitted. A repeated read is served from the capture, so a
/// bundle built from [`Self::sources`] holds exactly the bytes the compiler saw even if a file
/// changes on disk meanwhile. The capture refuses to grow past its [`SourceLimits`], and refuses
/// an oversized source before reading it when the inner provider knows its length. Lend it to the
/// compilation (`&mut project`) to keep the capture: [`Self::sources`] is then the closed source
/// set, keyed by root-relative logical path, which a [`MemProvider`] recompiles anywhere.
pub struct ProjectProvider<P> {
    inner: P,
    root: PathBuf,
    limits: SourceLimits,
    sources: BTreeMap<String, Arc<str>>,
    bytes: usize,
}

impl<P: SourceProvider> ProjectProvider<P> {
    /// A capture of the sources under `root`, which is canonicalized through `inner`.
    pub fn new(inner: P, root: &Path, limits: SourceLimits) -> Result<Self, String> {
        let root = inner.canonicalize(root)?;
        Ok(Self {
            inner,
            root,
            limits,
            sources: BTreeMap::new(),
            bytes: 0,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The root-relative logical path of a canonical path; an error when it lies outside the root.
    pub fn relative(&self, path: &Path) -> Result<String, String> {
        let relative = path.strip_prefix(&self.root).map_err(|_| {
            format!(
                "source {} is outside the project root {}",
                path.display(),
                self.root.display()
            )
        })?;
        relative
            .to_str()
            .map(|relative| relative.replace('\\', "/"))
            .ok_or_else(|| format!("source path is not UTF-8: {}", path.display()))
    }

    /// The sources read so far, by root-relative logical path.
    pub fn sources(&self) -> &BTreeMap<String, Arc<str>> {
        &self.sources
    }

    /// The bytes of text captured so far.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn into_sources(self) -> BTreeMap<String, Arc<str>> {
        self.sources
    }

    fn oversized(&self) -> String {
        let bytes = self.limits.bytes;
        if bytes > 0 && bytes.is_multiple_of(1_048_576) {
            format!("source project exceeds {} MiB", bytes / 1_048_576)
        } else {
            format!("source project exceeds {bytes} bytes")
        }
    }
}

impl<P: SourceProvider> SourceProvider for ProjectProvider<P> {
    fn canonicalize(&self, p: &Path) -> Result<PathBuf, String> {
        let path = self.inner.canonicalize(p)?;
        self.relative(&path)?;
        Ok(path)
    }

    fn read(&mut self, p: &Path) -> Result<Arc<str>, String> {
        let path = self.inner.canonicalize(p)?;
        let relative = self.relative(&path)?;
        if let Some(text) = self.sources.get(&relative) {
            return Ok(text.clone());
        }
        if self.sources.len() >= self.limits.files {
            return Err(format!(
                "source project exceeds {} files",
                self.limits.files
            ));
        }
        let remaining = self.limits.bytes.saturating_sub(self.bytes);
        if self
            .inner
            .byte_len(&path)
            .is_some_and(|len| len > remaining as u64)
        {
            return Err(self.oversized());
        }
        let text = self.inner.read(&path)?;
        if text.len() > remaining {
            return Err(self.oversized());
        }
        self.bytes += text.len();
        self.sources.insert(relative, text.clone());
        Ok(text)
    }

    fn cwd(&self) -> Option<PathBuf> {
        self.inner.cwd()
    }

    fn version(&self, path: &Path) -> Result<u64, String> {
        self.inner.version(&self.canonicalize(path)?)
    }

    fn byte_len(&self, path: &Path) -> Option<u64> {
        self.inner.byte_len(&self.canonicalize(path).ok()?)
    }
}

/// A content hash as a version: the first eight bytes of its BLAKE3 digest.
fn content_version(s: &str) -> u64 {
    let h = blake3::hash(s.as_bytes());
    let b = h.as_bytes();
    u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
}

/// The logical spelling [`MemProvider`] keys by. `\` becomes `/` before the components are read,
/// so a backslash separates on every target rather than only on Windows. The extended-length
/// prefix `std::fs::canonicalize` returns on Windows names the same file as the plain path, so it
/// is dropped: `\\?\C:\x` and `C:\x` are one key, as are `\\?\UNC\host\share` and
/// `\\host\share`.
fn logical_path(path: &Path) -> Result<String, String> {
    let spelled = slashed(path);
    let spelled = match spelled.strip_prefix("//?/") {
        Some(rest) => match rest.strip_prefix("UNC/") {
            Some(share) => format!("//{share}"),
            None => rest.to_owned(),
        },
        None => spelled,
    };
    normalize_path(Path::new(&spelled)).map(|normalized| slashed(&normalized))
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
