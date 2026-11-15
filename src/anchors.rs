//! Ancore: video di riferimento scelti dall'utente. Con almeno due ancore i punteggi si
//! confrontano con quelli dei tuoi video migliori e peggiori invece che con scale fisse.

use crate::model::VideoResult;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Anchor {
    pub purity_raw: f64,
    pub density_raw: f64,
    pub stability_raw: f64,
    pub blockiness: f64,
    pub fft_ratio: f64,
    pub upscale_score: f64,
    pub efficiency: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Anchors {
    /// Impronta dell'algoritmo: se cambia, le ancore non valgono più.
    pub algorithm: String,
    pub anchors: BTreeMap<String, Anchor>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Norm {
    pub purity: f64,
    pub density: f64,
    pub stability: f64,
    pub has_anchors: bool,
}

impl Anchors {
    pub fn new(algorithm: &str) -> Self {
        Self {
            algorithm: algorithm.to_string(),
            anchors: BTreeMap::new(),
        }
    }

    /// Legge il file; se manca, è rovinato o è di un altro algoritmo, parte vuoto.
    pub fn load(path: &Path, algorithm: &str) -> Self {
        let loaded = std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str::<Anchors>(&t).ok())
            .filter(|a| a.algorithm == algorithm);
        loaded.unwrap_or_else(|| Anchors::new(algorithm))
    }

    pub fn save(&self, path: &Path) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(t) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path, t);
        }
    }

    pub fn set(&mut self, name: &str, r: &VideoResult) {
        self.anchors.insert(
            name.to_string(),
            Anchor {
                purity_raw: r.purity_raw,
                density_raw: r.density_raw,
                stability_raw: r.stability_raw,
                blockiness: r.blockiness,
                fft_ratio: r.fft_ratio,
                upscale_score: r.upscale_score,
                efficiency: r.efficiency,
            },
        );
    }

    pub fn remove(&mut self, name: &str) -> bool {
        self.anchors.remove(name).is_some()
    }

    pub fn normalized(&self, r: &VideoResult) -> Norm {
        if self.anchors.len() < 2 {
            return Norm {
                purity: r.purity_raw.clamp(0.0, 100.0),
                density: (r.fft_ratio * 400.0 * (1.0 - r.upscale_score * 0.5)).clamp(0.0, 100.0),
                stability: r.stability_raw.clamp(0.0, 100.0),
                has_anchors: false,
            };
        }
        let norm = |value: f64, vals: Vec<f64>| -> f64 {
            let lo = vals.iter().copied().fold(f64::INFINITY, f64::min);
            let hi = vals.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            if hi.partial_cmp(&lo) != Some(std::cmp::Ordering::Greater) {
                return 50.0;
            }
            ((value - lo) / (hi - lo) * 100.0).clamp(0.0, 120.0)
        };
        let col = |f: fn(&Anchor) -> f64| -> Vec<f64> { self.anchors.values().map(f).collect() };
        Norm {
            purity: norm(r.purity_raw, col(|a| a.purity_raw)),
            density: norm(r.density_raw, col(|a| a.density_raw)),
            stability: norm(r.stability_raw, col(|a| a.stability_raw)),
            has_anchors: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn res(p: f64, d: f64, s: f64) -> VideoResult {
        VideoResult {
            purity_raw: p,
            density_raw: d,
            stability_raw: s,
            blockiness: 2.0,
            fft_ratio: 0.1,
            upscale_score: 0.5,
            ..VideoResult::default()
        }
    }

    #[test]
    fn without_anchors_it_uses_fixed_scales() {
        let a = Anchors::new("x");
        let n = a.normalized(&res(76.0, 0.0, 70.0));
        assert!(!n.has_anchors);
        assert_eq!(n.purity, 76.0);
        assert!((n.density - 30.0).abs() < 1e-9);
        assert_eq!(n.stability, 70.0);
        // Una sola ancora non basta.
        let mut a = Anchors::new("x");
        a.set("uno", &res(10.0, 10.0, 10.0));
        assert!(!a.normalized(&res(0.0, 0.0, 0.0)).has_anchors);
    }

    #[test]
    fn with_two_anchors_values_are_placed_between_them() {
        let mut a = Anchors::new("x");
        a.set("brutto", &res(20.0, 10.0, 30.0));
        a.set("bello", &res(80.0, 50.0, 90.0));
        let n = a.normalized(&res(50.0, 30.0, 60.0));
        assert!(n.has_anchors);
        assert!((n.purity - 50.0).abs() < 1e-9);
        assert!((n.density - 50.0).abs() < 1e-9);
        assert!((n.stability - 50.0).abs() < 1e-9);
        // Fuori scala: 0 in basso, 120 in alto.
        assert_eq!(a.normalized(&res(-100.0, 0.0, 0.0)).purity, 0.0);
        assert_eq!(a.normalized(&res(1000.0, 0.0, 0.0)).purity, 120.0);
    }

    #[test]
    fn equal_anchors_give_the_neutral_value() {
        let mut a = Anchors::new("x");
        a.set("a", &res(40.0, 40.0, 40.0));
        a.set("b", &res(40.0, 40.0, 40.0));
        assert_eq!(a.normalized(&res(99.0, 1.0, 5.0)).purity, 50.0);
    }

    #[test]
    fn save_load_and_algorithm_change() {
        let dir = std::env::temp_dir().join(format!("fd_anc_{}", std::process::id()));
        let p = dir.join("a.json");
        let mut a = Anchors::new("v1");
        a.set("x", &res(1.0, 2.0, 3.0));
        a.save(&p);
        assert_eq!(Anchors::load(&p, "v1"), a);
        assert!(
            Anchors::load(&p, "v2").anchors.is_empty(),
            "altro algoritmo: scartate"
        );
        std::fs::write(&p, "{rotto").unwrap();
        assert!(Anchors::load(&p, "v1").anchors.is_empty());
        assert!(Anchors::load(&dir.join("manca.json"), "v1")
            .anchors
            .is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn remove_reports_whether_something_was_there() {
        let mut a = Anchors::new("x");
        a.set("x", &res(1.0, 1.0, 1.0));
        assert!(a.remove("x"));
        assert!(!a.remove("x"));
    }
}
