//! Grant desktop IPC only to the main window's configured loopback server.

use tauri::Manager;

fn capability(base: &str) -> String {
    let mut capability: serde_json::Value =
        serde_json::from_str(include_str!("../runtime-capabilities/main.json"))
            .expect("valid main capability");
    capability["remote"]["urls"] = serde_json::json!([format!("{base}/**")]);
    capability.to_string()
}

pub fn install(app: &tauri::AppHandle) -> tauri::Result<()> {
    // This file lives outside capabilities/ so an inactive default port does
    // not retain permissions when NOOR_PORT or NOOR_ADDR overrides the port.
    app.add_capability(capability(&crate::server_url::base()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tauri::{
        ipc::{CallbackFn, InvokeBody},
        test::{get_ipc_response, mock_builder, MockRuntime, INVOKE_KEY},
        webview::InvokeRequest,
        WebviewWindow, WebviewWindowBuilder,
    };

    // Use a side-effect-free handler to probe the real app permission manifest.
    #[tauri::command]
    fn get_install_mode() -> &'static str {
        "portable"
    }

    fn app(base: &str) -> tauri::App<MockRuntime> {
        let app = mock_builder()
            .invoke_handler(tauri::generate_handler![get_install_mode])
            .build(tauri::generate_context!())
            .unwrap();
        app.add_capability(capability(base)).unwrap();
        app
    }

    fn invoke(window: &WebviewWindow<MockRuntime>, origin: &str, command: &str) -> bool {
        get_ipc_response(
            window,
            InvokeRequest {
                cmd: command.into(),
                callback: CallbackFn(0),
                error: CallbackFn(1),
                url: origin.parse().unwrap(),
                body: InvokeBody::Json(serde_json::json!({
                    "window": "main", "label": "main", "value": 1.05
                })),
                headers: Default::default(),
                invoke_key: INVOKE_KEY.into(),
            },
        )
        .is_ok()
    }

    #[test]
    fn configured_loopback_can_invoke_app_commands_and_zoom() {
        for base in ["http://127.0.0.1:17600", "http://127.0.0.1:17611"] {
            let app = app(base);
            let window = WebviewWindowBuilder::new(&app, "main", Default::default())
                .build()
                .unwrap();
            for path in ["/", "/settings?tab=general"] {
                let origin = format!("{base}{path}");
                assert!(invoke(&window, &origin, "get_install_mode"));
                assert!(invoke(&window, &origin, "plugin:webview|set_webview_zoom"));
            }
        }
    }

    #[test]
    fn other_ports_hosts_protocols_and_windows_cannot_invoke_desktop_ipc() {
        let app = app("http://127.0.0.1:17611");
        let window = WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();
        for origin in [
            "http://127.0.0.1:17600/",
            "http://127.0.0.1:17612/",
            "http://localhost:17611/",
            "https://127.0.0.1:17611/",
            "http://127.0.0.1.evil.example:17611/",
            "http://tauri.evil.example/",
            "https://tauri.localhost/",
            "tauri://localhost/",
        ] {
            for command in ["get_install_mode", "plugin:webview|set_webview_zoom"] {
                assert!(!invoke(&window, origin, command), "{origin}: {command}");
            }
        }
        let other = WebviewWindowBuilder::new(&app, "other", Default::default())
            .build()
            .unwrap();
        assert!(!invoke(
            &other,
            "http://127.0.0.1:17611/",
            "get_install_mode"
        ));
    }
}
