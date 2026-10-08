//! Video fullscreen without Tauri's window fullscreen.
//!
//! On Windows, Tauri (tao) goes fullscreen in steps: it drops the title bar,
//! then moves and grows the window asynchronously, and wry resizes the
//! webview container asynchronously after that. For a frame or two the
//! window and the page disagree, and whatever sits behind the window shows
//! through: the old title bar's close button, the desktop wallpaper in the
//! strip a left taskbar leaves, other apps on the way out.
//!
//! Here the page is laid out at fullscreen size first and given a few frames
//! to draw (WebView2 renders in its own process), then the title bar and the
//! size change in one synchronous call, and the webview is resized in the
//! same step. Leaving mirrors it. The window keeps its maximized flag the
//! whole time, so leaving needs no placement call (which hides and re-shows
//! the window) and un-maximizing afterwards still returns to the normal size.

use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

#[derive(Default)]
pub struct VideoFullscreen(Arc<Mutex<Option<imp::Saved>>>);

#[tauri::command]
pub async fn set_video_fullscreen(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, VideoFullscreen>,
    on: bool,
) -> Result<(), String> {
    let saved = state.0.clone();
    let (done, wait) = mpsc::channel();
    let target = window.clone();
    window
        .with_webview(move |webview| {
            let result = imp::apply(&target, &webview, &saved, on);
            let _ = done.send(result);
        })
        .map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || wait.recv_timeout(Duration::from_secs(2)))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|_| "video fullscreen timed out".to_string())?
}

#[cfg(windows)]
mod imp {
    use std::sync::Mutex;
    use tauri::webview::PlatformWebview;
    use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Controller;
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::Graphics::Dwm::DwmFlush;
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromWindow, RedrawWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
        RDW_ERASE, RDW_ERASENOW, RDW_INVALIDATE, RDW_UPDATENOW,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetClientRect, GetWindowLongPtrW, GetWindowRect, SetWindowLongPtrW, SetWindowPos,
        GWL_STYLE, HWND_TOP, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOCOPYBITS, SWP_NOMOVE,
        SWP_NOOWNERZORDER, SWP_NOZORDER, WS_CAPTION, WS_THICKFRAME,
    };

    /// The window as it was before fullscreen.
    pub struct Saved {
        style: isize,
        rect: RECT,
    }

    pub fn apply(
        window: &tauri::WebviewWindow,
        webview: &PlatformWebview,
        saved: &Mutex<Option<Saved>>,
        on: bool,
    ) -> Result<(), String> {
        let hwnd = HWND(window.hwnd().map_err(|error| error.to_string())?.0 as _);
        let controller = webview.controller();
        let mut saved = saved
            .lock()
            .map_err(|_| "video fullscreen state poisoned".to_string())?;
        unsafe {
            if on && saved.is_none() {
                *saved = Some(enter(hwnd, &controller));
            } else if !on {
                if let Some(previous) = saved.take() {
                    exit(hwnd, &controller, &previous);
                }
            }
        }
        Ok(())
    }

    /// Sizes the webview synchronously (wry's own resize after WM_SIZE is
    /// asynchronous).
    unsafe fn size_webview(controller: &ICoreWebView2Controller, width: i32, height: i32) {
        let mut container = HWND::default();
        if controller.ParentWindow(&mut container).is_ok() {
            let _ = SetWindowPos(
                container,
                None,
                0,
                0,
                width,
                height,
                SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOMOVE,
            );
        }
        let _ = controller.SetBounds(RECT {
            left: 0,
            top: 0,
            right: width,
            bottom: height,
        });
    }

    /// WebView2 draws in its own process; give it a few compositor frames.
    unsafe fn let_webview_draw() {
        for _ in 0..3 {
            let _ = DwmFlush();
        }
    }

    unsafe fn client_size(hwnd: HWND) -> (i32, i32) {
        let mut rc = RECT::default();
        let _ = GetClientRect(hwnd, &mut rc);
        (rc.right - rc.left, rc.bottom - rc.top)
    }

    /// Paints the window's background into newly exposed area now, before
    /// the compositor shows it.
    unsafe fn repaint(hwnd: HWND) {
        let _ = RedrawWindow(
            Some(hwnd),
            None,
            None,
            RDW_ERASE | RDW_INVALIDATE | RDW_ERASENOW | RDW_UPDATENOW,
        );
    }

    unsafe fn enter(hwnd: HWND, controller: &ICoreWebView2Controller) -> Saved {
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
        let mut rect = RECT::default();
        let _ = GetWindowRect(hwnd, &mut rect);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let _ = GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut info);
        let monitor = info.rcMonitor;
        let (width, height) = (monitor.right - monitor.left, monitor.bottom - monitor.top);

        size_webview(controller, width, height);
        let_webview_draw();

        let bare = (style as u32) & !(WS_CAPTION.0 | WS_THICKFRAME.0);
        SetWindowLongPtrW(hwnd, GWL_STYLE, bare as isize);
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOP),
            monitor.left,
            monitor.top,
            width,
            height,
            SWP_FRAMECHANGED | SWP_NOCOPYBITS | SWP_NOOWNERZORDER,
        );
        size_webview(controller, width, height);
        repaint(hwnd);
        Saved { style, rect }
    }

    unsafe fn exit(hwnd: HWND, controller: &ICoreWebView2Controller, saved: &Saved) {
        // Shrinking needs no head start: the page is only clipped, never
        // short of the window.
        SetWindowLongPtrW(hwnd, GWL_STYLE, saved.style);
        let r = saved.rect;
        let _ = SetWindowPos(
            hwnd,
            None,
            r.left,
            r.top,
            r.right - r.left,
            r.bottom - r.top,
            SWP_FRAMECHANGED | SWP_NOCOPYBITS | SWP_NOZORDER | SWP_NOACTIVATE,
        );
        let (width, height) = client_size(hwnd);
        size_webview(controller, width, height);
        repaint(hwnd);
    }
}

#[cfg(not(windows))]
mod imp {
    use std::sync::Mutex;
    use tauri::webview::PlatformWebview;

    pub struct Saved;

    pub fn apply(
        window: &tauri::WebviewWindow,
        _webview: &PlatformWebview,
        _saved: &Mutex<Option<Saved>>,
        on: bool,
    ) -> Result<(), String> {
        window.set_fullscreen(on).map_err(|error| error.to_string())
    }
}
