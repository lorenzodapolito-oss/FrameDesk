//! Tipi dei risultati: voto, miniature, risultato di un video.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Grade {
    Eccellente,
    MoltoBuono,
    Buono,
    Sufficiente,
    Mediocre,
    Scarso,
}

impl Grade {
    /// Soglie del programma Python: 85, 70, 50, 35, 20.
    pub fn from_score(score: f64) -> Self {
        if score >= 85.0 {
            Grade::Eccellente
        } else if score >= 70.0 {
            Grade::MoltoBuono
        } else if score >= 50.0 {
            Grade::Buono
        } else if score >= 35.0 {
            Grade::Sufficiente
        } else if score >= 20.0 {
            Grade::Mediocre
        } else {
            Grade::Scarso
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Grade::Eccellente => "ECCELLENTE",
            Grade::MoltoBuono => "MOLTO BUONO",
            Grade::Buono => "BUONO",
            Grade::Sufficiente => "SUFFICIENTE",
            Grade::Mediocre => "MEDIOCRE",
            Grade::Scarso => "SCARSO",
        }
    }

    /// Colore pastello del voto (RGB). Mai bianco né grigio.
    pub fn rgb(self) -> [u8; 3] {
        match self {
            Grade::Eccellente => [0x8F, 0xE3, 0xB8],
            Grade::MoltoBuono => [0xB5, 0xE8, 0x9A],
            Grade::Buono => [0xE4, 0xEE, 0x9A],
            Grade::Sufficiente => [0xF8, 0xE0, 0x8A],
            Grade::Mediocre => [0xF8, 0xBC, 0x92],
            Grade::Scarso => [0xF6, 0x9E, 0xB0],
        }
    }

    pub fn css(self) -> &'static str {
        match self {
            Grade::Eccellente => "excellent",
            Grade::MoltoBuono => "very-good",
            Grade::Buono => "good",
            Grade::Sufficiente => "sufficient",
            Grade::Mediocre => "mediocre",
            Grade::Scarso => "poor",
        }
    }
}

/// Miniatura 160x90 di un fotogramma significativo. Non si salva nella cache.
#[derive(Clone, Debug, PartialEq)]
pub struct Thumb {
    pub frame_idx: u64,
    pub score: f64,
    pub purity: f64,
    pub density: f64,
    pub upscale: f64,
    pub w: usize,
    pub h: usize,
    pub rgb: Vec<u8>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Thumbs {
    pub best: Option<Thumb>,
    pub worst: Option<Thumb>,
    pub upscale_suspect: Option<Thumb>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct TimelinePoint {
    pub frame_idx: u64,
    pub purity: f64,
    pub density: f64,
    pub blockiness: f64,
    pub fft: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VideoResult {
    pub name: String,
    pub path: String,
    pub error: Option<String>,
    pub size_text: String,
    pub duration_text: String,
    pub total_frames: u64,
    pub analyzed_frames: usize,
    pub codec: String,
    pub width: usize,
    pub height: usize,
    pub fps: f64,
    pub color_space: String,
    pub color_transfer: String,
    pub pix_fmt: String,
    pub bits: u32,
    pub is_hdr: bool,
    pub hdr_type: String,
    pub purity_raw: f64,
    pub density_raw: f64,
    pub stability_raw: f64,
    pub blockiness: f64,
    pub fft_ratio: f64,
    pub upscale_score: f64,
    pub flicker: f64,
    pub freeze_ratio: f64,
    pub dead_seconds: f64,
    pub efficiency: f64,
    pub bpp: f64,
    pub media_lum: f64,
    pub timeline: Vec<TimelinePoint>,
    pub duration_sec: f64,
    pub file_size: u64,
    pub bitrate_mbps: f64,
    pub purity_norm: f64,
    pub density_norm: f64,
    pub stability_norm: f64,
    pub has_anchors: bool,
    pub score: f64,
    pub grade: Option<Grade>,
    pub from_cache: bool,
    #[serde(skip)]
    pub thumbs: Thumbs,
}

impl Default for VideoResult {
    fn default() -> Self {
        Self {
            name: String::new(),
            path: String::new(),
            error: None,
            size_text: String::new(),
            duration_text: String::new(),
            total_frames: 0,
            analyzed_frames: 0,
            codec: "unknown".into(),
            width: 0,
            height: 0,
            fps: 0.0,
            color_space: "unknown".into(),
            color_transfer: "unknown".into(),
            pix_fmt: "unknown".into(),
            bits: 8,
            is_hdr: false,
            hdr_type: "SDR".into(),
            purity_raw: 0.0,
            density_raw: 0.0,
            stability_raw: 0.0,
            blockiness: 0.0,
            fft_ratio: 0.0,
            upscale_score: 0.0,
            flicker: 0.0,
            freeze_ratio: 0.0,
            dead_seconds: 0.0,
            efficiency: 0.0,
            bpp: 0.0,
            media_lum: 0.0,
            timeline: Vec::new(),
            duration_sec: 0.0,
            file_size: 0,
            bitrate_mbps: 0.0,
            purity_norm: 0.0,
            density_norm: 0.0,
            stability_norm: 0.0,
            has_anchors: false,
            score: 0.0,
            grade: None,
            from_cache: false,
            thumbs: Thumbs::default(),
        }
    }
}

impl VideoResult {
    pub fn failed(path: &str, msg: &str) -> Self {
        let name = std::path::Path::new(path)
            .file_name()
            .map_or_else(|| path.to_string(), |n| n.to_string_lossy().into_owned());
        Self {
            name,
            path: path.to_string(),
            error: Some(msg.chars().take(160).collect()),
            ..Self::default()
        }
    }
}

/// Dimensione leggibile (1024 → «1.0 KB»), come `humanize.naturalsize` ma con la virgola decimale.
pub fn human_size(bytes: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i + 1 < U.len() {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", U.get(i).copied().unwrap_or("TB")).replace('.', ",")
    }
}

/// `MM:SS` (o `H:MM:SS` oltre un'ora).
pub fn duration_text(sec: f64) -> String {
    if !sec.is_finite() || sec <= 0.0 {
        return "??:??".into();
    }
    let t = sec as u64;
    if t >= 3600 {
        format!("{}:{:02}:{:02}", t / 3600, (t % 3600) / 60, t % 60)
    } else {
        format!("{:02}:{:02}", t / 60, t % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grade_thresholds_match_the_python_program() {
        for (s, g) in [
            (100.0, Grade::Eccellente),
            (85.0, Grade::Eccellente),
            (84.9, Grade::MoltoBuono),
            (70.0, Grade::MoltoBuono),
            (50.0, Grade::Buono),
            (35.0, Grade::Sufficiente),
            (20.0, Grade::Mediocre),
            (19.9, Grade::Scarso),
            (0.0, Grade::Scarso),
            (-5.0, Grade::Scarso),
        ] {
            assert_eq!(Grade::from_score(s), g, "{s}");
        }
        assert_eq!(Grade::from_score(f64::NAN), Grade::Scarso);
    }

    #[test]
    fn grade_colors_are_never_white_or_gray() {
        for g in [
            Grade::Eccellente,
            Grade::MoltoBuono,
            Grade::Buono,
            Grade::Sufficiente,
            Grade::Mediocre,
            Grade::Scarso,
        ] {
            let [r, gr, b] = g.rgb();
            let range = r.max(gr).max(b) - r.min(gr).min(b);
            assert!(range >= 24, "{g:?}");
        }
    }

    #[test]
    fn sizes_and_durations_read_well() {
        assert_eq!(human_size(10), "10 B");
        assert_eq!(human_size(1536), "1,5 KB");
        assert_eq!(human_size(5 * 1024 * 1024 * 1024), "5,0 GB");
        assert_eq!(duration_text(75.0), "01:15");
        assert_eq!(duration_text(3725.0), "1:02:05");
        assert_eq!(duration_text(0.0), "??:??");
        assert_eq!(duration_text(f64::NAN), "??:??");
    }

    #[test]
    fn failed_result_keeps_the_name_and_cuts_long_messages() {
        let r = VideoResult::failed("C:/video/film.mkv", &"x".repeat(1000));
        assert_eq!(r.name, "film.mkv");
        assert_eq!(r.error.as_deref().map(str::len), Some(160));
    }

    #[test]
    fn result_round_trips_through_json_without_thumbnails() {
        let mut r = VideoResult {
            name: "a.mp4".into(),
            score: 71.5,
            grade: Some(Grade::MoltoBuono),
            ..VideoResult::default()
        };
        r.thumbs.best = Some(Thumb {
            frame_idx: 1,
            score: 1.0,
            purity: 1.0,
            density: 1.0,
            upscale: 0.0,
            w: 1,
            h: 1,
            rgb: vec![1, 2, 3],
        });
        let t = serde_json::to_string(&r).unwrap();
        let back: VideoResult = serde_json::from_str(&t).unwrap();
        assert_eq!(back.score, 71.5);
        assert!(back.thumbs.best.is_none());
        // Un file con campi mancanti si legge lo stesso.
        let partial: VideoResult = serde_json::from_str(r#"{"name":"x"}"#).unwrap();
        assert_eq!(partial.codec, "unknown");
    }
}
