//! Impostazioni dell'analisi (le stesse del programma Python, con gli stessi valori di partenza).

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Meno fotogrammi sui video lunghi.
    pub adaptive_frames: bool,
    /// Analizza solo la parte centrale del fotogramma (esclude bordi neri e scritte).
    pub smart_crop: bool,
    /// Quanta parte del lato si tiene nel ritaglio (0,2-1,0).
    pub crop_ratio: f64,
    /// Sotto questa luminanza media un fotogramma è considerato nero.
    pub min_frame_luminance: f64,
    pub min_frames_required: usize,
    /// Fotogrammi analizzati quando `adaptive_frames` è spento.
    pub frame_sampling_base: usize,
    pub weight_purity: f64,
    pub weight_density: f64,
    pub weight_stability: f64,
    pub blockiness_spacings: Vec<usize>,
    /// Differenza media sotto la quale due fotogrammi di fila sono "fermi".
    pub dead_segment_threshold: f64,
    pub dead_segment_min_duration_sec: f64,
    pub upscale_ssim_weight: f64,
    pub scene_detection: bool,
    pub scene_max_samples: usize,
    pub cache_enabled: bool,
    pub cache_ttl_hours: u64,
    /// Video analizzati insieme (0 = automatico).
    pub max_threads: usize,
    /// Secondi massimi per un singolo video.
    pub timeout_seconds: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            adaptive_frames: true,
            smart_crop: true,
            crop_ratio: 0.5,
            min_frame_luminance: 15.0,
            min_frames_required: 3,
            frame_sampling_base: 60,
            weight_purity: 0.4,
            weight_density: 0.35,
            weight_stability: 0.25,
            blockiness_spacings: vec![8, 16, 32],
            dead_segment_threshold: 5.0,
            dead_segment_min_duration_sec: 1.0,
            upscale_ssim_weight: 0.35,
            scene_detection: true,
            scene_max_samples: 40,
            cache_enabled: true,
            cache_ttl_hours: 168,
            max_threads: 0,
            timeout_seconds: 500,
        }
    }
}

impl Config {
    /// Riporta ogni valore in un intervallo sensato: un file di impostazioni rovinato o
    /// modificato a mano non deve poter bloccare o far andare in crisi l'analisi.
    pub fn sanitized(mut self) -> Self {
        let d = Config::default();
        let fin = |v: f64, dflt: f64| if v.is_finite() { v } else { dflt };
        self.crop_ratio = fin(self.crop_ratio, d.crop_ratio).clamp(0.2, 1.0);
        self.min_frame_luminance =
            fin(self.min_frame_luminance, d.min_frame_luminance).clamp(0.0, 100.0);
        self.min_frames_required = self.min_frames_required.clamp(1, 20);
        self.frame_sampling_base = self.frame_sampling_base.clamp(4, 200);
        self.weight_purity = fin(self.weight_purity, d.weight_purity).clamp(0.0, 1.0);
        self.weight_density = fin(self.weight_density, d.weight_density).clamp(0.0, 1.0);
        self.weight_stability = fin(self.weight_stability, d.weight_stability).clamp(0.0, 1.0);
        if self.weight_purity + self.weight_density + self.weight_stability <= 0.0 {
            self.weight_purity = d.weight_purity;
            self.weight_density = d.weight_density;
            self.weight_stability = d.weight_stability;
        }
        self.blockiness_spacings.retain(|s| (2..=128).contains(s));
        self.blockiness_spacings.truncate(8);
        if self.blockiness_spacings.is_empty() {
            self.blockiness_spacings = d.blockiness_spacings;
        }
        self.dead_segment_threshold =
            fin(self.dead_segment_threshold, d.dead_segment_threshold).clamp(0.1, 50.0);
        self.dead_segment_min_duration_sec =
            fin(self.dead_segment_min_duration_sec, 1.0).clamp(0.1, 60.0);
        self.upscale_ssim_weight =
            fin(self.upscale_ssim_weight, d.upscale_ssim_weight).clamp(0.0, 1.0);
        self.scene_max_samples = self.scene_max_samples.clamp(4, 200);
        self.cache_ttl_hours = self.cache_ttl_hours.clamp(1, 24 * 365);
        self.max_threads = self.max_threads.min(32);
        self.timeout_seconds = self.timeout_seconds.clamp(10, 24 * 3600);
        self
    }

    /// Quanti video si analizzano insieme.
    pub fn threads(&self) -> usize {
        if self.max_threads > 0 {
            return self.max_threads;
        }
        let cpus = std::thread::available_parallelism().map_or(2, |n| n.get());
        cpus.saturating_sub(1).clamp(1, 8)
    }

    /// Dati che, se cambiano, rendono vecchi i risultati salvati.
    pub fn algorithm_key(&self) -> String {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        "framedesk-1".hash(&mut h);
        self.blockiness_spacings.hash(&mut h);
        for w in [
            self.weight_purity,
            self.weight_density,
            self.weight_stability,
            self.upscale_ssim_weight,
            self.crop_ratio,
            self.min_frame_luminance,
            self.dead_segment_threshold,
        ] {
            w.to_bits().hash(&mut h);
        }
        (self.smart_crop, self.scene_detection, self.adaptive_frames).hash(&mut h);
        (self.scene_max_samples, self.frame_sampling_base).hash(&mut h);
        format!("{:016x}", h.finish())
    }
}

/// Cartella dei dati del programma: `%LOCALAPPDATA%\FrameDesk` (o la cartella temporanea).
pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("XDG_DATA_HOME"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("share")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("FrameDesk")
}

pub fn load(path: &std::path::Path) -> Config {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<Config>(&t).ok())
        .unwrap_or_default()
        .sanitized()
}

pub fn save(path: &std::path::Path, cfg: &Config) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(t) = serde_json::to_string_pretty(cfg) {
        let _ = std::fs::write(path, t);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_python_program() {
        let c = Config::default();
        assert_eq!(
            (c.weight_purity, c.weight_density, c.weight_stability),
            (0.4, 0.35, 0.25)
        );
        assert_eq!(c.blockiness_spacings, vec![8, 16, 32]);
        assert_eq!(c.scene_max_samples, 40);
        assert_eq!(c.min_frames_required, 3);
    }

    #[test]
    fn hostile_values_are_pulled_back() {
        let c = Config {
            crop_ratio: f64::NAN,
            weight_purity: -5.0,
            weight_density: f64::INFINITY,
            weight_stability: 0.0,
            blockiness_spacings: vec![0, 1, 100000],
            min_frames_required: 0,
            frame_sampling_base: 0,
            scene_max_samples: 1_000_000,
            max_threads: 10_000,
            timeout_seconds: 0,
            ..Config::default()
        }
        .sanitized();
        assert!((0.2..=1.0).contains(&c.crop_ratio));
        assert_eq!(c.blockiness_spacings, vec![8, 16, 32]);
        assert!(c.min_frames_required >= 1 && c.frame_sampling_base >= 4);
        assert!(c.scene_max_samples <= 200 && c.max_threads <= 32 && c.timeout_seconds >= 10);
        assert!(c.weight_purity + c.weight_density + c.weight_stability > 0.0);
    }

    #[test]
    fn all_zero_weights_fall_back_to_defaults() {
        let c = Config {
            weight_purity: 0.0,
            weight_density: 0.0,
            weight_stability: 0.0,
            ..Config::default()
        }
        .sanitized();
        assert_eq!(c.weight_purity, 0.4);
    }

    #[test]
    fn algorithm_key_changes_with_the_formulas_only() {
        let a = Config::default();
        let b = Config {
            weight_purity: 0.5,
            ..Config::default()
        };
        let c = Config {
            cache_enabled: false,
            max_threads: 3,
            ..Config::default()
        };
        assert_ne!(a.algorithm_key(), b.algorithm_key());
        assert_eq!(a.algorithm_key(), c.algorithm_key());
    }

    #[test]
    fn save_and_load_round_trip_and_broken_file_gives_defaults() {
        let dir = std::env::temp_dir().join(format!("fd_cfg_{}", std::process::id()));
        let p = dir.join("c.json");
        let c = Config {
            smart_crop: false,
            ..Config::default()
        };
        save(&p, &c);
        assert_eq!(load(&p), c);
        std::fs::write(&p, "{ non json").unwrap();
        assert_eq!(load(&p), Config::default());
        assert_eq!(load(&dir.join("manca.json")), Config::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn thread_count_is_at_least_one() {
        assert!(Config::default().threads() >= 1);
        assert_eq!(
            Config {
                max_threads: 3,
                ..Config::default()
            }
            .threads(),
            3
        );
    }
}
