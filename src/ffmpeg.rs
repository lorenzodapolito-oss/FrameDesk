//! ffprobe e ffmpeg: informazioni sul video e singoli fotogrammi come pixel grezzi.

use crate::imgproc::Rgb;
use serde_json::Value;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Percorsi dei due programmi.
#[derive(Clone, Debug)]
pub struct Tools {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
}

/// Informazioni lette da ffprobe.
#[derive(Clone, Debug, PartialEq)]
pub struct Probe {
    pub codec: String,
    pub pix_fmt: String,
    pub color_space: String,
    pub color_transfer: String,
    pub color_primaries: String,
    pub bits: u32,
    pub is_hdr: bool,
    pub hdr_type: String,
    pub width: usize,
    pub height: usize,
    pub fps: f64,
    pub duration: f64,
    pub total_frames: u64,
}

impl Default for Probe {
    fn default() -> Self {
        Self {
            codec: "unknown".into(),
            pix_fmt: "unknown".into(),
            color_space: "unknown".into(),
            color_transfer: "unknown".into(),
            color_primaries: "unknown".into(),
            bits: 8,
            is_hdr: false,
            hdr_type: "SDR".into(),
            width: 0,
            height: 0,
            fps: 25.0,
            duration: 0.0,
            total_frames: 0,
        }
    }
}

/// Evita la finestra nera dei programmi a console. Solo Windows.
pub fn quiet(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    cmd
}

fn exe_name(base: &str) -> String {
    if cfg!(windows) {
        format!("{base}.exe")
    } else {
        base.to_string()
    }
}

/// Cerca ffmpeg e ffprobe: accanto al programma (anche in `ffmpeg` o `ffmpeg\bin`), poi nel PATH.
pub fn find_tools() -> Option<Tools> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(d) = exe.parent() {
            dirs.push(d.to_path_buf());
            dirs.push(d.join("ffmpeg"));
            dirs.push(d.join("ffmpeg").join("bin"));
            dirs.push(d.join("bin"));
        }
    }
    if let Some(paths) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&paths));
    }
    let (fm, fp) = (exe_name("ffmpeg"), exe_name("ffprobe"));
    dirs.into_iter().find_map(|d| {
        let (a, b) = (d.join(&fm), d.join(&fp));
        (a.is_file() && b.is_file()).then_some(Tools {
            ffmpeg: a,
            ffprobe: b,
        })
    })
}

/// Esegue un programma con un tempo massimo; se lo supera lo termina.
/// Dà l'uscita standard solo se il programma è finito bene.
fn run_capture(cmd: &mut Command, timeout: Duration, max_bytes: usize) -> Result<Vec<u8>, String> {
    quiet(cmd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("non riesco ad avviare il programma: {e}"))?;
    let mut out = child.stdout.take().ok_or("uscita non disponibile")?;
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = (&mut out).take(max_bytes as u64 + 1).read_to_end(&mut buf);
        buf
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = reader.join();
                    return Err("tempo scaduto".into());
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(e) => return Err(format!("errore nell'attesa: {e}")),
        }
    };
    let buf = reader
        .join()
        .map_err(|_| "lettura interrotta".to_string())?;
    if !status.success() {
        return Err("il programma ha dato errore".into());
    }
    if buf.len() > max_bytes {
        return Err("risposta troppo grande".into());
    }
    Ok(buf)
}

fn parse_rate(s: &str) -> Option<f64> {
    let (n, d) = s.split_once('/').map_or((s, "1"), |p| p);
    let (n, d): (f64, f64) = (n.trim().parse().ok()?, d.trim().parse().ok()?);
    (d != 0.0 && n.is_finite() && n > 0.0).then(|| n / d)
}

fn str_of(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_string)
}

fn num_of(v: &Value, key: &str) -> Option<f64> {
    v.get(key).and_then(|x| match x {
        Value::String(s) => s.parse().ok(),
        other => other.as_f64(),
    })
}

/// Legge l'uscita JSON di ffprobe (`-show_streams -show_format`). Valori mancanti o strani
/// ("N/A") danno quelli di partenza.
pub fn parse_probe(json: &str) -> Probe {
    let mut p = Probe::default();
    let Ok(root) = serde_json::from_str::<Value>(json) else {
        return p;
    };
    let stream = root
        .get("streams")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .cloned()
        .unwrap_or(Value::Null);
    if let Some(c) = str_of(&stream, "codec_name") {
        p.codec = c;
    }
    if let Some(c) = str_of(&stream, "pix_fmt") {
        p.pix_fmt = c;
    }
    if let Some(c) = str_of(&stream, "color_space") {
        p.color_space = c;
    }
    if let Some(c) = str_of(&stream, "color_transfer") {
        p.color_transfer = c;
    }
    if let Some(c) = str_of(&stream, "color_primaries") {
        p.color_primaries = c;
    }
    p.bits = match num_of(&stream, "bits_per_raw_sample") {
        Some(b) if (1.0..=32.0).contains(&b) => b as u32,
        _ => {
            if p.pix_fmt.contains("10") {
                10
            } else if p.pix_fmt.contains("12") {
                12
            } else {
                8
            }
        }
    };
    p.width = num_of(&stream, "width").map_or(0, |v| v.clamp(0.0, 65535.0) as usize);
    p.height = num_of(&stream, "height").map_or(0, |v| v.clamp(0.0, 65535.0) as usize);

    p.fps = ["avg_frame_rate", "r_frame_rate"]
        .iter()
        .filter_map(|k| str_of(&stream, k))
        .find_map(|s| parse_rate(&s))
        .filter(|f| *f <= 1000.0)
        .unwrap_or(25.0);

    let dur = num_of(&stream, "duration")
        .or_else(|| root.get("format").and_then(|f| num_of(f, "duration")))
        .filter(|d| d.is_finite() && *d > 0.0)
        .unwrap_or(0.0);
    p.duration = dur;
    let nb = num_of(&stream, "nb_frames").filter(|n| *n > 0.0 && n.is_finite());
    p.total_frames = match nb {
        Some(n) => n as u64,
        None if dur > 0.0 => (dur * p.fps).round() as u64,
        None => 0,
    };
    if p.duration <= 0.0 && p.total_frames > 0 {
        p.duration = p.total_frames as f64 / p.fps;
    }

    let (ct, cs, cp) = (&p.color_transfer, &p.color_space, &p.color_primaries);
    if ct == "smpte2084" || ct == "arib-std-b67" || cs == "bt2020nc" || cp.contains("2020") {
        p.is_hdr = true;
        p.hdr_type = match ct.as_str() {
            "smpte2084" => "HDR10",
            "arib-std-b67" => "HLG",
            _ => "HDR",
        }
        .to_string();
    }
    let dovi = stream
        .get("side_data_list")
        .and_then(Value::as_array)
        .is_some_and(|l| {
            l.iter()
                .any(|sd| sd.to_string().to_lowercase().contains("dovi"))
        });
    if dovi {
        p.is_hdr = true;
        p.hdr_type = "Dolby Vision".into();
    }
    p
}

impl Tools {
    pub fn probe(&self, path: &Path) -> Probe {
        let mut cmd = Command::new(&self.ffprobe);
        cmd.args([
            "-v",
            "quiet",
            "-print_format",
            "json",
            "-show_streams",
            "-show_format",
            "-select_streams",
            "v:0",
        ])
        .arg(path);
        match run_capture(&mut cmd, Duration::from_secs(20), 4 << 20) {
            Ok(out) => parse_probe(&String::from_utf8_lossy(&out)),
            Err(_) => Probe::default(),
        }
    }

    /// Un fotogramma al secondo `t`, ridotto a `w`x`h` (media di area), come pixel RGB.
    pub fn frame(&self, path: &Path, t: f64, w: usize, h: usize) -> Result<Rgb, String> {
        if w == 0 || h == 0 || w > 4096 || h > 4096 {
            return Err("dimensioni non valide".into());
        }
        let need = w * h * 3;
        let mut cmd = Command::new(&self.ffmpeg);
        cmd.args([
            "-v",
            "error",
            "-nostdin",
            "-an",
            "-sn",
            "-ss",
            &format!("{:.3}", t.max(0.0)),
            "-i",
        ])
        .arg(path)
        .args([
            "-frames:v",
            "1",
            "-vf",
            &format!("scale={w}:{h}:flags=area"),
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "-",
        ]);
        let out = run_capture(&mut cmd, Duration::from_secs(60), need)?;
        Rgb::new(w, h, out).ok_or_else(|| "fotogramma non disponibile".to_string())
    }
}

/// Dimensioni a cui si riduce un fotogramma: al massimo `max_dim` sul lato lungo, mai meno di 10.
pub fn proc_size(w: usize, h: usize, max_dim: usize) -> (usize, usize) {
    let longest = w.max(h);
    if longest <= max_dim || longest == 0 {
        return (w.max(1), h.max(1));
    }
    let scale = max_dim as f64 / longest as f64;
    (
        ((w as f64 * scale) as usize).max(10),
        ((h as f64 * scale) as usize).max(10),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const SDR: &str = r#"{"streams":[{"codec_name":"h264","pix_fmt":"yuv420p","width":1920,"height":1080,
        "avg_frame_rate":"30000/1001","nb_frames":"300","bits_per_raw_sample":"N/A","duration":"10.01"}],
        "format":{"duration":"10.01"}}"#;

    #[test]
    fn parses_a_normal_sdr_stream() {
        let p = parse_probe(SDR);
        assert_eq!((p.codec.as_str(), p.width, p.height), ("h264", 1920, 1080));
        assert!((p.fps - 29.97).abs() < 0.01);
        assert_eq!(p.total_frames, 300);
        assert_eq!(p.bits, 8, "N/A: si ricava da pix_fmt");
        assert!(!p.is_hdr && p.hdr_type == "SDR");
    }

    #[test]
    fn ten_bit_and_hdr_are_recognised() {
        let j = r#"{"streams":[{"codec_name":"hevc","pix_fmt":"yuv420p10le","color_transfer":"smpte2084",
            "color_space":"bt2020nc","color_primaries":"bt2020","width":3840,"height":2160,
            "r_frame_rate":"24/1"}],"format":{"duration":"60"}}"#;
        let p = parse_probe(j);
        assert_eq!(p.bits, 10);
        assert!(p.is_hdr);
        assert_eq!(p.hdr_type, "HDR10");
        assert_eq!(p.total_frames, 1440, "senza nb_frames: durata x fps");
        let hlg = parse_probe(r#"{"streams":[{"color_transfer":"arib-std-b67"}]}"#);
        assert_eq!(hlg.hdr_type, "HLG");
        let wide = parse_probe(r#"{"streams":[{"color_primaries":"bt2020"}]}"#);
        assert_eq!(wide.hdr_type, "HDR");
    }

    #[test]
    fn dolby_vision_side_data_wins() {
        let j = r#"{"streams":[{"color_transfer":"smpte2084",
            "side_data_list":[{"side_data_type":"DOVI configuration record"}]}]}"#;
        assert_eq!(parse_probe(j).hdr_type, "Dolby Vision");
    }

    #[test]
    fn broken_or_hostile_json_gives_defaults() {
        assert_eq!(parse_probe(""), Probe::default());
        assert_eq!(parse_probe("non json"), Probe::default());
        assert_eq!(parse_probe(r#"{"streams":[]}"#), Probe::default());
        let p = parse_probe(
            r#"{"streams":[{"width":-5,"height":1e30,"avg_frame_rate":"0/0","nb_frames":"-3","bits_per_raw_sample":"999"}]}"#,
        );
        assert_eq!(p.width, 0);
        assert_eq!(p.height, 65535);
        assert_eq!(p.fps, 25.0);
        assert_eq!(p.total_frames, 0);
        assert_eq!(p.bits, 8);
        let p = parse_probe(r#"{"streams":[{"avg_frame_rate":"1000000/1"}]}"#);
        assert_eq!(p.fps, 25.0, "frequenze assurde si scartano");
    }

    #[test]
    fn rates_parse_fractions_and_plain_numbers() {
        assert_eq!(parse_rate("25/1"), Some(25.0));
        assert_eq!(parse_rate("30"), Some(30.0));
        assert_eq!(parse_rate("0/0"), None);
        assert_eq!(parse_rate("abc"), None);
        assert_eq!(parse_rate("-5/1"), None);
    }

    #[test]
    fn processing_size_caps_the_long_side() {
        assert_eq!(proc_size(1920, 1080, 640), (640, 360));
        assert_eq!(proc_size(320, 240, 640), (320, 240));
        assert_eq!(proc_size(1080, 1920, 640), (360, 640));
        assert_eq!(proc_size(0, 0, 640), (1, 1));
        let (w, h) = proc_size(10000, 3, 640);
        assert!(w == 640 && h >= 10);
    }

    #[test]
    fn missing_tools_are_reported_not_panicked() {
        let t = Tools {
            ffmpeg: PathBuf::from("/non/esiste/ffmpeg"),
            ffprobe: PathBuf::from("/non/esiste/ffprobe"),
        };
        assert_eq!(t.probe(Path::new("x.mp4")), Probe::default());
        assert!(t.frame(Path::new("x.mp4"), 0.0, 16, 16).is_err());
        assert!(t.frame(Path::new("x.mp4"), 0.0, 0, 16).is_err());
        assert!(t.frame(Path::new("x.mp4"), 0.0, 99999, 16).is_err());
    }
}
