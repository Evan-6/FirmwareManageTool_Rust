#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
fn main() {
    if std::env::args().any(|arg| arg == "--latency-worker") {
        if let Err(error) = firmware_manage_tool::latency::worker_entry() {
            let event = firmware_manage_tool::latency::WorkerEvent::Error(format!("{error:#}"));
            println!("{}", serde_json::to_string(&event).unwrap());
            std::process::exit(1);
        }
        return;
    }
    firmware_manage_tool::platform::enable_dpi_awareness();
    #[cfg(feature = "gui")]
    {
        let options = eframe::NativeOptions {
            renderer: eframe::Renderer::Wgpu,
            viewport: eframe::egui::ViewportBuilder::default()
                .with_inner_size([1080., 780.])
                .with_min_inner_size([800., 600.]),
            ..Default::default()
        };
        if let Err(error) = eframe::run_native(
            "FirmwareManageTool Rust",
            options,
            Box::new(|context| Ok(Box::new(firmware_manage_tool::app::App::new(context)))),
        ) {
            eprintln!("GUI 啟動失敗：{error}");
            std::process::exit(1);
        }
    }
    #[cfg(not(feature = "gui"))]
    eprintln!("請啟用預設 gui feature 建置 GUI。");
}
