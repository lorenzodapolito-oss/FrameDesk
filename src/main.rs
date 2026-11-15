#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use framedesk::app;

fn icon() -> eframe::egui::IconData {
    eframe::egui::IconData {
        rgba: include_bytes!("../assets/icon.rgba").to_vec(),
        width: 128,
        height: 128,
    }
}

fn options(renderer: eframe::Renderer) -> eframe::NativeOptions {
    eframe::NativeOptions {
        renderer,
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title(format!(
                "{} di Lorenzo D'Apolito v{}",
                app::APP_TITLE,
                app::APP_VERSION
            ))
            .with_inner_size([1180.0, 760.0])
            .with_min_inner_size([760.0, 480.0])
            .with_drag_and_drop(true)
            .with_icon(icon()),
        ..Default::default()
    }
}

fn report_fatal(text: &str) {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let _ = std::fs::write(dir.join("FrameDesk-errore.txt"), text);
        }
    }
    rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Error)
        .set_title("FrameDesk non riesce ad avviarsi")
        .set_description(text)
        .show();
}

fn run(open: Vec<std::path::PathBuf>) -> eframe::Result {
    let mut last = Ok(());
    for renderer in [eframe::Renderer::Wgpu, eframe::Renderer::Glow] {
        let open = open.clone();
        last = eframe::run_native(
            app::APP_TITLE,
            options(renderer),
            Box::new(move |cc| Ok(Box::new(app::App::new(cc, open.clone())))),
        );
        if last.is_ok() {
            break;
        }
    }
    last
}

fn main() {
    std::panic::set_hook(Box::new(|info| {
        report_fatal(&format!(
            "FrameDesk v{}\nErrore imprevisto.\n\n{info}",
            app::APP_VERSION
        ));
    }));
    let open: Vec<std::path::PathBuf> = std::env::args_os().skip(1).map(Into::into).collect();
    if let Err(e) = run(open) {
        report_fatal(&format!(
            "FrameDesk v{}\nLa finestra non si apre: serve DirectX/Vulkan o OpenGL. Installa i driver video dal sito del produttore.\n\n{e}",
            app::APP_VERSION
        ));
    }
}
