use serde::{Deserialize, Serialize};
use tauri::{PhysicalPosition, PhysicalSize, WebviewWindow, Window};

const FILE_NAME: &str = "window-state.json";

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct SavedWindowState {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub maximized: bool,
}

impl Default for SavedWindowState {
    fn default() -> Self {
        Self {
            x: 0,
            y: 0,
            width: 1280,
            height: 800,
            maximized: false,
        }
    }
}

pub fn load() -> Option<SavedWindowState> {
    let bytes = std::fs::read(crate::paths::data_dir().join(FILE_NAME)).ok()?;
    let state: SavedWindowState = serde_json::from_slice(&bytes).ok()?;
    (state.width >= 320 && state.height >= 240 && state.width <= 16384 && state.height <= 16384)
        .then_some(state)
}

pub fn save(state: SavedWindowState) {
    let path = crate::paths::data_dir().join(FILE_NAME);
    if let Ok(bytes) = serde_json::to_vec(&state) {
        if let Err(error) = std::fs::write(path, bytes) {
            eprintln!("window state could not be saved: {error}");
        }
    }
}

fn intersection_area(a: SavedWindowState, x: i32, y: i32, width: u32, height: u32) -> i64 {
    let left = i64::from(a.x).max(i64::from(x));
    let top = i64::from(a.y).max(i64::from(y));
    let right = (i64::from(a.x) + i64::from(a.width)).min(i64::from(x) + i64::from(width));
    let bottom = (i64::from(a.y) + i64::from(a.height)).min(i64::from(y) + i64::from(height));
    (right - left).max(0) * (bottom - top).max(0)
}

fn clamp_to_work_area(
    state: SavedWindowState,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    frame_width: u32,
    frame_height: u32,
) -> SavedWindowState {
    let bounded_width = state.width.min(width.saturating_sub(frame_width).max(1));
    let bounded_height = state.height.min(height.saturating_sub(frame_height).max(1));
    let max_x = (i64::from(x) + i64::from(width.max(1)) - i64::from(bounded_width + frame_width))
        .max(i64::from(x));
    let max_y = (i64::from(y) + i64::from(height.max(1))
        - i64::from(bounded_height + frame_height))
    .max(i64::from(y));
    SavedWindowState {
        x: i64::from(state.x).clamp(i64::from(x), max_x) as i32,
        y: i64::from(state.y).clamp(i64::from(y), max_y) as i32,
        width: bounded_width,
        height: bounded_height,
        maximized: state.maximized,
    }
}

pub fn restore(window: &WebviewWindow, saved: Option<SavedWindowState>) {
    let monitors = window.available_monitors().unwrap_or_default();
    let primary = window.primary_monitor().ok().flatten();
    let frame = window
        .outer_size()
        .ok()
        .zip(window.inner_size().ok())
        .map(|(outer, inner)| {
            (
                outer.width.saturating_sub(inner.width),
                outer.height.saturating_sub(inner.height),
            )
        })
        .unwrap_or((0, 0));
    let preferred = saved.unwrap_or_else(|| {
        let size = window.inner_size().unwrap_or(PhysicalSize::new(1280, 800));
        SavedWindowState {
            width: size.width,
            height: size.height,
            ..SavedWindowState::default()
        }
    });
    let saved_monitor = monitors
        .iter()
        .max_by_key(|monitor| {
            let area = monitor.work_area();
            intersection_area(
                preferred,
                area.position.x,
                area.position.y,
                area.size.width,
                area.size.height,
            )
        })
        .filter(|monitor| {
            let area = monitor.work_area();
            intersection_area(
                preferred,
                area.position.x,
                area.position.y,
                area.size.width,
                area.size.height,
            ) > 0
        });
    let monitor = saved_monitor
        .filter(|_| saved.is_some())
        .or(primary.as_ref())
        .or_else(|| monitors.first());

    if let Some(monitor) = monitor {
        let area = monitor.work_area();
        let state = if saved.is_some() {
            clamp_to_work_area(
                preferred,
                area.position.x,
                area.position.y,
                area.size.width,
                area.size.height,
                frame.0,
                frame.1,
            )
        } else {
            let width = preferred
                .width
                .min(area.size.width.saturating_sub(frame.0).max(1));
            let height = preferred
                .height
                .min(area.size.height.saturating_sub(frame.1).max(1));
            SavedWindowState {
                x: area.position.x + (area.size.width.saturating_sub(width + frame.0) / 2) as i32,
                y: area.position.y + (area.size.height.saturating_sub(height + frame.1) / 2) as i32,
                width,
                height,
                maximized: false,
            }
        };
        let _ = window.set_size(PhysicalSize::new(state.width, state.height));
        let _ = window.set_position(PhysicalPosition::new(state.x, state.y));
        if state.maximized {
            let _ = window.maximize();
        }
    }
}

pub fn observe(window: &Window, state: &mut SavedWindowState) {
    let maximized = window.is_maximized().unwrap_or(state.maximized);
    state.maximized = maximized;
    if maximized {
        return;
    }
    if let (Ok(position), Ok(size)) = (window.outer_position(), window.inner_size()) {
        state.x = position.x;
        state.y = position.y;
        state.width = size.width;
        state.height = size.height;
    }
}

#[cfg(test)]
mod tests {
    use super::{clamp_to_work_area, SavedWindowState};

    #[test]
    fn removed_monitor_returns_window_to_work_area() {
        let previous = SavedWindowState {
            x: 3500,
            y: 500,
            width: 1800,
            height: 1200,
            maximized: false,
        };
        let restored = clamp_to_work_area(previous, 0, 0, 1920, 1040, 16, 40);
        assert_eq!(
            (restored.x, restored.y, restored.width, restored.height),
            (104, 0, 1800, 1000)
        );
    }
}
