//! Tauri wiring: one background worker from the library, events forwarded as
//! typed payloads, and a small command surface the React UI calls.

use std::sync::Mutex;
use std::time::Duration;

use firmware_manage_tool::backend::Backend;
use tauri::{Emitter, Manager};

use crate::dto::UiEvent;

/// Set once at startup so `app_info` can tell the UI whether the emergency
/// release hotkey is actually live.
pub static HOTKEY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

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

/// Ctrl+Alt+Shift+Esc releases everything, even when the window is behind a game.
/// Registration happens before the window exists (so `app_info` can report it and
/// no event is lost), the message loop needs a thread of its own.
#[cfg(windows)]
pub fn register_hotkey() -> bool {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, RegisterHotKey, VK_ESCAPE,
    };
    // SAFETY: registering a thread-agnostic hotkey with a null window handle.
    unsafe {
        RegisterHotKey(
            std::ptr::null_mut(),
            HOTKEY_ID,
            MOD_CONTROL | MOD_ALT | MOD_SHIFT | MOD_NOREPEAT,
            u32::from(VK_ESCAPE),
        ) != 0
    }
}

#[cfg(windows)]
const HOTKEY_ID: i32 = 1;

#[cfg(windows)]
fn pump_hotkey(handle: tauri::AppHandle) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetMessageW, MSG, WM_HOTKEY};

    std::thread::spawn(move || {
        let registered = register_hotkey();
        HOTKEY.store(registered, std::sync::atomic::Ordering::Relaxed);
        let _ = handle.emit(
            "app://event",
            UiEvent::Log {
                line: if registered {
                    "緊急放開熱鍵已啟用：Ctrl+Alt+Shift+Esc".to_string()
                } else {
                    "無法註冊緊急放開熱鍵 Ctrl+Alt+Shift+Esc（可能被其他程式佔用）".to_string()
                },
            },
        );
        let mut message: MSG = unsafe { std::mem::zeroed() };
        loop {
            // SAFETY: the thread owns the queue that received the registration.
            let got = unsafe { GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) };
            if got <= 0 {
                return;
            }
            if message.message != WM_HOTKEY || message.wParam != HOTKEY_ID as usize {
                continue;
            }
            if let Some(state) = handle.try_state::<State>() {
                let _ = state.submit(firmware_manage_tool::backend::AppCommand::Reset);
            }
            let _ = handle.emit(
                "app://event",
                UiEvent::Log {
                    line: "熱鍵：全部放開".into(),
                },
            );
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
            crate::commands::type_text,
            crate::commands::chord,
            crate::commands::trajectory_preview,
            crate::commands::macro_save,
            crate::commands::macro_load,
            crate::commands::macro_list,
            crate::commands::choose_target,
            crate::commands::cancel,
            crate::commands::settings_load,
            crate::commands::settings_save,
        ])
        .setup(|app| {
            spawn_pump(app.handle().clone());
            #[cfg(windows)]
            pump_hotkey(app.handle().clone());
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
