use std::path::{Path, PathBuf};

pub fn resolve_db_path_from_env() -> PathBuf {
    let explicit = std::env::var("NOOR_DB").ok().map(PathBuf::from);
    let data_dir = std::env::var("NOOR_DATA_DIR").ok().map(PathBuf::from);
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."));
    resolve_db_path(explicit, data_dir, &exe_dir)
}

pub fn resolve_db_path(
    explicit: Option<PathBuf>,
    data_dir: Option<PathBuf>,
    exe_dir: &Path,
) -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    resolve_db_path_with_cwd(explicit, data_dir, exe_dir, &cwd)
}

pub fn resolve_db_path_with_cwd(
    explicit: Option<PathBuf>,
    data_dir: Option<PathBuf>,
    exe_dir: &Path,
    cwd: &Path,
) -> PathBuf {
    if let Some(path) = explicit {
        return absolutize(path, cwd);
    }

    if let Some(dir) = data_dir {
        return absolutize(dir, cwd).join("noor.db");
    }

    dev_root_for_exe_dir(exe_dir)
        .map(|root| root.join("noor.db"))
        .unwrap_or_else(|| exe_dir.join("noor.db"))
}

fn absolutize(path: PathBuf, cwd: &Path) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    }
}

fn dev_root_for_exe_dir(exe_dir: &Path) -> Option<PathBuf> {
    let profile = exe_dir.file_name()?.to_str()?;
    if profile != "debug" && profile != "release" {
        return None;
    }
    let target = exe_dir.parent()?;
    if target.file_name()?.to_str()? != "target" {
        return None;
    }
    target.parent().map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    #[test]
    fn db_path_prefers_explicit_db_then_data_dir_then_dev_then_exe_adjacent() {
        let root = std::env::current_dir().unwrap().join("workspace-root");
        let exe_dir = root.join("target").join("release");
        let explicit = std::env::current_dir()
            .unwrap()
            .join("custom")
            .join("noor-test.db");
        let data_dir = std::env::current_dir()
            .unwrap()
            .join("local-data")
            .join("NOORwave");

        let db = super::resolve_db_path(Some(explicit.clone()), Some(data_dir.clone()), &exe_dir);
        assert_eq!(db, explicit);

        let db = super::resolve_db_path(None, Some(data_dir.clone()), &exe_dir);
        assert_eq!(db, data_dir.join("noor.db"));

        let db = super::resolve_db_path(None, None, &exe_dir);
        assert_eq!(db, root.join("noor.db"));

        let installed_exe_dir = std::env::current_dir()
            .unwrap()
            .join("Programs")
            .join("NOORwave");
        let db = super::resolve_db_path(None, None, &installed_exe_dir);
        assert_eq!(db, installed_exe_dir.join("noor.db"));
    }

    #[test]
    fn relative_explicit_db_is_resolved_against_current_dir() {
        let cwd = std::env::current_dir().unwrap().join("workspace-root");
        let exe_dir = cwd.join("target").join("release");
        let db =
            super::resolve_db_path_with_cwd(Some(PathBuf::from("local.db")), None, &exe_dir, &cwd);
        assert_eq!(db, cwd.join("local.db"));
    }
}
