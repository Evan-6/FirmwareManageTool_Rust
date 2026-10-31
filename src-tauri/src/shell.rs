//! Tauri wiring: one background worker from the library, events forwarded as
//! typed payloads, and a small command surface the React UI calls.

use std::sync::Mutex;
use std::time::Duration;

use firmware_manage_tool::backend::Backend;
use tauri::{Emitter, Manager};

use crate::dto::UiEvent;

pub struct State {
    /// The library worker. Locked only for the microseconds it takes to queue a
    /// command or copy one event out; the heavy lifting happens on its own thread.
    pub backend: Mutex<Backend>,
}

impl State {
    pub fn submit(&self, command: firmware_manage_tool::backend::AppCommand) -> Result<(), String> {
        self.backend
            .lock()
            .map_err(|_| "背景工作狀態損毀".to_string())?
            .submit(command)
            .map_err(|error| format!("{error:#}"))
    }

    pub fn cancel(&self) {
        if let Ok(backend) = self.backend.lock() {
            backend.cancel();
        }
    }

    pub fn shutdown(&self) {
        if let Ok(backend) = self.backend.lock() {
            backend.shutdown();
        }
    }

    pub fn choose(&self, target: String) -> Result<(), String> {
        self.backend
            .lock()
            .map_err(|_| "背景工作狀態損毀".to_string())?
            .choices
            .send(target)
            .map_err(|_| "沒有等待中的 bootloader 選擇".to_string())
    }
}

/// Forwards worker events to the webview. Polling keeps the worker free of any
/// Tauri dependency; 5 ms is invisible next to the 150 ms STATUS cadence.
fn spawn_pump(handle: tauri::AppHandle) {
    std::thread::spawn(move || {
        loop {
            let Some(state) = handle.try_state::<State>() else {
                return;
            };
            let event = match state.backend.lock() {
                Ok(backend) => backend.events.try_recv().ok(),
                Err(_) => None,
            };
            match event {
                Some(event) => {
                    let _ = handle.emit("app://event", UiEvent::from(event));
                }
                None => std::thread::sleep(Duration::from_millis(5)),
            }
        }
    });
}

pub fn run() {
    let backend = Backend::start();
    let app = tauri::Builder::default()
        .manage(State {
            backend: Mutex::new(backend),
        })
        .invoke_handler(tauri::generate_handler![
            crate::commands::app_info,
            crate::commands::scan,
            crate::commands::connect,
            crate::commands::disconnect,
            crate::commands::request_status,
            crate::commands::reset_input,
            crate::commands::upload,
            crate::commands::latency,
            crate::commands::mouse,
            crate::commands::choose_target,
            crate::commands::cancel,
            crate::commands::settings_load,
            crate::commands::settings_save,
        ])
        .setup(|app| {
            spawn_pump(app.handle().clone());
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("Tauri 初始化失敗");
    app.run(|handle, event| {
        if matches!(event, tauri::RunEvent::Exit) {
            handle.state::<State>().shutdown();
        }
    });
}
