//! The voice filter-chain process: a `pipewire -c` client that hosts the
//! DSP graph natively, so processing never passes through omalive.
use anyhow::{Context, Result};
use omalive_core::{Plugins, VoiceSettings, voice};
use std::{
    io::{Read, Seek},
    path::{Path, PathBuf},
    process::{Child, Stdio},
};

pub struct Chain {
    pub child: Child,
    folder: PathBuf,
    log: std::fs::File,
}

impl Chain {
    pub fn spawn(settings: &VoiceSettings, plugins: &Plugins) -> Result<Self> {
        let configuration = voice::configuration(settings, plugins)?;
        let base = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let folder = base.join(format!("omalive-voice-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(&folder)
                .with_context(|| format!("Creating {}", folder.display()))?;
        }
        let conf = folder.join("voice.conf");
        std::fs::write(&conf, voice::render(&configuration)?)?;
        let log = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(folder.join("pipewire.log"))?;
        let child = command(&conf)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(log.try_clone()?)
            .spawn()
            .context("Starting the voice engine (is pipewire installed?)")?;
        Ok(Self { child, folder, log })
    }

    /// The last error the chain logged, prefixed with a space, or nothing.
    pub fn error(&mut self) -> String {
        let mut text = String::new();
        let _ = self.log.rewind();
        let _ = self.log.read_to_string(&mut text);
        last_error(&text)
    }
}

impl Drop for Chain {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.folder);
    }
}

/// setpriv makes the kernel stop the chain if omalive itself dies, so a
/// crash can never leave an orphaned virtual microphone behind.
fn command(conf: &Path) -> std::process::Command {
    let setpriv = ["/usr/bin/setpriv", "/bin/setpriv"]
        .into_iter()
        .find(|path| Path::new(path).exists());
    let mut command = match setpriv {
        Some(setpriv) => {
            let mut command = std::process::Command::new(setpriv);
            command.args(["--pdeathsig", "TERM", "--", "pipewire", "-c"]);
            command
        }
        None => {
            let mut command = std::process::Command::new("pipewire");
            command.arg("-c");
            command
        }
    };
    command.arg(conf);
    command
}

fn last_error(log: &str) -> String {
    log.lines()
        .rev()
        .find(|line| line.contains("[E]") || line.to_lowercase().contains("error"))
        .map(|line| format!(" {}", line.trim().chars().take(300).collect::<String>()))
        .unwrap_or_default()
}

/// Mirror a live control change into the settings it came from.
pub fn apply_control(settings: &mut VoiceSettings, name: &str, value: f32) {
    let key = match name {
        "rumble:Freq" => "highpass",
        "low:Gain" => "low",
        "mid:Gain" => "mid",
        "high:Gain" => "high",
        "denoise:VAD Threshold (%)" => "vad",
        "compress:Threshold level (dB)" => "threshold",
        "compress:Ratio (1:n)" => "ratio",
        "compress:Attack time (ms)" => "attack",
        "compress:Release time (ms)" => "release",
        "compress:Makeup gain (dB)" => "makeup",
        "gain:Mult" => {
            settings.set("output", 20. * value.max(1e-9).log10());
            return;
        }
        _ => return,
    };
    settings.set(key, value);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_live_control_round_trips() {
        let source = VoiceSettings {
            low: 4.5,
            mid: -2.,
            high: 1.5,
            highpass: 120.,
            vad: 70.,
            threshold: -24.,
            ratio: 4.,
            attack: 20.,
            release: 300.,
            makeup: 6.,
            output: -6.,
            ..Default::default()
        };
        let mut mirrored = VoiceSettings::default();
        for (name, value) in voice::live_controls(&source) {
            apply_control(&mut mirrored, &name, value);
        }
        for (key, _) in omalive_core::settings::VOICE_RANGES {
            assert!((mirrored.get(key) - source.get(key)).abs() < 0.01, "{key}");
        }
    }

    #[test]
    fn command_uses_setpriv_when_available() {
        let command = command(Path::new("/tmp/voice.conf"));
        let args: Vec<_> = command
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args.last().unwrap(), "/tmp/voice.conf");
        if Path::new("/usr/bin/setpriv").exists() {
            assert_eq!(&args[..2], ["--pdeathsig", "TERM"]);
        }
    }

    #[test]
    fn last_error_line_is_reported() {
        let log = "[I] fine\n[E] mod.filter-chain | cannot load plugin\n[W] meh\n";
        assert_eq!(
            last_error(log),
            " [E] mod.filter-chain | cannot load plugin"
        );
        assert_eq!(last_error("[I] ok"), "");
    }
}
