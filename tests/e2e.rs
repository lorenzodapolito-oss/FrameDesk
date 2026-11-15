#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]
//! Prova completa con ffmpeg vero (si salta se ffmpeg non c'è).
use framedesk::analyzer::{analyze_all, Analyzer};
use framedesk::anchors::Anchors;
use framedesk::cache::Cache;
use framedesk::config::Config;
use framedesk::ffmpeg::find_tools;
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

fn make(path: &std::path::Path, filter: &str, extra: &[&str]) -> bool {
    Command::new("ffmpeg")
        .args(["-y", "-v", "error", "-f", "lavfi", "-i", filter, "-t", "3"])
        .args(extra)
        .arg(path)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[test]
fn analyses_generated_videos_end_to_end() {
    let Some(tools) = find_tools() else {
        return;
    };
    let dir = std::env::temp_dir().join(format!("fd_e2e_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let good = dir.join("buono.mp4");
    let bad = dir.join("scarso.mp4");
    if !make(
        &good,
        "testsrc2=size=640x360:rate=25",
        &["-c:v", "libx264", "-crf", "18", "-pix_fmt", "yuv420p"],
    ) || !make(
        &bad,
        "testsrc2=size=640x360:rate=25",
        &["-c:v", "libx264", "-crf", "45", "-pix_fmt", "yuv420p"],
    ) {
        let _ = std::fs::remove_dir_all(&dir);
        return;
    }
    let cfg = Config::default();
    let cancel = AtomicBool::new(false);
    let r = Analyzer {
        tools: &tools,
        cfg: &cfg,
        cancel: &cancel,
        frame_jobs: 2,
    }
    .analyze(&good);
    assert!(r.error.is_none(), "{:?}", r.error);
    assert_eq!((r.width, r.height), (640, 360));
    assert!(r.analyzed_frames >= 3 && r.thumbs.best.is_some());
    assert!(r.purity_raw.is_finite() && r.density_raw.is_finite());

    let paths = vec![
        good.to_string_lossy().into_owned(),
        bad.to_string_lossy().into_owned(),
    ];
    let out = analyze_all(
        &paths,
        &cfg,
        &tools,
        &Anchors::new("x"),
        &Mutex::new(Cache::default()),
        &cancel,
        &|_| {},
        &|_, _| {},
    );
    assert_eq!(out.len(), 2);
    assert!(out.iter().all(|r| r.error.is_none() && r.grade.is_some()));
    assert!(out[0].file_size > out[1].file_size);
    assert!(
        out[0].score >= out[1].score,
        "{} < {}",
        out[0].score,
        out[1].score
    );
    let _ = std::fs::remove_dir_all(&dir);
}
