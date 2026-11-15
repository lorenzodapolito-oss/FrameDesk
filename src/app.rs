//! Interfaccia: lista video, analisi in parallelo con Annulla, dettaglio, ancore, report.

use crate::analyzer::{analyze_all, finalize, Progress};
use crate::anchors::Anchors;
use crate::cache::Cache;
use crate::config::{self, Config};
use crate::ffmpeg::{find_tools, Tools};
use crate::model::{Thumb, VideoResult};
use crate::{report, theme};
use eframe::egui::{self, Color32, RichText, TextureHandle, TextureOptions};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub const APP_TITLE: &str = "FrameDesk";
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
const VIDEO_EXT: [&str; 14] = [
    "mp4", "mkv", "avi", "mov", "wmv", "flv", "webm", "m4v", "ts", "m2ts", "mpg", "mpeg", "3gp",
    "mts",
];
const MAX_FILES: usize = 5000;

pub fn is_video(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| VIDEO_EXT.contains(&e.to_lowercase().as_str()))
}

/// Video in una cartella (anche nelle sottocartelle, fino a 6 livelli), ordinati per nome.
pub fn scan_folder(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > 6 || out.len() >= MAX_FILES {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
    entries.sort();
    for p in entries {
        if out.len() >= MAX_FILES {
            return;
        }
        if p.is_dir() {
            scan_folder(&p, depth + 1, out);
        } else if is_video(&p) {
            out.push(p);
        }
    }
}

/// Dal percorso di ffmpeg ricava la coppia ffmpeg/ffprobe (ffprobe accanto).
pub fn tools_from(ffmpeg: &Path) -> Option<Tools> {
    let name = ffmpeg.file_name()?.to_string_lossy().to_string();
    let probe_name = name.replace("ffmpeg", "ffprobe");
    let probe = ffmpeg.with_file_name(probe_name);
    (ffmpeg.is_file() && probe.is_file()).then(|| Tools {
        ffmpeg: ffmpeg.to_path_buf(),
        ffprobe: probe,
    })
}

#[derive(Default)]
struct Shared {
    results: Vec<Option<VideoResult>>,
    progress: Option<Progress>,
    running: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum Sort {
    Order,
    Score,
}

pub struct App {
    cfg: Config,
    cfg_path: PathBuf,
    tools: Option<Tools>,
    files: Vec<String>,
    shared: Arc<Mutex<Shared>>,
    cancel: Arc<AtomicBool>,
    cache: Arc<Mutex<Cache>>,
    cache_path: PathBuf,
    anchors: Anchors,
    anchors_path: PathBuf,
    selected: Option<usize>,
    sort: Sort,
    show_settings: bool,
    message: String,
    textures: HashMap<String, TextureHandle>,
}

fn grade_color(r: &VideoResult) -> Color32 {
    r.grade.map_or(theme::PRIMARY_LIGHT, |g| {
        let [a, b, c] = g.rgb();
        Color32::from_rgb(a, b, c)
    })
}

fn color_button(ui: &mut egui::Ui, text: &str, color: Color32) -> egui::Response {
    ui.add(egui::Button::new(RichText::new(text).color(theme::ON_COLOR).strong()).fill(color))
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, open: Vec<PathBuf>) -> Self {
        theme::apply(&cc.egui_ctx);
        let dir = config::data_dir();
        let cfg_path = dir.join("impostazioni.json");
        let cfg = config::load(&cfg_path);
        let anchors_path = dir.join("ancore.json");
        let cache_path = dir.join("cache.json");
        let mut app = Self {
            anchors: Anchors::load(&anchors_path, &cfg.algorithm_key()),
            cache: Arc::new(Mutex::new(Cache::load(&cache_path))),
            tools: find_tools(),
            cfg,
            cfg_path,
            files: Vec::new(),
            shared: Arc::new(Mutex::new(Shared::default())),
            cancel: Arc::new(AtomicBool::new(false)),
            cache_path,
            anchors_path,
            selected: None,
            sort: Sort::Order,
            show_settings: false,
            message: String::new(),
            textures: HashMap::new(),
        };
        app.add_paths(open);
        app
    }

    fn running(&self) -> bool {
        self.shared.lock().map(|s| s.running).unwrap_or(false)
    }

    fn add_paths(&mut self, paths: Vec<PathBuf>) {
        if self.running() {
            self.message =
                "Aspetta la fine dell'analisi (o premi Annulla) prima di aggiungere video.".into();
            return;
        }
        let mut found = Vec::new();
        for p in paths {
            if p.is_dir() {
                scan_folder(&p, 0, &mut found);
            } else if is_video(&p) {
                found.push(p);
            }
        }
        let mut added = 0;
        for p in found {
            let s = p.to_string_lossy().into_owned();
            if !self.files.contains(&s) && self.files.len() < MAX_FILES {
                self.files.push(s);
                added += 1;
            }
        }
        if let Ok(mut sh) = self.shared.lock() {
            sh.results.resize(self.files.len(), None);
        }
        self.message = if added == 0 {
            "Nessun nuovo video trovato.".into()
        } else {
            format!("Aggiunti {added} video.")
        };
    }

    fn clear(&mut self) {
        if self.running() {
            return;
        }
        self.files.clear();
        self.selected = None;
        self.textures.clear();
        if let Ok(mut sh) = self.shared.lock() {
            *sh = Shared::default();
        }
    }

    fn start(&mut self) {
        let Some(tools) = self.tools.clone() else {
            self.message =
                "ffmpeg non trovato: scegli il file ffmpeg con il pulsante qui sopra.".into();
            return;
        };
        if self.files.is_empty() || self.running() {
            return;
        }
        self.cfg = self.cfg.clone().sanitized();
        config::save(&self.cfg_path, &self.cfg);
        let key = self.cfg.algorithm_key();
        if self.anchors.algorithm != key {
            self.anchors = Anchors::new(&key);
        }
        self.cancel.store(false, Ordering::SeqCst);
        self.textures.clear();
        if let Ok(mut sh) = self.shared.lock() {
            sh.results = vec![None; self.files.len()];
            sh.running = true;
            sh.progress = None;
        }
        let (files, cfg, anchors) = (self.files.clone(), self.cfg.clone(), self.anchors.clone());
        let (shared, cache, cancel) =
            (self.shared.clone(), self.cache.clone(), self.cancel.clone());
        let cache_path = self.cache_path.clone();
        std::thread::spawn(move || {
            let (s1, s2) = (shared.clone(), shared.clone());
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                analyze_all(
                    &files,
                    &cfg,
                    &tools,
                    &anchors,
                    &cache,
                    &cancel,
                    &move |p| {
                        if let Ok(mut s) = s1.lock() {
                            s.progress = Some(p);
                        }
                    },
                    &move |i, r| {
                        if let Ok(mut s) = s2.lock() {
                            if let Some(slot) = s.results.get_mut(i) {
                                *slot = Some(r.clone());
                            }
                        }
                    },
                );
            }));
            if let Ok(c) = cache.lock() {
                c.save(&cache_path);
            }
            if let Ok(mut s) = shared.lock() {
                s.running = false;
                if r.is_err() {
                    s.progress = None;
                }
            }
        });
    }

    fn done_results(&self) -> Vec<VideoResult> {
        self.shared
            .lock()
            .map(|s| s.results.iter().flatten().cloned().collect())
            .unwrap_or_default()
    }

    fn refinalize(&mut self) {
        let (anchors, cfg) = (self.anchors.clone(), self.cfg.clone());
        if let Ok(mut s) = self.shared.lock() {
            for r in s.results.iter_mut().flatten() {
                finalize(r, &anchors, &cfg);
            }
        }
        self.anchors.save(&self.anchors_path);
    }

    fn export(&mut self) {
        let results = self.done_results();
        if results.is_empty() {
            self.message = "Nessun risultato da esportare.".into();
            return;
        }
        let Some(path) = rfd::FileDialog::new()
            .set_file_name("FrameDesk-report.html")
            .add_filter("Pagina web", &["html"])
            .save_file()
        else {
            return;
        };
        let when = chrono::Local::now().format("%d/%m/%Y %H:%M").to_string();
        self.message = match std::fs::write(&path, report::html(&results, &when)) {
            Ok(()) => format!("Report salvato in {}", path.display()),
            Err(e) => format!("Non riesco a salvare il report: {e}"),
        };
    }

    fn texture(&mut self, ctx: &egui::Context, key: String, t: &Thumb) -> Option<egui::TextureId> {
        if !self.textures.contains_key(&key) {
            if t.rgb.len() != t.w * t.h * 3 || t.w == 0 {
                return None;
            }
            let img = egui::ColorImage::from_rgb([t.w, t.h], &t.rgb);
            self.textures.insert(
                key.clone(),
                ctx.load_texture(key.clone(), img, TextureOptions::LINEAR),
            );
        }
        self.textures.get(&key).map(TextureHandle::id)
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let running = self.running();
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(format!("{APP_TITLE} di Lorenzo D'Apolito"))
                    .size(24.0)
                    .strong()
                    .color(theme::PRIMARY),
            );
            ui.label(RichText::new(format!("v{APP_VERSION}")).color(theme::MUTED));
        });
        ui.horizontal_wrapped(|ui| {
            if color_button(ui, "Aggiungi video", theme::BTN_BLUE).clicked() {
                if let Some(f) = rfd::FileDialog::new()
                    .add_filter("Video", &VIDEO_EXT)
                    .pick_files()
                {
                    self.add_paths(f);
                }
            }
            if color_button(ui, "Aggiungi cartella", theme::BTN_LILAC).clicked() {
                if let Some(d) = rfd::FileDialog::new().pick_folder() {
                    self.add_paths(vec![d]);
                }
            }
            if running {
                if color_button(ui, "Annulla", theme::BTN_PINK).clicked() {
                    self.cancel.store(true, Ordering::SeqCst);
                }
            } else if color_button(ui, "Analizza", theme::BTN_MINT).clicked() {
                self.start();
            }
            if color_button(ui, "Svuota", theme::BTN_PEACH).clicked() {
                self.clear();
            }
            if color_button(ui, "Esporta report", theme::BTN_YELLOW).clicked() {
                self.export();
            }
            if color_button(ui, "Impostazioni", theme::BTN_BLUE).clicked() {
                self.show_settings = !self.show_settings;
            }
            if self.tools.is_none() && color_button(ui, "Scegli ffmpeg…", theme::BTN_PINK).clicked()
            {
                if let Some(f) = rfd::FileDialog::new()
                    .set_title("Scegli ffmpeg")
                    .pick_file()
                {
                    self.tools = tools_from(&f);
                    self.message = if self.tools.is_some() {
                        "ffmpeg trovato.".into()
                    } else {
                        "Accanto a ffmpeg ci deve essere anche ffprobe.".into()
                    };
                }
            }
            ui.selectable_value(&mut self.sort, Sort::Order, "Ordine di inserimento");
            ui.selectable_value(&mut self.sort, Sort::Score, "Per punteggio");
        });
        if let Some(p) = self
            .shared
            .lock()
            .ok()
            .and_then(|s| s.progress.clone())
            .filter(|_| running)
        {
            ui.add(
                egui::ProgressBar::new(p.done as f32 / p.total.max(1) as f32)
                    .fill(theme::BTN_MINT)
                    .text(format!("{} / {} · {}", p.done, p.total, p.current)),
            );
        }
        if self.tools.is_none() {
            ui.colored_label(
                theme::ERR,
                "ffmpeg e ffprobe non sono stati trovati: mettili accanto al programma o nel PATH.",
            );
        }
        if !self.message.is_empty() {
            ui.label(RichText::new(&self.message).color(theme::MUTED));
        }
    }

    fn table(&mut self, ui: &mut egui::Ui) {
        if self.files.is_empty() {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new("Trascina qui i video o le cartelle")
                        .size(22.0)
                        .color(theme::MUTED),
                );
            });
            return;
        }
        let rows: Vec<(usize, Option<VideoResult>)> = {
            let sh = self.shared.lock();
            let mut v: Vec<(usize, Option<VideoResult>)> = (0..self.files.len())
                .map(|i| {
                    (
                        i,
                        sh.as_ref()
                            .ok()
                            .and_then(|s| s.results.get(i).cloned().flatten()),
                    )
                })
                .collect();
            if self.sort == Sort::Score {
                v.sort_by(|a, b| {
                    let (sa, sb) = (
                        a.1.as_ref().map_or(-1.0, |r| r.score),
                        b.1.as_ref().map_or(-1.0, |r| r.score),
                    );
                    sb.total_cmp(&sa)
                });
            }
            v
        };
        egui::ScrollArea::both()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Grid::new("tabella")
                    .striped(true)
                    .num_columns(10)
                    .spacing([14.0, 6.0])
                    .show(ui, |ui| {
                        for h in [
                            "Video",
                            "Durata",
                            "Risoluzione",
                            "Codec",
                            "Mbps",
                            "Purezza",
                            "Densità",
                            "Stabilità",
                            "Punteggio",
                            "Voto",
                        ] {
                            ui.label(RichText::new(h).strong());
                        }
                        ui.end_row();
                        for (i, r) in &rows {
                            let name = self.files.get(*i).map_or("?", |p| {
                                Path::new(p)
                                    .file_name()
                                    .and_then(|n| n.to_str())
                                    .unwrap_or(p)
                            });
                            let sel = self.selected == Some(*i);
                            if ui.selectable_label(sel, name).clicked() {
                                self.selected = Some(*i);
                            }
                            match r {
                                None => {
                                    ui.label("…");
                                    for _ in 0..8 {
                                        ui.label("");
                                    }
                                }
                                Some(r) if r.error.is_some() => {
                                    ui.colored_label(theme::ERR, r.error.as_deref().unwrap_or(""));
                                    for _ in 0..8 {
                                        ui.label("");
                                    }
                                }
                                Some(r) => {
                                    ui.label(&r.duration_text);
                                    ui.label(format!("{}x{}", r.width, r.height));
                                    ui.label(&r.codec);
                                    ui.label(format!("{:.2}", r.bitrate_mbps));
                                    ui.label(format!("{:.0}", r.purity_norm));
                                    ui.label(format!("{:.0}", r.density_norm));
                                    ui.label(format!("{:.0}", r.stability_norm));
                                    ui.label(RichText::new(format!("{:.1}", r.score)).strong());
                                    let label = r.grade.map_or("-", |g| g.label());
                                    ui.add(
                                        egui::Button::new(
                                            RichText::new(label).color(theme::ON_COLOR).strong(),
                                        )
                                        .fill(grade_color(r)),
                                    );
                                }
                            }
                            ui.end_row();
                        }
                    });
            });
    }

    fn timeline(ui: &mut egui::Ui, r: &VideoResult) {
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width().min(560.0), 110.0),
            egui::Sense::hover(),
        );
        let p = ui.painter();
        p.rect_filled(rect, 8.0, theme::SURFACE);
        p.rect_stroke(
            rect,
            8.0,
            egui::Stroke::new(1.0, theme::BORDER),
            egui::StrokeKind::Inside,
        );
        let n = r.timeline.len();
        if n < 2 {
            return;
        }
        let max_frame = r
            .timeline
            .iter()
            .map(|t| t.frame_idx)
            .max()
            .unwrap_or(1)
            .max(1) as f32;
        let line = |f: &dyn Fn(&crate::model::TimelinePoint) -> f64, c: Color32| {
            let pts: Vec<egui::Pos2> = r
                .timeline
                .iter()
                .map(|t| {
                    let x = rect.left()
                        + 6.0
                        + (rect.width() - 12.0) * (t.frame_idx as f32 / max_frame);
                    let y = rect.bottom()
                        - 6.0
                        - (rect.height() - 12.0) * (f(t).clamp(0.0, 100.0) as f32 / 100.0);
                    egui::pos2(x, y)
                })
                .collect();
            p.add(egui::Shape::line(pts, egui::Stroke::new(2.0, c)));
        };
        line(&|t| t.purity, theme::PLOT_PURITY);
        line(&|t| t.density, theme::PLOT_DENSITY);
    }

    fn detail(&mut self, ui: &mut egui::Ui) {
        let Some(i) = self.selected else { return };
        let Some(r) = self
            .shared
            .lock()
            .ok()
            .and_then(|s| s.results.get(i).cloned().flatten())
        else {
            return;
        };
        ui.add_space(4.0);
        ui.label(RichText::new(&r.name).size(18.0).strong());
        if let Some(e) = &r.error {
            ui.colored_label(theme::ERR, e);
            return;
        }
        ui.label(format!(
            "{} · {} · {}x{} {:.2} fps · {} · {} bit {} · {} fotogrammi analizzati su {}{}",
            r.size_text,
            r.duration_text,
            r.width,
            r.height,
            r.fps,
            r.codec,
            r.bits,
            r.hdr_type,
            r.analyzed_frames,
            r.total_frames,
            if r.from_cache { " (dalla cache)" } else { "" }
        ));
        ui.label(format!(
            "Blocchettatura {:.2} · rapporto dettaglio {:.3} · sospetto upscaling {:.0}% · sfarfallio {:.2} · fermi immagine {:.0}% · efficienza {:.1}",
            r.blockiness, r.fft_ratio, r.upscale_score * 100.0, r.flicker, r.freeze_ratio * 100.0, r.efficiency
        ));
        ui.horizontal(|ui| {
            let anchored = self.anchors.anchors.contains_key(&r.name);
            if anchored {
                if color_button(ui, "Togli dalle ancore", theme::BTN_PEACH).clicked() {
                    self.anchors.remove(&r.name);
                    self.refinalize();
                }
            } else if color_button(ui, "Usa come ancora", theme::BTN_LILAC).clicked() {
                self.anchors.set(&r.name, &r);
                self.refinalize();
            }
            ui.label(
                RichText::new(format!(
                    "Ancore: {} (con almeno 2 i voti si confrontano coi tuoi riferimenti)",
                    self.anchors.anchors.len()
                ))
                .color(theme::MUTED),
            );
        });
        ui.horizontal(|ui| {
            ui.label(RichText::new("Purezza").color(theme::PLOT_PURITY).strong());
            ui.label(RichText::new("Densità").color(theme::PLOT_DENSITY).strong());
            ui.label(RichText::new("lungo il video").color(theme::MUTED));
        });
        Self::timeline(ui, &r);
        let ctx = ui.ctx().clone();
        ui.horizontal(|ui| {
            for (title, t, tag) in [
                ("Migliore", &r.thumbs.best, "b"),
                ("Peggiore", &r.thumbs.worst, "w"),
                ("Sospetto upscaling", &r.thumbs.upscale_suspect, "u"),
            ] {
                let Some(t) = t else { continue };
                if let Some(id) = self.texture(&ctx, format!("{}-{tag}", r.path), t) {
                    ui.vertical(|ui| {
                        ui.label(title);
                        ui.add(
                            egui::Image::new((id, egui::vec2(t.w as f32 * 1.5, t.h as f32 * 1.5)))
                                .corner_radius(8.0),
                        );
                    });
                }
            }
        });
    }

    fn settings(&mut self, ctx: &egui::Context) {
        let mut open = self.show_settings;
        let mut changed = false;
        egui::Window::new("Impostazioni")
            .open(&mut open)
            .resizable(false)
            .show(ctx, |ui| {
                let c = &mut self.cfg;
                ui.checkbox(
                    &mut c.adaptive_frames,
                    "Numero di fotogrammi in base alla durata",
                );
                ui.checkbox(&mut c.smart_crop, "Ritaglio centrale (ignora i bordi neri)");
                ui.checkbox(&mut c.scene_detection, "Rileva i cambi di scena");
                ui.checkbox(&mut c.cache_enabled, "Ricorda i risultati (cache)");
                ui.add(
                    egui::Slider::new(&mut c.crop_ratio, 0.3..=1.0)
                        .text("Parte centrale analizzata"),
                );
                ui.add(
                    egui::Slider::new(&mut c.frame_sampling_base, 8..=120)
                        .text("Fotogrammi (se non adattivo)"),
                );
                ui.add(
                    egui::Slider::new(&mut c.max_threads, 1..=32)
                        .text("Video in parallelo (massimo)"),
                );
                ui.add(
                    egui::Slider::new(&mut c.timeout_seconds, 30..=3600)
                        .text("Tempo massimo per video (s)"),
                );
                ui.separator();
                ui.label("Pesi del punteggio");
                ui.add(egui::Slider::new(&mut c.weight_purity, 0.0..=1.0).text("Purezza"));
                ui.add(egui::Slider::new(&mut c.weight_density, 0.0..=1.0).text("Densità"));
                ui.add(egui::Slider::new(&mut c.weight_stability, 0.0..=1.0).text("Stabilità"));
                ui.horizontal(|ui| {
                    if color_button(ui, "Applica", theme::BTN_MINT).clicked() {
                        changed = true;
                    }
                    if color_button(ui, "Valori predefiniti", theme::BTN_PEACH).clicked() {
                        *c = Config::default();
                        changed = true;
                    }
                    if color_button(ui, "Svuota cache", theme::BTN_PINK).clicked() {
                        if let Ok(mut k) = self.cache.lock() {
                            k.clear();
                            k.save(&self.cache_path);
                        }
                    }
                });
            });
        self.show_settings = open;
        if changed {
            self.cfg = self.cfg.clone().sanitized();
            config::save(&self.cfg_path, &self.cfg);
            let key = self.cfg.algorithm_key();
            if self.anchors.algorithm != key {
                self.anchors = Anchors::new(&key);
            }
            self.refinalize();
            self.message = "Impostazioni applicate. Rianalizza per usare i nuovi parametri.".into();
        }
    }
}

impl eframe::App for App {
    fn clear_color(&self, _v: &egui::Visuals) -> [f32; 4] {
        theme::BG.to_normalized_gamma_f32()
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .filter(|p| !p.as_os_str().is_empty())
                .collect()
        });
        if !dropped.is_empty() {
            self.add_paths(dropped);
        }
        if self.running() {
            ctx.request_repaint_after(std::time::Duration::from_millis(150));
        }
        egui::Panel::top("barra").show(ui, |ui| {
            ui.add_space(6.0);
            self.top_bar(ui);
            ui.add_space(4.0);
        });
        if self.selected.is_some() {
            egui::Panel::bottom("dettaglio")
                .resizable(true)
                .default_size(330.0)
                .show(ui, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| self.detail(ui));
                });
        }
        egui::CentralPanel::default().show(ui, |ui| self.table(ui));
        if self.show_settings {
            self.settings(&ctx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_video_extensions_case_insensitively() {
        assert!(is_video(Path::new("a/B.MKV")) && is_video(Path::new("x.mp4")));
        assert!(!is_video(Path::new("a.txt")) && !is_video(Path::new("mp4")));
    }

    #[test]
    fn scans_folders_recursively() {
        let d = std::env::temp_dir().join(format!("fd_scan_{}", std::process::id()));
        let sub = d.join("s");
        let _ = std::fs::create_dir_all(&sub);
        for f in [d.join("a.mp4"), sub.join("b.avi"), sub.join("c.txt")] {
            std::fs::write(f, b"x").unwrap();
        }
        let mut out = Vec::new();
        scan_folder(&d, 0, &mut out);
        assert_eq!(out.len(), 2);
        let mut none = Vec::new();
        scan_folder(&d.join("manca"), 0, &mut none);
        assert!(none.is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn tools_from_needs_ffprobe_next_to_ffmpeg() {
        let d = std::env::temp_dir().join(format!("fd_tools_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&d);
        std::fs::write(d.join("ffmpeg.exe"), b"x").unwrap();
        assert!(tools_from(&d.join("ffmpeg.exe")).is_none());
        std::fs::write(d.join("ffprobe.exe"), b"x").unwrap();
        assert!(tools_from(&d.join("ffmpeg.exe")).is_some());
        assert!(tools_from(&d.join("manca.exe")).is_none());
        let _ = std::fs::remove_dir_all(&d);
    }
}
