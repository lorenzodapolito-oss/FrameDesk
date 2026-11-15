//! Cache dei risultati: se il file e le impostazioni non sono cambiati, non si rianalizza.
//! Le miniature non si salvano (il file resterebbe enorme e lento da leggere).

use crate::model::VideoResult;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_ENTRIES: usize = 300;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Entry {
    size: u64,
    mtime: u64,
    algorithm: String,
    saved: u64,
    result: VideoResult,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Cache {
    entries: HashMap<String, Entry>,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Dimensione e data di modifica (in secondi) di un file.
pub fn file_stamp(path: &Path) -> Option<(u64, u64)> {
    let m = std::fs::metadata(path).ok()?;
    let t = m
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_secs();
    Some((m.len(), t))
}

impl Cache {
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(t) = serde_json::to_string(self) {
            let _ = std::fs::write(path, t);
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Il risultato salvato, se il file è lo stesso, l'algoritmo non è cambiato e non è scaduto.
    pub fn get(&self, video: &Path, algorithm: &str, ttl_hours: u64) -> Option<VideoResult> {
        let e = self.entries.get(&key(video))?;
        let (size, mtime) = file_stamp(video)?;
        let fresh = now().saturating_sub(e.saved) <= ttl_hours.saturating_mul(3600);
        (e.size == size
            && e.mtime == mtime
            && e.algorithm == algorithm
            && fresh
            && e.result.error.is_none())
        .then(|| {
            let mut r = e.result.clone();
            r.from_cache = true;
            r
        })
    }

    pub fn set(&mut self, video: &Path, algorithm: &str, r: &VideoResult) {
        if r.error.is_some() {
            return;
        }
        let Some((size, mtime)) = file_stamp(video) else {
            return;
        };
        let mut r = r.clone();
        r.from_cache = false;
        self.entries.insert(
            key(video),
            Entry {
                size,
                mtime,
                algorithm: algorithm.to_string(),
                saved: now(),
                result: r,
            },
        );
        while self.entries.len() > MAX_ENTRIES {
            let oldest = self
                .entries
                .iter()
                .min_by_key(|(_, e)| e.saved)
                .map(|(k, _)| k.clone());
            match oldest {
                Some(k) => {
                    self.entries.remove(&k);
                }
                None => break,
            }
        }
    }
}

fn key(video: &Path) -> String {
    std::fs::canonicalize(video)
        .unwrap_or_else(|_| video.to_path_buf())
        .to_string_lossy()
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("fd_cache_{}_{name}", std::process::id()));
        let _ = std::fs::create_dir_all(&d);
        d
    }

    fn ok_result(name: &str) -> VideoResult {
        VideoResult {
            name: name.into(),
            score: 50.0,
            ..VideoResult::default()
        }
    }

    #[test]
    fn stores_and_returns_a_result_for_an_unchanged_file() {
        let d = tmp("a");
        let f = d.join("v.mp4");
        std::fs::write(&f, b"1234").unwrap();
        let mut c = Cache::default();
        c.set(&f, "algo", &ok_result("v.mp4"));
        let r = c.get(&f, "algo", 24).unwrap();
        assert!(r.from_cache);
        assert_eq!(r.name, "v.mp4");
        assert_eq!(c.len(), 1);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn changed_file_algorithm_or_age_invalidate_the_entry() {
        let d = tmp("b");
        let f = d.join("v.mp4");
        std::fs::write(&f, b"1234").unwrap();
        let mut c = Cache::default();
        c.set(&f, "algo", &ok_result("v"));
        assert!(c.get(&f, "altro", 24).is_none());
        std::fs::write(&f, b"12345678").unwrap();
        assert!(c.get(&f, "algo", 24).is_none(), "il file è cambiato");
        c.set(&f, "algo", &ok_result("v"));
        // Un risultato salvato "nel passato" è scaduto.
        if let Some(e) = c.entries.values_mut().next() {
            e.saved = 1;
        }
        assert!(c.get(&f, "algo", 1).is_none());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn errors_and_missing_files_are_never_cached() {
        let d = tmp("c");
        let f = d.join("v.mp4");
        std::fs::write(&f, b"1").unwrap();
        let mut c = Cache::default();
        c.set(&f, "a", &VideoResult::failed("v.mp4", "errore"));
        assert!(c.is_empty());
        c.set(&d.join("manca.mp4"), "a", &ok_result("x"));
        assert!(c.is_empty());
        assert!(c.get(&d.join("manca.mp4"), "a", 1).is_none());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_cache_is_bounded() {
        let d = tmp("d");
        let mut c = Cache::default();
        for i in 0..MAX_ENTRIES + 20 {
            let f = d.join(format!("v{i}.mp4"));
            std::fs::write(&f, b"1").unwrap();
            c.set(&f, "a", &ok_result("x"));
        }
        assert_eq!(c.len(), MAX_ENTRIES);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn save_load_round_trip_and_broken_file() {
        let d = tmp("e");
        let f = d.join("v.mp4");
        let p = d.join("cache.json");
        std::fs::write(&f, b"1").unwrap();
        let mut c = Cache::default();
        c.set(&f, "a", &ok_result("x"));
        c.save(&p);
        assert!(Cache::load(&p).get(&f, "a", 24).is_some());
        std::fs::write(&p, "rotto").unwrap();
        assert!(Cache::load(&p).is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }
}
