//! L'analisi di un video e di una lista di video.
//!
//! Per ogni video: ffprobe dà le informazioni, ffmpeg estrae alcuni fotogrammi sparsi (più i
//! punti di cambio scena), su ciascuno si misurano purezza e densità, tutti insieme danno la
//! stabilità. Il voto finale mescola le tre misure con i pesi delle impostazioni.

use crate::anchors::Anchors;
use crate::cache::Cache;
use crate::config::Config;
use crate::ffmpeg::{proc_size, Probe, Tools};
use crate::imgproc::{self, Gray, Rgb};
use crate::metrics::{self, round_to, StabilityInput};
use crate::model::{duration_text, human_size, Grade, Thumb, Thumbs, TimelinePoint, VideoResult};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Lato lungo massimo a cui si riduce un fotogramma prima di misurarlo.
const MAX_DIM: usize = 640;

/// Quanti fotogrammi analizzare in base alla durata (se `adaptive_frames`).
pub fn frame_count(cfg: &Config, duration: f64, total_frames: u64) -> usize {
    let n = if cfg.adaptive_frames {
        if duration < 60.0 {
            40
        } else if duration < 300.0 {
            25
        } else if duration < 1800.0 {
            18
        } else {
            12
        }
    } else {
        cfg.frame_sampling_base
    };
    n.min(usize::try_from(total_frames).unwrap_or(usize::MAX))
}

/// Indici dei fotogrammi da analizzare: sparsi in modo uniforme, i punti chiave
/// (inizio, quarti, fine) e i cambi di scena.
pub fn frame_indices(
    cfg: &Config,
    total_frames: u64,
    duration: f64,
    scene_changes: &[u64],
) -> Vec<u64> {
    if total_frames == 0 {
        return Vec::new();
    }
    let n = frame_count(cfg, duration, total_frames).max(1) as u64;
    let step = (total_frames / n).max(1);
    let mut idx: Vec<u64> = (0..total_frames)
        .step_by(step as usize)
        .take(n as usize)
        .collect();
    for cp in [
        0,
        total_frames / 4,
        total_frames / 2,
        3 * total_frames / 4,
        total_frames - 1,
    ] {
        if cp < total_frames && !idx.contains(&cp) {
            idx.push(cp);
        }
    }
    let mut sc: Vec<u64> = scene_changes.to_vec();
    if cfg.scene_detection {
        if sc.len() > 10 {
            let s = (sc.len() / 10).max(1);
            sc = sc.into_iter().step_by(s).collect();
        }
        idx.extend(sc);
    }
    idx.sort_unstable();
    idx.dedup();
    idx
}

/// Istogramma di colore 8x8x8 con norma L2 uguale a 1 (come `calcHist` + `normalize`).
pub fn color_hist(rgb: &Rgb) -> Vec<f64> {
    let mut h = vec![0.0_f64; 512];
    for p in rgb.d.chunks_exact(3) {
        let i = usize::from(p[2] >> 5) * 64 + usize::from(p[1] >> 5) * 8 + usize::from(p[0] >> 5);
        if let Some(b) = h.get_mut(i) {
            *b += 1.0;
        }
    }
    let norm = h.iter().map(|v| v * v).sum::<f64>().sqrt();
    if norm > 0.0 {
        for v in &mut h {
            *v /= norm;
        }
    }
    h
}

/// Correlazione tra due istogrammi; se uno dei due è costante vale 1 (come OpenCV).
pub fn hist_correl(a: &[f64], b: &[f64]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return 1.0;
    }
    let (ma, mb) = (imgproc::mean(a), imgproc::mean(b));
    let (mut sab, mut saa, mut sbb) = (0.0, 0.0, 0.0);
    for (x, y) in a.iter().zip(b) {
        sab += (x - ma) * (y - mb);
        saa += (x - ma) * (x - ma);
        sbb += (y - mb) * (y - mb);
    }
    let den = (saa * sbb).sqrt();
    if den > f64::EPSILON {
        sab / den
    } else {
        1.0
    }
}

/// Cambi di scena: i fotogrammi il cui istogramma assomiglia poco al precedente.
pub fn scene_changes(hists: &[(u64, Vec<f64>)]) -> Vec<u64> {
    hists
        .windows(2)
        .filter(|p| hist_correl(&p[0].1, &p[1].1) < 0.3)
        .map(|p| p[1].0)
        .collect()
}

/// Un fotogramma pronto per le misure.
struct Prepared {
    idx: u64,
    gray: Gray,
    lum: f64,
    thumb: Option<Rgb>,
}

/// Ritaglio centrale (se attivo) e grigio. Immagini troppo piccole si scartano.
fn prepare(idx: u64, rgb: &Rgb, cfg: &Config) -> Option<Prepared> {
    if rgb.h < 10 || rgb.w < 10 {
        return None;
    }
    let mut gray = rgb.to_gray();
    if cfg.smart_crop && gray.w.min(gray.h) > 64 {
        let side = gray.w.min(gray.h);
        let m = (side as f64 * (1.0 - cfg.crop_ratio) / 2.0) as usize;
        if m > 0 && m * 2 + 10 < side {
            gray = gray.crop_margin(m)?;
        }
    }
    let thumb = rgb.resize_area(160, 90);
    let lum = gray.mean();
    Some(Prepared {
        idx,
        gray,
        lum,
        thumb,
    })
}

struct Scored {
    idx: u64,
    lum: f64,
    purity: metrics::Purity,
    density: metrics::Density,
}

/// Esegue `f` su ogni elemento con al massimo `jobs` thread; l'ordine del risultato è quello dell'ingresso.
fn par_map<T: Sync, R: Send>(items: &[T], jobs: usize, f: &(dyn Fn(&T) -> R + Sync)) -> Vec<R> {
    let jobs = jobs.clamp(1, items.len().max(1));
    if jobs == 1 {
        return items.iter().map(f).collect();
    }
    let next = AtomicUsize::new(0);
    let slots: Mutex<Vec<Option<R>>> = Mutex::new((0..items.len()).map(|_| None).collect());
    std::thread::scope(|s| {
        for _ in 0..jobs {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::SeqCst);
                let Some(item) = items.get(i) else { break };
                let r = f(item);
                if let Ok(mut g) = slots.lock() {
                    if let Some(slot) = g.get_mut(i) {
                        *slot = Some(r);
                    }
                }
            });
        }
    });
    slots
        .into_inner()
        .unwrap_or_default()
        .into_iter()
        .flatten()
        .collect()
}

pub struct Analyzer<'a> {
    pub tools: &'a Tools,
    pub cfg: &'a Config,
    pub cancel: &'a AtomicBool,
    /// Thread per i fotogrammi di un solo video.
    pub frame_jobs: usize,
}

impl Analyzer<'_> {
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    fn times(&self, probe: &Probe, indices: &[u64]) -> Vec<(u64, f64)> {
        let last = (probe.duration - 0.1).max(0.0);
        indices
            .iter()
            .map(|i| (*i, (*i as f64 / probe.fps.max(1.0)).min(last)))
            .collect()
    }

    /// Cambi di scena su fotogrammi minuscoli (80x45) sparsi su tutta la durata.
    fn detect_scenes(&self, path: &Path, probe: &Probe) -> Vec<u64> {
        if !self.cfg.scene_detection || probe.total_frames == 0 {
            return Vec::new();
        }
        let samples = (self.cfg.scene_max_samples as u64)
            .min(probe.total_frames)
            .max(1);
        let step = (probe.total_frames / samples).max(1);
        let idx: Vec<u64> = (0..probe.total_frames).step_by(step as usize).collect();
        let times = self.times(probe, &idx);
        let hists: Vec<Option<(u64, Vec<f64>)>> = par_map(&times, self.frame_jobs, &|(i, t)| {
            if self.cancelled() {
                return None;
            }
            self.tools
                .frame(path, *t, 80, 45)
                .ok()
                .map(|f| (*i, color_hist(&f)))
        });
        let hists: Vec<(u64, Vec<f64>)> = hists.into_iter().flatten().collect();
        scene_changes(&hists)
    }

    /// Analisi grezza di un video (senza normalizzazione né voto: lo fa `finalize`).
    pub fn analyze(&self, path: &Path) -> VideoResult {
        let p = path.to_string_lossy().into_owned();
        if !path.is_file() {
            return VideoResult::failed(&p, "File non trovato");
        }
        let started = Instant::now();
        let probe = self.tools.probe(path);
        if probe.width == 0 || probe.height == 0 || probe.total_frames == 0 {
            return VideoResult::failed(&p, "Video non apribile");
        }
        let file_size = std::fs::metadata(path).map_or(0, |m| m.len());
        let scenes = self.detect_scenes(path, &probe);
        if self.cancelled() {
            return VideoResult::failed(&p, "Annullato");
        }
        let indices = frame_indices(self.cfg, probe.total_frames, probe.duration, &scenes);
        let (pw, ph) = proc_size(probe.width, probe.height, MAX_DIM);
        let times = self.times(&probe, &indices);
        let deadline = Duration::from_secs(self.cfg.timeout_seconds);

        let prepared: Vec<Option<Prepared>> = par_map(&times, self.frame_jobs, &|(i, t)| {
            if self.cancelled() || started.elapsed() > deadline {
                return None;
            }
            let rgb = self.tools.frame(path, *t, pw, ph).ok()?;
            prepare(*i, &rgb, self.cfg)
        });
        if self.cancelled() {
            return VideoResult::failed(&p, "Annullato");
        }
        if started.elapsed() > deadline {
            return VideoResult::failed(&p, "Tempo scaduto");
        }
        let all: Vec<Prepared> = prepared.into_iter().flatten().collect();
        // Prima si scartano i fotogrammi neri; se ne restano troppo pochi (video scuro) si tengono tutti.
        let bright: Vec<&Prepared> = all
            .iter()
            .filter(|f| f.lum >= self.cfg.min_frame_luminance)
            .collect();
        let chosen: Vec<&Prepared> = if bright.len() >= self.cfg.min_frames_required {
            bright
        } else {
            all.iter().collect()
        };
        if chosen.is_empty() {
            return VideoResult::failed(&p, "Nessun frame analizzabile");
        }

        let scored: Vec<Scored> = par_map(&chosen, self.frame_jobs, &|f| Scored {
            idx: f.idx,
            lum: f.lum,
            purity: metrics::purity(&f.gray, self.cfg),
            density: metrics::density(&f.gray, self.cfg),
        });

        let mut thumbs = Thumbs::default();
        let (mut best, mut worst) = (-1.0_f64, 999.0_f64);
        for (f, s) in chosen.iter().zip(&scored) {
            let score = s.purity.purity_raw * 0.5 + s.density.density_raw * 0.5;
            let mk = || {
                f.thumb.as_ref().map(|t| Thumb {
                    frame_idx: f.idx,
                    score,
                    purity: s.purity.purity_raw,
                    density: s.density.density_raw,
                    upscale: s.density.upscale_score,
                    w: t.w,
                    h: t.h,
                    rgb: t.d.clone(),
                })
            };
            if score > best {
                best = score;
                thumbs.best = mk();
            }
            if score < worst {
                worst = score;
                thumbs.worst = mk();
            }
            let better = thumbs
                .upscale_suspect
                .as_ref()
                .is_none_or(|t| s.density.upscale_score > t.upscale);
            if s.density.upscale_score > 0.6 && better {
                thumbs.upscale_suspect = mk();
            }
        }

        let grays: Vec<Gray> = chosen
            .iter()
            .map(|f| {
                if f.gray.w > 320 {
                    f.gray.half().unwrap_or_else(|| f.gray.clone())
                } else {
                    f.gray.clone()
                }
            })
            .collect();
        let idx: Vec<u64> = scored.iter().map(|s| s.idx).collect();
        let stab = metrics::stability(
            &grays,
            &idx,
            StabilityInput {
                fps: probe.fps,
                file_size,
                width: probe.width,
                height: probe.height,
                duration: probe.duration,
            },
            self.cfg,
        );

        let avg =
            |f: &dyn Fn(&Scored) -> f64| imgproc::mean(&scored.iter().map(f).collect::<Vec<_>>());
        VideoResult {
            name: path
                .file_name()
                .map_or_else(|| p.clone(), |n| n.to_string_lossy().into_owned()),
            path: p,
            size_text: if file_size > 0 {
                human_size(file_size)
            } else {
                "?".into()
            },
            duration_text: duration_text(probe.duration),
            total_frames: probe.total_frames,
            analyzed_frames: scored.len(),
            codec: probe.codec,
            width: probe.width,
            height: probe.height,
            fps: round_to(probe.fps, 2),
            color_space: probe.color_space,
            color_transfer: probe.color_transfer,
            pix_fmt: probe.pix_fmt,
            bits: probe.bits,
            is_hdr: probe.is_hdr,
            hdr_type: probe.hdr_type,
            purity_raw: avg(&|s| s.purity.purity_raw),
            density_raw: avg(&|s| s.density.density_raw),
            stability_raw: stab.stability_raw,
            blockiness: avg(&|s| s.purity.blockiness),
            fft_ratio: avg(&|s| s.density.fft_ratio),
            upscale_score: avg(&|s| s.density.upscale_score),
            flicker: stab.flicker,
            freeze_ratio: stab.freeze_ratio,
            dead_seconds: stab.dead_seconds,
            efficiency: stab.efficiency,
            bpp: stab.bpp,
            media_lum: round_to(avg(&|s| s.lum), 1),
            timeline: scored
                .iter()
                .map(|s| TimelinePoint {
                    frame_idx: s.idx,
                    purity: s.purity.purity_raw,
                    density: s.density.density_raw,
                    blockiness: s.purity.blockiness,
                    fft: s.density.fft_ratio,
                })
                .collect(),
            duration_sec: probe.duration,
            file_size,
            thumbs,
            ..VideoResult::default()
        }
    }
}

/// Bitrate, posizione normalizzata, punteggio e voto. Per i video con errore tutto resta a zero.
pub fn finalize(r: &mut VideoResult, anchors: &Anchors, cfg: &Config) {
    if r.error.is_some() {
        r.purity_norm = 0.0;
        r.density_norm = 0.0;
        r.stability_norm = 0.0;
        r.score = 0.0;
        r.bitrate_mbps = 0.0;
        r.grade = None;
        return;
    }
    r.bitrate_mbps = if r.duration_sec > 0.0 && r.file_size > 0 {
        round_to(r.file_size as f64 * 8.0 / r.duration_sec / 1_000_000.0, 2)
    } else {
        0.0
    };
    let n = anchors.normalized(r);
    r.has_anchors = n.has_anchors;
    r.purity_norm = round_to(n.purity, 1);
    r.density_norm = round_to(n.density, 1);
    r.stability_norm = round_to(n.stability, 1);
    let raw = n.purity * cfg.weight_purity
        + n.density * cfg.weight_density
        + n.stability * cfg.weight_stability;
    let score = if raw.is_finite() {
        raw.clamp(0.0, 100.0)
    } else {
        0.0
    };
    r.score = round_to(score, 1);
    r.grade = Some(Grade::from_score(score));
}

/// Avanzamento mostrato all'utente.
#[derive(Clone, Debug)]
pub struct Progress {
    pub done: usize,
    pub total: usize,
    pub current: String,
}

/// Analizza tutti i video (in parallelo). Il risultato è nello stesso ordine della lista.
/// `on_result` si chiama appena un video è pronto (indice, risultato).
#[allow(clippy::too_many_arguments)]
pub fn analyze_all(
    paths: &[String],
    cfg: &Config,
    tools: &Tools,
    anchors: &Anchors,
    cache: &Mutex<Cache>,
    cancel: &AtomicBool,
    on_progress: &(dyn Fn(Progress) + Sync),
    on_result: &(dyn Fn(usize, &VideoResult) + Sync),
) -> Vec<VideoResult> {
    let total = paths.len();
    if total == 0 {
        return Vec::new();
    }
    let video_jobs = cfg.threads().min(total).max(1);
    let frame_jobs = (cfg.threads() / video_jobs).clamp(1, 4);
    let algorithm = cfg.algorithm_key();
    let done = AtomicUsize::new(0);
    let jobs: Vec<(usize, &String)> = paths.iter().enumerate().collect();

    let results = par_map(&jobs, video_jobs, &|(i, p)| {
        let path = Path::new(p.as_str());
        let cached = if cfg.cache_enabled {
            cache
                .lock()
                .ok()
                .and_then(|c| c.get(path, &algorithm, cfg.cache_ttl_hours))
        } else {
            None
        };
        let mut r = match cached {
            Some(r) => r,
            None if cancel.load(Ordering::SeqCst) => VideoResult::failed(p, "Annullato"),
            None => Analyzer {
                tools,
                cfg,
                cancel,
                frame_jobs,
            }
            .analyze(path),
        };
        finalize(&mut r, anchors, cfg);
        if cfg.cache_enabled && !r.from_cache {
            if let Ok(mut c) = cache.lock() {
                c.set(path, &algorithm, &r);
            }
        }
        let n = done.fetch_add(1, Ordering::SeqCst) + 1;
        on_progress(Progress {
            done: n,
            total,
            current: r.name.clone(),
        });
        on_result(*i, &r);
        r
    });
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Config {
        Config::default()
    }

    #[test]
    fn frame_count_depends_on_duration() {
        let c = cfg();
        assert_eq!(frame_count(&c, 30.0, 10_000), 40);
        assert_eq!(frame_count(&c, 120.0, 10_000), 25);
        assert_eq!(frame_count(&c, 600.0, 100_000), 18);
        assert_eq!(frame_count(&c, 7200.0, 100_000), 12);
        assert_eq!(
            frame_count(&c, 30.0, 7),
            7,
            "mai più dei fotogrammi che ci sono"
        );
        let fixed = Config {
            adaptive_frames: false,
            frame_sampling_base: 60,
            ..cfg()
        };
        assert_eq!(frame_count(&fixed, 7200.0, 100_000), 60);
    }

    #[test]
    fn indices_are_sorted_unique_and_include_key_points() {
        let c = Config {
            scene_detection: false,
            ..cfg()
        };
        let idx = frame_indices(&c, 1000, 40.0, &[]);
        assert!(idx.windows(2).all(|w| w[0] < w[1]));
        for k in [0, 250, 500, 750, 999] {
            assert!(idx.contains(&k), "{k}");
        }
        assert!(idx.iter().all(|i| *i < 1000));
        assert!(frame_indices(&c, 0, 0.0, &[]).is_empty());
        assert_eq!(frame_indices(&c, 1, 0.04, &[]), vec![0]);
    }

    #[test]
    fn scene_changes_are_added_and_thinned() {
        let c = cfg();
        let few = frame_indices(&c, 1000, 40.0, &[333, 666]);
        assert!(few.contains(&333) && few.contains(&666));
        let many: Vec<u64> = (1..=100).map(|i| i * 9).collect();
        let idx = frame_indices(&c, 1000, 40.0, &many);
        let from_scenes = many.iter().filter(|m| idx.contains(m)).count();
        assert!(
            from_scenes <= 14,
            "{from_scenes}: più di 10 cambi si diradano"
        );
        let off = Config {
            scene_detection: false,
            ..cfg()
        };
        assert!(!frame_indices(&off, 1000, 40.0, &[333]).contains(&333));
    }

    fn solid(r: u8, g: u8, b: u8) -> Rgb {
        Rgb::new(8, 8, [r, g, b].repeat(64)).unwrap()
    }

    #[test]
    fn histograms_detect_cuts() {
        let red = color_hist(&solid(250, 10, 10));
        let red2 = color_hist(&solid(252, 12, 12));
        let blue = color_hist(&solid(10, 10, 250));
        let n: f64 = red.iter().map(|v| v * v).sum();
        assert!((n - 1.0).abs() < 1e-9);
        assert!(hist_correl(&red, &red2) > 0.99);
        assert!(hist_correl(&red, &blue) < 0.3);
        let h = vec![(0, red.clone()), (1, red2), (2, blue), (3, red)];
        assert_eq!(scene_changes(&h), vec![2, 3]);
    }

    #[test]
    fn degenerate_histograms_count_as_equal() {
        let flat = vec![0.5; 512];
        assert_eq!(hist_correl(&flat, &flat), 1.0);
        assert_eq!(hist_correl(&[], &[]), 1.0);
        assert_eq!(hist_correl(&[1.0], &[1.0, 2.0]), 1.0);
        assert!(scene_changes(&[]).is_empty());
        assert!(scene_changes(&[(0, flat)]).is_empty());
    }

    #[test]
    fn prepare_crops_the_center_and_rejects_tiny_frames() {
        let c = cfg();
        let big = Rgb::new(200, 100, vec![120; 200 * 100 * 3]).unwrap();
        let p = prepare(7, &big, &c).unwrap();
        // lato corto 100, ritaglio 50%: toglie 25 pixel per lato.
        assert_eq!((p.gray.w, p.gray.h), (150, 50));
        assert_eq!(p.idx, 7);
        assert!((p.lum - 120.0).abs() < 1.0);
        assert_eq!(p.thumb.as_ref().map(|t| (t.w, t.h)), Some((160, 90)));
        let nocrop = Config {
            smart_crop: false,
            ..cfg()
        };
        assert_eq!(prepare(0, &big, &nocrop).unwrap().gray.w, 200);
        assert!(prepare(0, &Rgb::new(5, 5, vec![0; 75]).unwrap(), &c).is_none());
        // Una immagine piccola (lato <= 64) non si ritaglia.
        let small = Rgb::new(60, 40, vec![9; 60 * 40 * 3]).unwrap();
        assert_eq!(prepare(0, &small, &c).unwrap().gray.w, 60);
    }

    #[test]
    fn par_map_keeps_the_order_for_any_thread_count() {
        let v: Vec<u32> = (0..50).collect();
        for jobs in [0, 1, 3, 100] {
            let out = par_map(&v, jobs, &|x| x * 2);
            assert_eq!(
                out,
                (0..50).map(|x| x * 2).collect::<Vec<_>>(),
                "jobs={jobs}"
            );
        }
        assert!(par_map::<u32, u32>(&[], 4, &|x| *x).is_empty());
    }

    fn raw_result() -> VideoResult {
        VideoResult {
            name: "a.mp4".into(),
            duration_sec: 10.0,
            file_size: 12_500_000,
            purity_raw: 88.0,
            blockiness: 1.0,
            fft_ratio: 0.2,
            upscale_score: 0.0,
            stability_raw: 80.0,
            ..VideoResult::default()
        }
    }

    #[test]
    fn finalize_computes_bitrate_score_and_grade() {
        let mut r = raw_result();
        finalize(&mut r, &Anchors::new("x"), &cfg());
        assert_eq!(r.bitrate_mbps, 10.0);
        // purezza 88, densità 80, stabilità 80 → 88*.4 + 80*.35 + 80*.25 = 83,2
        assert!((r.score - 83.2).abs() < 0.11, "{}", r.score);
        assert_eq!(r.grade, Some(Grade::MoltoBuono));
        assert!(!r.has_anchors);
    }

    #[test]
    fn finalize_zeroes_failed_videos_and_survives_bad_numbers() {
        let mut e = VideoResult::failed("x.mp4", "boom");
        e.score = 99.0;
        finalize(&mut e, &Anchors::new("x"), &cfg());
        assert_eq!((e.score, e.bitrate_mbps), (0.0, 0.0));
        assert!(e.grade.is_none());
        let mut r = raw_result();
        r.stability_raw = f64::NAN;
        r.duration_sec = 0.0;
        finalize(&mut r, &Anchors::new("x"), &cfg());
        assert!(r.score.is_finite() && (0.0..=100.0).contains(&r.score));
        assert_eq!(r.bitrate_mbps, 0.0);
    }

    #[test]
    fn missing_files_and_missing_tools_give_errors_not_panics() {
        let tools = Tools {
            ffmpeg: "/non/esiste/ffmpeg".into(),
            ffprobe: "/non/esiste/ffprobe".into(),
        };
        let cancel = AtomicBool::new(false);
        let a = Analyzer {
            tools: &tools,
            cfg: &cfg(),
            cancel: &cancel,
            frame_jobs: 2,
        };
        assert_eq!(
            a.analyze(Path::new("/non/esiste.mp4")).error.as_deref(),
            Some("File non trovato")
        );
        let f = std::env::temp_dir().join(format!("fd_fake_{}.mp4", std::process::id()));
        std::fs::write(&f, b"non e' un video").unwrap();
        assert_eq!(a.analyze(&f).error.as_deref(), Some("Video non apribile"));
        let _ = std::fs::remove_file(&f);
    }

    #[test]
    fn analyze_all_keeps_order_and_reports_progress() {
        let tools = Tools {
            ffmpeg: "/x/ffmpeg".into(),
            ffprobe: "/x/ffprobe".into(),
        };
        let paths: Vec<String> = (0..5).map(|i| format!("/non/esiste/{i}.mp4")).collect();
        let cancel = AtomicBool::new(false);
        let seen = Mutex::new(Vec::new());
        let last_done = AtomicUsize::new(0);
        let out = analyze_all(
            &paths,
            &cfg(),
            &tools,
            &Anchors::new("x"),
            &Mutex::new(Cache::default()),
            &cancel,
            &|p| {
                last_done.fetch_max(p.done, Ordering::SeqCst);
                assert_eq!(p.total, 5);
            },
            &|i, r| {
                if let Ok(mut s) = seen.lock() {
                    s.push((i, r.name.clone()));
                }
            },
        );
        assert_eq!(out.len(), 5);
        assert!(out
            .iter()
            .enumerate()
            .all(|(i, r)| r.name == format!("{i}.mp4")));
        assert_eq!(last_done.load(Ordering::SeqCst), 5);
        assert_eq!(seen.lock().map(|s| s.len()).unwrap_or(0), 5);
        assert!(analyze_all(
            &[],
            &cfg(),
            &tools,
            &Anchors::new("x"),
            &Mutex::new(Cache::default()),
            &cancel,
            &|_| {},
            &|_, _| {}
        )
        .is_empty());
    }

    #[test]
    fn cancel_stops_before_analysing() {
        let tools = Tools {
            ffmpeg: "/x/ffmpeg".into(),
            ffprobe: "/x/ffprobe".into(),
        };
        let cancel = AtomicBool::new(true);
        let f = std::env::temp_dir().join(format!("fd_cancel_{}.mp4", std::process::id()));
        std::fs::write(&f, b"x").unwrap();
        let out = analyze_all(
            &[f.to_string_lossy().into_owned()],
            &Config {
                cache_enabled: false,
                ..cfg()
            },
            &tools,
            &Anchors::new("x"),
            &Mutex::new(Cache::default()),
            &cancel,
            &|_| {},
            &|_, _| {},
        );
        assert_eq!(
            out.first().and_then(|r| r.error.as_deref()),
            Some("Annullato")
        );
        let _ = std::fs::remove_file(&f);
    }
}
