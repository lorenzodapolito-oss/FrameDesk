//! Le tre misure di qualità di un fotogramma/video: purezza, densità, stabilità.
//! Stesse formule del programma Python (Video Quality Analyzer PRO v6.2).

use crate::config::Config;
use crate::imgproc::{self, Gray};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Purity {
    pub blockiness: f64,
    pub blockiness_spacing: Option<usize>,
    pub ringing: f64,
    pub purity_raw: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Density {
    pub fft_ratio: f64,
    pub upscale_score: f64,
    pub density_raw: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Stability {
    pub flicker: f64,
    pub freeze_ratio: f64,
    pub dead_seconds: f64,
    pub bpp: f64,
    pub efficiency: f64,
    pub stability_raw: f64,
}

pub fn round_to(v: f64, decimals: i32) -> f64 {
    let k = 10f64.powi(decimals);
    let r = (v * k).round() / k;
    if r.is_finite() {
        r
    } else {
        0.0
    }
}

/// Blocchettoni di compressione (distanza regolare dei bordi) e aloni attorno ai contorni.
pub fn purity(gray: &Gray, cfg: &Config) -> Purity {
    let (w, h) = (gray.w, gray.h);
    let (gx, gy) = imgproc::sobel(gray);
    let proj_x: Vec<f64> = (0..w)
        .map(|x| {
            (0..h)
                .map(|y| gx.get(y * w + x).copied().unwrap_or(0.0).abs())
                .sum::<f64>()
                / h.max(1) as f64
        })
        .collect();
    let proj_y: Vec<f64> = (0..h)
        .map(|y| {
            (0..w)
                .map(|x| gy.get(y * w + x).copied().unwrap_or(0.0).abs())
                .sum::<f64>()
                / w.max(1) as f64
        })
        .collect();

    // Si toglie l'andamento lento del profilo (media mobile): resta solo la parte che si ripete
    // ogni `spacing` pixel, cioè la griglia dei blocchi. Senza questo, qualunque immagine naturale
    // dava una correlazione altissima e la purezza finiva sempre a zero.
    let detrend = |p: &[f64], s: usize| -> Vec<f64> {
        let half = s.max(1);
        (0..p.len())
            .map(|i| {
                let lo = i.saturating_sub(half);
                let hi = (i + half + 1).min(p.len());
                let win = p.get(lo..hi).unwrap_or(&[]);
                p.get(i).copied().unwrap_or(0.0) - imgproc::mean(win)
            })
            .collect()
    };
    let corr = |p: &[f64], s: usize| -> f64 {
        if p.len() > s * 2 {
            let d = detrend(p, s);
            let a = d.get(..d.len() - s).unwrap_or(&[]);
            let b = d.get(s..).unwrap_or(&[]);
            imgproc::pearson(a, b).max(0.0)
        } else {
            0.0
        }
    };

    let mut best_score = 0.0_f64;
    let mut best_spacing = None;
    for &spacing in &cfg.blockiness_spacings {
        if spacing >= w.min(h) / 2 {
            continue;
        }
        let combined = ((corr(&proj_x, spacing) + corr(&proj_y, spacing)) / 2.0).clamp(0.0, 1.0);
        if combined > best_score {
            best_score = combined;
            best_spacing = Some(spacing);
        }
        if combined > 0.80 {
            break;
        }
    }

    let blockiness_raw = best_score * 100.0;
    let edges = imgproc::canny(gray, 50.0, 150.0);
    let ringing = if edges.iter().filter(|e| **e).count() > 100 {
        let dil = imgproc::dilate5(&edges, w, h);
        let ring: Vec<f64> = (0..w * h)
            .filter(|i| {
                dil.get(*i).copied().unwrap_or(false) && !edges.get(*i).copied().unwrap_or(false)
            })
            .map(|i| f64::from(gray.d.get(i).copied().unwrap_or(0)))
            .collect();
        imgproc::std(&ring)
    } else {
        0.0
    };
    let purity_raw = (100.0 - blockiness_raw * 1.2 - ringing * 0.5).max(0.0);
    Purity {
        blockiness: round_to(blockiness_raw, 2),
        blockiness_spacing: best_spacing,
        ringing: round_to(ringing, 2),
        purity_raw: round_to(purity_raw, 2),
    }
}

/// Quanto dettaglio fine c'è (energia alle alte frequenze) e se l'immagine sembra ingrandita.
pub fn density(gray: &Gray, cfg: &Config) -> Density {
    let (h, w) = (gray.h, gray.w);
    let fft_src = if h >= 256 && w >= 256 {
        gray.crop_center(256, 256).unwrap_or_else(|| gray.clone())
    } else {
        gray.clone()
    };
    let mag = imgproc::fft_magnitude_shifted(&fft_src);
    let (fh, fw) = (fft_src.h, fft_src.w);
    let (ch, cw) = ((fh / 2) as i64, (fw / 2) as i64);
    let r_low = (fh.min(fw) / 4) as i64;
    let r_high = ((fh.min(fw) / 2) as f64 * 0.8) as i64;
    let (mut low_e, mut high_e) = (1e-6, 0.0);
    for y in 0..fh {
        for x in 0..fw {
            let d2 = (x as i64 - cw).pow(2) + (y as i64 - ch).pow(2);
            let m = mag.get(y * fw + x).copied().unwrap_or(0.0);
            if d2 <= r_low * r_low {
                low_e += m;
            }
            if d2 >= r_high * r_high {
                high_e += m;
            }
        }
    }
    let fft_ratio = high_e / low_e;

    // Se rimpicciolendo e reingrandendo l'immagine non cambia quasi nulla, era già ingrandita.
    let mut upscale = 0.0;
    if h.min(w) >= 128 {
        let roi_size = 256.min(h).min(w);
        if let Some(roi) = gray.crop_center(roi_size, roi_size) {
            let small = roi.resize_area((roi.w / 2).max(1), (roi.h / 2).max(1));
            let recreated = small.and_then(|s| s.resize_cubic(roi.w, roi.h));
            if let Some(rec) = recreated {
                let n = roi.d.len().max(1) as f64;
                let mse = roi
                    .d
                    .iter()
                    .zip(&rec.d)
                    .map(|(a, b)| {
                        let d = f64::from(*a) - f64::from(*b);
                        d * d
                    })
                    .sum::<f64>()
                    / n;
                upscale = (1.0 - mse / 1500.0).clamp(0.0, 1.0);
            }
        }
    }
    let density_raw =
        (fft_ratio * 500.0 * (1.0 - upscale * cfg.upscale_ssim_weight)).clamp(0.0, 100.0);
    Density {
        fft_ratio: round_to(fft_ratio, 5),
        upscale_score: round_to(upscale, 3),
        density_raw: round_to(density_raw, 2),
    }
}

/// Informazioni del file che servono alla stabilità.
#[derive(Clone, Copy, Debug)]
pub struct StabilityInput {
    pub fps: f64,
    pub file_size: u64,
    pub width: usize,
    pub height: usize,
    pub duration: f64,
}

/// Sfarfallio, fermi immagine e efficienza di compressione (bit per pixel).
pub fn stability(frames: &[Gray], indices: &[u64], inp: StabilityInput, cfg: &Config) -> Stability {
    if frames.len() < 2 {
        return Stability {
            efficiency: 50.0,
            stability_raw: 50.0,
            ..Stability::default()
        };
    }
    let diffs: Vec<f64> = frames
        .windows(2)
        .map(|p| match p {
            [a, b] if a.w == b.w && a.h == b.h => {
                let s: f64 =
                    a.d.iter()
                        .zip(&b.d)
                        .map(|(x, y)| f64::from(x.abs_diff(*y)))
                        .sum();
                s / a.d.len().max(1) as f64
            }
            _ => 0.0,
        })
        .collect();
    let flicker = if diffs.len() > 1 {
        imgproc::std(&diffs)
    } else {
        0.0
    };
    let thr = cfg.dead_segment_threshold;
    let freeze_ratio =
        diffs.iter().filter(|d| **d < thr).count() as f64 / diffs.len().max(1) as f64;

    let fallback = if inp.duration > 0.0 {
        inp.duration / frames.len() as f64
    } else {
        1.0
    };
    let intervals: Vec<f64> = indices
        .windows(2)
        .map(|p| {
            let gap = if inp.fps > 0.0 {
                (p[1] as f64 - p[0] as f64) / inp.fps
            } else {
                fallback
            };
            if gap > 0.0 {
                gap
            } else {
                fallback
            }
        })
        .collect();
    let (mut run, mut dead) = (0.0, 0.0);
    for (d, iv) in diffs.iter().zip(&intervals) {
        if *d < thr {
            run += iv;
        } else {
            if run >= cfg.dead_segment_min_duration_sec {
                dead += run;
            }
            run = 0.0;
        }
    }
    if run >= cfg.dead_segment_min_duration_sec {
        dead += run;
    }

    let (bpp, efficiency) = if inp.width > 0 && inp.height > 0 && inp.duration > 0.0 {
        let bits = inp.file_size as f64 * 8.0;
        let fps = if inp.fps > 0.0 { inp.fps } else { 25.0 };
        let pixels = inp.width as f64 * inp.height as f64 * inp.duration * fps;
        let bpp = if pixels > 0.0 { bits / pixels } else { 0.0 };
        (bpp, ((bpp - 0.02) / 0.13 * 100.0).clamp(0.0, 100.0))
    } else {
        (0.0, 50.0)
    };
    let stability_raw =
        (100.0 - flicker * 2.0 - freeze_ratio * 50.0 + efficiency * 0.2).clamp(0.0, 100.0);
    Stability {
        flicker: round_to(flicker, 2),
        freeze_ratio: round_to(freeze_ratio, 3),
        dead_seconds: round_to(dead, 2),
        bpp: round_to(bpp, 5),
        efficiency: round_to(efficiency, 2),
        stability_raw: round_to(stability_raw, 2),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noise(w: usize, h: usize, seed: u32) -> Gray {
        let mut s = seed;
        let d = (0..w * h)
            .map(|_| {
                s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (s >> 24) as u8
            })
            .collect();
        Gray::new(w, h, d).unwrap()
    }

    fn smooth(w: usize, h: usize) -> Gray {
        let d = (0..w * h).map(|i| ((i % w) * 255 / w) as u8).collect();
        Gray::new(w, h, d).unwrap()
    }

    /// Griglia di blocchi 8x8 piatti: la tipica "mattonella" della compressione forte.
    fn blocky(w: usize, h: usize) -> Gray {
        let d = (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                let b = (x / 8 + y / 8) as u32;
                (((b * 97) % 200) + 20) as u8
            })
            .collect();
        Gray::new(w, h, d).unwrap()
    }

    #[test]
    fn blocky_image_has_more_blockiness_than_a_smooth_one() {
        let cfg = Config::default();
        let b = purity(&blocky(256, 144), &cfg);
        let s = purity(&smooth(256, 144), &cfg);
        assert!(b.blockiness > s.blockiness, "{b:?} / {s:?}");
        assert!(b.blockiness_spacing.is_some_and(|k| k % 8 == 0), "{b:?}");
        assert!(b.purity_raw < s.purity_raw);
        assert!((0.0..=100.0).contains(&b.purity_raw));
    }

    #[test]
    fn natural_looking_images_do_not_all_score_zero() {
        let cfg = Config::default();
        for img in [smooth(256, 144), noise(256, 144, 3)] {
            let p = purity(&img, &cfg);
            assert!(p.purity_raw > 20.0, "{p:?}");
        }
    }

    #[test]
    fn purity_of_tiny_images_does_not_panic() {
        let cfg = Config::default();
        for (w, h) in [(1, 1), (3, 2), (10, 10), (17, 5)] {
            let p = purity(&noise(w, h, 7), &cfg);
            assert!(p.purity_raw.is_finite());
        }
    }

    #[test]
    fn noise_has_more_detail_than_a_smooth_gradient() {
        let cfg = Config::default();
        let n = density(&noise(256, 256, 1), &cfg);
        let s = density(&smooth(256, 256), &cfg);
        assert!(n.fft_ratio > s.fft_ratio * 10.0, "{n:?} / {s:?}");
        assert!(n.density_raw > s.density_raw);
        assert!((0.0..=100.0).contains(&n.density_raw));
    }

    #[test]
    fn an_upscaled_image_is_detected() {
        let cfg = Config::default();
        // Rumore a metà risoluzione, poi ingrandito 2x: sembra "falso HD".
        let small = noise(128, 128, 3);
        let big = small.resize_cubic(256, 256).unwrap();
        let real = noise(256, 256, 3);
        let up = density(&big, &cfg);
        let nat = density(&real, &cfg);
        assert!(up.upscale_score > 0.5, "{up:?}");
        assert!(
            up.upscale_score > nat.upscale_score + 0.3,
            "{up:?} / {nat:?}"
        );
        assert!(up.fft_ratio < nat.fft_ratio);
    }

    #[test]
    fn density_of_small_images_skips_the_upscale_test() {
        let d = density(&noise(64, 64, 5), &Config::default());
        assert_eq!(d.upscale_score, 0.0);
        let d = density(&noise(1, 1, 5), &Config::default());
        assert!(d.density_raw.is_finite());
    }

    fn inp(duration: f64) -> StabilityInput {
        StabilityInput {
            fps: 25.0,
            file_size: 5_000_000,
            width: 640,
            height: 360,
            duration,
        }
    }

    #[test]
    fn stability_needs_two_frames() {
        let s = stability(&[noise(8, 8, 1)], &[0], inp(10.0), &Config::default());
        assert_eq!((s.efficiency, s.stability_raw), (50.0, 50.0));
        let s = stability(&[], &[], inp(10.0), &Config::default());
        assert_eq!(s.stability_raw, 50.0);
    }

    #[test]
    fn frozen_video_counts_dead_seconds() {
        let f = noise(32, 32, 9);
        let frames = vec![
            f.clone(),
            f.clone(),
            f.clone(),
            f.clone(),
            noise(32, 32, 10),
        ];
        let idx = [0, 50, 100, 150, 200];
        let s = stability(&frames, &idx, inp(8.0), &Config::default());
        assert_eq!(s.freeze_ratio, 0.75);
        // Tre intervalli fermi da 2 secondi ciascuno.
        assert_eq!(s.dead_seconds, 6.0);
        assert!(s.stability_raw < 100.0);
    }

    #[test]
    fn short_freezes_are_not_dead_seconds() {
        let f = noise(16, 16, 9);
        let frames = vec![f.clone(), f.clone(), noise(16, 16, 2)];
        // Intervalli di 0,4 s: sotto la durata minima di 1 s.
        let s = stability(&frames, &[0, 10, 20], inp(1.0), &Config::default());
        assert_eq!(s.dead_seconds, 0.0);
        assert!(s.freeze_ratio > 0.0);
    }

    #[test]
    fn efficiency_follows_bits_per_pixel() {
        let frames = vec![noise(16, 16, 1), noise(16, 16, 2)];
        let low = StabilityInput {
            file_size: 100_000,
            ..inp(10.0)
        };
        let high = StabilityInput {
            file_size: 80_000_000,
            ..inp(10.0)
        };
        let a = stability(&frames, &[0, 1], low, &Config::default());
        let b = stability(&frames, &[0, 1], high, &Config::default());
        assert!(a.efficiency < b.efficiency);
        assert_eq!(b.efficiency, 100.0);
        // Dimensioni a zero: valore neutro, niente divisioni per zero.
        let z = StabilityInput {
            width: 0,
            ..inp(10.0)
        };
        assert_eq!(
            stability(&frames, &[0, 1], z, &Config::default()).efficiency,
            50.0
        );
    }

    #[test]
    fn mismatched_frame_sizes_do_not_panic() {
        let frames = vec![noise(8, 8, 1), noise(9, 9, 2), noise(8, 8, 3)];
        let s = stability(&frames, &[0, 1, 2], inp(3.0), &Config::default());
        assert!(s.stability_raw.is_finite());
    }

    #[test]
    fn rounding_helper_handles_non_finite_values() {
        assert_eq!(round_to(1.23456, 2), 1.23);
        assert_eq!(round_to(f64::NAN, 2), 0.0);
        assert_eq!(round_to(f64::INFINITY, 2), 0.0);
    }
}
