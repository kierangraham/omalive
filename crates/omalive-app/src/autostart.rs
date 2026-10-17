//! "Start at login": an XDG autostart entry that launches omalive in the
//! background. uwsm (Omarchy's session manager) runs these at login.
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

pub fn dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::home_dir().map(|home| home.join(".config")))
        .unwrap_or_else(|| PathBuf::from(".config"))
        .join("autostart")
}

/// Add or remove the entry in `dir`, launching `binary`.
pub fn set(dir: &Path, enabled: bool, binary: &Path) -> Result<()> {
    let path = dir.join("omalive.desktop");
    if !enabled {
        return match std::fs::remove_file(&path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                Err(error).with_context(|| format!("Removing {}", path.display()))
            }
            _ => Ok(()),
        };
    }
    std::fs::create_dir_all(dir).with_context(|| format!("Creating {}", dir.display()))?;
    // The session's PATH may not include ~/.local/bin, so use the full path.
    let entry = format!(
        "[Desktop Entry]\nType=Application\nName=omalive\nComment=Voice and camera processing for calls, in the background\nExec=\"{}\" --background\nIcon=omalive\nTerminal=false\nX-GNOME-Autostart-enabled=true\n",
        binary.display()
    );
    std::fs::write(&path, entry).with_context(|| format!("Writing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_is_written_and_removed() {
        let dir = tempfile::tempdir().unwrap();
        let autostart = dir.path().join("autostart");
        let binary = Path::new("/home/me/.local/bin/omalive");
        set(&autostart, true, binary).unwrap();
        let entry = std::fs::read_to_string(autostart.join("omalive.desktop")).unwrap();
        assert!(entry.contains("Exec=\"/home/me/.local/bin/omalive\" --background"));
        set(&autostart, false, binary).unwrap();
        assert!(!autostart.join("omalive.desktop").exists());
        // Removing twice is fine.
        set(&autostart, false, binary).unwrap();
    }
}
