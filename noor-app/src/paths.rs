use std::path::{Path, PathBuf};

pub fn is_installed_mode() -> bool {
    if cfg!(debug_assertions) {
        return false;
    }
    let Some(exe_parent) = exe_parent() else {
        return false;
    };
    is_installed_mode_for(
        exe_parent.join("uninstall.exe").is_file(),
        exe_parent.join("www").is_dir(),
        has_dev_frontend_build(&exe_parent),
    )
}

pub fn is_installed_mode_for(
    has_uninstaller: bool,
    has_adjacent_www: bool,
    has_dev_frontend_build: bool,
) -> bool {
    if has_uninstaller {
        return true;
    }
    !has_adjacent_www && !has_dev_frontend_build
}

pub fn data_dir() -> PathBuf {
    let exe_dir = exe_parent().unwrap_or_else(|| PathBuf::from("."));
    let local_app_data = std::env::var("LOCALAPPDATA").ok().map(PathBuf::from);
    let dir = data_dir_for(is_installed_mode(), &exe_dir, local_app_data.as_deref());
    if is_installed_mode() {
        let _ = std::fs::create_dir_all(&dir);
    }
    dir
}

pub fn data_dir_for(installed: bool, exe_dir: &Path, local_app_data: Option<&Path>) -> PathBuf {
    if installed {
        if let Some(base) = local_app_data {
            return base.join("NOORwave");
        }
        eprintln!("warning: LOCALAPPDATA unset in installed mode; using exe-adjacent data dir");
    }
    exe_dir.to_path_buf()
}

fn exe_parent() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
}

fn has_dev_frontend_build(exe_parent: &Path) -> bool {
    let mut cursor = Some(exe_parent);
    while let Some(dir) = cursor {
        if dir.join("frontend").join("build").is_dir() {
            return true;
        }
        cursor = dir.parent();
    }
    false
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    #[test]
    fn installed_mode_is_false_for_portable_and_dev_layouts() {
        assert!(!super::is_installed_mode_for(false, true, false));
        assert!(!super::is_installed_mode_for(false, false, true));
        assert!(super::is_installed_mode_for(false, false, false));
        assert!(super::is_installed_mode_for(true, true, false));
    }

    #[test]
    fn installed_data_dir_uses_local_app_data_when_present() {
        let exe_dir = PathBuf::from(r"C:\Users\Felix\AppData\Local\Programs\NOORwave");
        let local_app_data = PathBuf::from(r"C:\Users\Felix\AppData\Local");

        let data_dir = super::data_dir_for(true, &exe_dir, Some(&local_app_data));
        assert_eq!(data_dir, local_app_data.join("NOORwave"));

        let data_dir = super::data_dir_for(false, &exe_dir, Some(&local_app_data));
        assert_eq!(data_dir, exe_dir);
    }
}
