//! Tauri shell for FirmwareManageTool.
//!
//! The egui frontend was replaced by this binary plus the React UI in `ui/`. All
//! hardware work still runs in the library's background worker, so the shell only
//! validates arguments, forwards commands and re-emits events as typed payloads.
//!
//! `--latency-worker` must keep working: the latency test re-runs this executable
//! as a child process (the same contract the egui build had), so it is handled
//! before Tauri initialises.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod commands;
mod dto;
mod shell;

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
    shell::run();
}
