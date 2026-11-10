#![windows_subsystem = "windows"]
use windows_reactor::*;
mod api;
mod chinese_dictionary;
mod desktop;
mod dictionary;
mod model;
mod model_download;
mod native;
mod offline;
mod routing;
mod selection;
mod speech;
mod spm;
mod storage;
mod ui;
fn main() {
    set_process_identity();
    if std::env::args().any(|arg| arg == "--check-configuration") {
        let _ = storage::check_configuration();
        return;
    }
    std::panic::set_hook(Box::new(|info| {
        let dir = storage::data_dir();
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join("last-error.log"), info.to_string());
    }));
    let background = std::env::args().any(|arg| arg == "--background");
    storage::diagnostic(serde_json::json!({
        "event": "process_start",
        "background": background,
        "executable": std::env::current_exe().ok(),
    }));
    if !background {
        desktop::remember_foreground();
    }
    let _session = match native::initialize(background) {
        Ok(Some(session)) => session,
        Ok(None) => return,
        Err(error) => {
            report_error(&error.to_string(), background);
            return;
        }
    };
    // Load and decrypt configuration before initializing WinUI window state.
    let mut initial_configuration = storage::load();
    let mut initial_text = String::new();
    if background {
        let (settings, _, warning) = &initial_configuration;
        storage::diagnostic(serde_json::json!({
            "event":"background_ready", "winui_started":false, "hotkey":"Ctrl+Tab",
            "selected_provider":settings.provider, "configuration_warning":warning.is_some(),
            "configured_providers":model::Provider::ALL.into_iter()
                .filter(|provider| routing::has_credentials(settings, *provider))
                .collect::<Vec<_>>(),
        }));
        std::thread::spawn(|| {
            dictionary::warm_up();
            offline::warm_up();
        });
        match native::wait_for_activation() {
            Ok(true) => {}
            Ok(false) => return,
            Err(error) => {
                report_error(&error.to_string(), true);
                return;
            }
        }
        if native::take_startup_selection_request() {
            initial_text = selection::capture(&|| false).unwrap_or_default();
        }
        // A resident process can wait for hours before its first window opens.
        initial_configuration = storage::load();
    }
    storage::diagnostic(
        serde_json::json!({"event":"ui_start", "trigger":if background { "explicit_open" } else { "manual_launch" }}),
    );
    if let Err(error) = App::run_component::<ui::Translator>(ui::WindowInput {
        initial_text,
        initial_configuration: Some(initial_configuration),
        ..Default::default()
    }) {
        report_error(
            &format!("{error}\n请确认已安装 Windows App Runtime 2.4 或更新的 2.x。"),
            background,
        );
    }
}
fn set_process_identity() {
    #[link(name = "shell32")]
    unsafe extern "system" {
        fn SetCurrentProcessExplicitAppUserModelID(identity: *const u16) -> i32;
    }
    let identity: Vec<u16> = "WinTranslater.Desktop\0".encode_utf16().collect();
    unsafe {
        SetCurrentProcessExplicitAppUserModelID(identity.as_ptr());
    }
}
fn report_error(error: &str, background: bool) {
    let dir = storage::data_dir();
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(dir.join("last-error.log"), error.to_string());
    if background {
        return;
    }
    let text: Vec<u16> = format!(
        "启动失败：{error}\n{}",
        dir.join("last-error.log").display()
    )
    .encode_utf16()
    .chain(Some(0))
    .collect();
    let title: Vec<u16> = "WinTranslater\0".encode_utf16().collect();
    #[link(name = "user32")]
    unsafe extern "system" {
        fn MessageBoxW(
            hwnd: *const std::ffi::c_void,
            text: *const u16,
            caption: *const u16,
            kind: u32,
        ) -> i32;
    }
    unsafe {
        MessageBoxW(std::ptr::null(), text.as_ptr(), title.as_ptr(), 0x10);
    }
}
