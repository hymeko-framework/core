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

#[derive(Clone, Default)]
pub struct MemProvider {
    files: HashMap<PathBuf, Arc<str>>,
    reads: Arc<Mutex<HashMap<PathBuf, usize>>>,
}

impl MemProvider {
    pub fn insert_file(&mut self, path: impl Into<PathBuf>, content: impl Into<Arc<str>>) {
        self.files.insert(path.into(), content.into());
    }

    pub fn with_file(mut self, path: impl Into<PathBuf>, content: impl Into<Arc<str>>) -> Self {
        self.files.insert(path.into(), content.into());
        self
    }

    pub fn read_count(&self, p: impl AsRef<Path>) -> usize {
        let m = self.reads.lock().unwrap();
        *m.get(p.as_ref()).unwrap_or(&0)
    }
}

impl SourceProvider for MemProvider {
    fn canonicalize(&self, p: &Path) -> Result<PathBuf, String> {
        // Tesztben elég az identity. (Ha akarsz, itt normalizálhatsz.)
        Ok(p.to_path_buf())
    }

    fn read(&mut self, p: &Path) -> Result<Arc<str>, String> {
        {
            let mut m = self.reads.lock().unwrap();
            *m.entry(p.to_path_buf()).or_insert(0) += 1;
        }
        self.files
            .get(p)
            .cloned()
            .ok_or_else(|| format!("file not found: {}", p.display()))
    }

    fn version(&self, path: &Path) -> Result<u64, String> {
        let s = self
            .files
            .get(path)
            .ok_or_else(|| format!("file not found: {}", path.display()))?;

        // content-hash → u64 version
        let h = blake3::hash(s.as_bytes());
        let b = h.as_bytes();
        Ok(u64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }
}
