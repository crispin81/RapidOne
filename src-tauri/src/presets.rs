//! RapidOne's own presets: one JSON file each (`<name>.rpp`) in a `presets`
//! folder in the app's data folder, so every preset - saved from a photo or
//! imported (converted) from Lightroom - sits in the same place. The app
//! writes and reads the contents; this only stores them.

use std::path::{Path, PathBuf};
use tauri::Manager;

const EXTENSION: &str = "rpp";

fn folder(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?.join("presets");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    Ok(dir)
}

/// A file name for a preset name: characters Windows or Linux won't take are
/// replaced, so any name can be saved.
fn file_name(name: &str) -> String {
    let safe: String = name
        .trim()
        .chars()
        .map(|c| if c.is_control() || "<>:\"/\\|?*".contains(c) { '_' } else { c })
        .collect();
    format!("{}.{EXTENSION}", safe.trim_end_matches(['.', ' ']))
}

/// Every preset's contents (JSON).
pub fn list(app: &tauri::AppHandle) -> Result<Vec<String>, String> {
    let dir = folder(app)?;
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir).map_err(|e| e.to_string())?.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|x| x == EXTENSION) {
            if let Ok(text) = std::fs::read_to_string(&path) {
                out.push(text);
            }
        }
    }
    Ok(out)
}

/// Saves a preset under its name (the same name replaces it).
pub fn save(app: &tauri::AppHandle, name: &str, contents: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("a preset needs a name".into());
    }
    let path = folder(app)?.join(file_name(name));
    write(&path, contents)
}

pub fn delete(app: &tauri::AppHandle, name: &str) -> Result<(), String> {
    let path = folder(app)?.join(file_name(name));
    std::fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Copies every preset into `dest` (a backup), returning how many.
pub fn export(app: &tauri::AppHandle, dest: &Path) -> Result<usize, String> {
    let dir = folder(app)?;
    let mut n = 0;
    for entry in std::fs::read_dir(&dir).map_err(|e| e.to_string())?.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|x| x == EXTENSION) {
            std::fs::copy(&path, dest.join(entry.file_name())).map_err(|e| format!("{}: {e}", path.display()))?;
            n += 1;
        }
    }
    Ok(n)
}

fn write(path: &Path, contents: &str) -> Result<(), String> {
    let tmp = path.with_extension(format!("{EXTENSION}.tmp"));
    std::fs::write(&tmp, contents).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn any_name_makes_a_safe_file_name() {
        assert_eq!(file_name("Warm / Soft: 2"), "Warm _ Soft_ 2.rpp");
        assert_eq!(file_name("  Moody. "), "Moody.rpp");
    }
}
