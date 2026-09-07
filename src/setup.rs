// SPDX-License-Identifier: GPL-3.0-or-later

//! Interactive machine-specific keyboard enrollment.
//!
//! Setup deliberately observes evdev passively, just like the daemon. It
//! discovers the actual Hyper and command-key streams from physical presses,
//! resolves the volatile `/dev/input/eventN` paths to stable
//! `/dev/input/by-id/` symlinks, verifies the complete Hyper+A chord using only
//! those stable paths, and then atomically persists machine configuration.

use super::FileConfig;
use anyhow::{bail, Context, Result};
use evdev::{Device, EventSummary, KeyCode};
use std::collections::HashSet;
use std::env;
use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;

const INPUT_BY_ID: &str = "/dev/input/by-id";

const BOLD_CYAN: &str = "1;36";
const BOLD_GREEN: &str = "1;32";
const BOLD_YELLOW: &str = "1;33";
const BOLD_MAGENTA: &str = "1;35";
const DIM: &str = "2";

#[derive(Debug)]
struct ObservedKeyPress {
    device_path: PathBuf,
    device_name: String,
    key: KeyCode,
}

#[derive(Debug)]
struct VerificationEvent {
    device_path: PathBuf,
    key: KeyCode,
    value: i32,
}

#[derive(Debug, Default)]
struct VerificationState {
    hyper_devices: HashSet<PathBuf>,
    saw_a_press: bool,
    saw_a_release: bool,
}

impl VerificationState {
    fn observe(&mut self, event: VerificationEvent, hyper_key: KeyCode) -> bool {
        if event.key == hyper_key {
            match event.value {
                1 => {
                    self.hyper_devices.insert(event.device_path);
                }
                0 => {
                    self.hyper_devices.remove(&event.device_path);
                    if self.hyper_devices.is_empty() {
                        if self.saw_a_press && self.saw_a_release {
                            return true;
                        }

                        // Hyper was released before a complete A press/release.
                        // Reset so the user can simply try the chord again.
                        self.saw_a_press = false;
                        self.saw_a_release = false;
                    }
                }
                _ => {}
            }

            return false;
        }

        if event.key == KeyCode::KEY_A {
            match event.value {
                1 if !self.hyper_devices.is_empty() => {
                    self.saw_a_press = true;
                    self.saw_a_release = false;
                }
                0 if self.saw_a_press && !self.hyper_devices.is_empty() => {
                    self.saw_a_release = true;
                }
                _ => {}
            }
        }

        false
    }
}

pub fn run() -> Result<()> {
    let candidate_count = readable_key_devices().len();

    if candidate_count == 0 {
        bail!("no readable evdev devices that report key events were found");
    }

    println!();
    println!("{}", paint("◆ hyperkeyd setup", BOLD_CYAN));
    println!(
        "{}",
        paint(
            format!("  Found {candidate_count} readable evdev key-event stream(s)."),
            DIM,
        )
    );

    ui_section("1/3", "Choose the Hyper key");
    ui_prompt("Press and release the physical key you want to use as Hyper.", "HYPER")?;

    let hyper = observe_press(None)?;

    println!();
    ui_success("Hyper key detected.");
    print_observation(&hyper);

    ui_section("2/3", "Find the command-key stream");
    ui_prompt("Press and release the physical A key.", "A")?;

    let command = observe_press(Some(KeyCode::KEY_A))?;

    println!();
    ui_success("Command-key stream detected.");
    print_observation(&command);

    println!();
    if hyper.device_path == command.device_path {
        ui_status("Topology", "Hyper and command keys share one evdev stream.");
    } else {
        ui_status("Topology", "Hyper and command keys use separate evdev streams.");
    }

    let hyper_stable = stable_by_id_path(&hyper.device_path)?;
    let command_stable = stable_by_id_path(&command.device_path)?;

    println!();
    println!("{}", paint("Stable device paths", BOLD_CYAN));
    println!("  Hyper   → {}", hyper_stable.display());
    println!("  Command → {}", command_stable.display());

    let mut devices = vec![hyper_stable];
    if command_stable != devices[0] {
        devices.push(command_stable);
    }

    let config = FileConfig {
        hyper_key: config_key_name(hyper.key),
        devices: devices.clone(),
        script_dir: None,
    };
    let preview = toml::to_string_pretty(&config)
        .context("failed to serialize enrollment configuration")?;

    println!();
    println!("{}", paint("Configuration preview", BOLD_CYAN));
    println!("{}", paint("────────────────────────────────────────", DIM));
    print!("{preview}");
    println!("{}", paint("────────────────────────────────────────", DIM));

    let config_path = default_config_path()?;

    ui_section("3/3", "Verify the enrollment");
    println!(
        "  On success, configuration will be written to:\n  {}",
        paint(config_path.display().to_string(), BOLD_MAGENTA)
    );
    println!();
    println!("  Perform this chord in order:");
    println!("    1. Hold {}", paint("HYPER", BOLD_MAGENTA));
    println!("    2. Press and release {}", paint("A", BOLD_MAGENTA));
    println!("    3. Release {}", paint("HYPER", BOLD_MAGENTA));
    println!();
    println!("{}", paint("▶ ACTION REQUIRED", BOLD_YELLOW));
    println!("  Waiting for the complete Hyper+A chord…");
    io::stdout()
        .flush()
        .context("failed to flush setup verification prompt")?;

    verify_hyper_a(&devices, hyper.key)?;

    println!();
    ui_success("Verification succeeded — complete Hyper+A chord observed.");

    write_config_atomic(&config_path, &config)?;

    println!();
    println!("{}", paint("✓ Setup complete", BOLD_GREEN));
    println!("  Configuration written atomically to:");
    println!("  {}", paint(config_path.display().to_string(), BOLD_MAGENTA));
    println!();

    Ok(())
}

fn observe_press(expected_key: Option<KeyCode>) -> Result<ObservedKeyPress> {
    let devices = readable_key_devices();

    if devices.is_empty() {
        bail!("no readable evdev devices that report key events were found");
    }

    let (tx, rx) = mpsc::channel::<ObservedKeyPress>();

    for (path, mut device) in devices {
        let tx = tx.clone();
        let device_name = device.name().unwrap_or("unnamed device").to_string();

        thread::spawn(move || loop {
            let events = match device.fetch_events() {
                Ok(events) => events,
                Err(_) => return,
            };

            for event in events {
                let EventSummary::Key(_, key, 1) = event.destructure() else {
                    continue;
                };

                if expected_key.is_some_and(|expected| key != expected) {
                    continue;
                }

                let _ = tx.send(ObservedKeyPress {
                    device_path: path,
                    device_name,
                    key,
                });
                return;
            }
        });
    }

    drop(tx);

    rx.recv()
        .context("all candidate input streams closed before the requested key press was observed")
}

fn verify_hyper_a(paths: &[PathBuf], hyper_key: KeyCode) -> Result<()> {
    let mut opened = Vec::new();
    for path in paths {
        let device = Device::open(path)
            .with_context(|| format!("failed to open stable enrollment device {}", path.display()))?;
        opened.push((path.clone(), device));
    }

    let (tx, rx) = mpsc::channel::<VerificationEvent>();

    for (path, mut device) in opened {
        let tx = tx.clone();
        thread::spawn(move || loop {
            let events = match device.fetch_events() {
                Ok(events) => events,
                Err(_) => return,
            };

            for event in events {
                if let EventSummary::Key(_, key, value) = event.destructure() {
                    if tx
                        .send(VerificationEvent {
                            device_path: path.clone(),
                            key,
                            value,
                        })
                        .is_err()
                    {
                        return;
                    }
                }
            }
        });
    }

    drop(tx);

    let mut state = VerificationState::default();
    while let Ok(event) = rx.recv() {
        if state.observe(event, hyper_key) {
            return Ok(());
        }
    }

    bail!("all selected stable input streams closed before Hyper+A verification succeeded")
}

fn print_observation(observed: &ObservedKeyPress) {
    println!("  device  → {}", observed.device_path.display());
    println!("  name    → {}", observed.device_name);
    println!("  keycode → {}", observed.key.code());
    println!("  evdev   → {:?}", observed.key);
}

fn readable_key_devices() -> Vec<(PathBuf, Device)> {
    let mut devices = evdev::enumerate()
        .filter(|(_, device)| device.supported_keys().is_some())
        .collect::<Vec<_>>();

    devices.sort_by(|(left, _), (right, _)| left.cmp(right));
    devices
}

fn stable_by_id_path(event_path: &Path) -> Result<PathBuf> {
    let event_target = fs::canonicalize(event_path)
        .with_context(|| format!("failed to resolve observed device {}", event_path.display()))?;

    let entries = fs::read_dir(INPUT_BY_ID)
        .with_context(|| format!("failed to read stable input-device directory {INPUT_BY_ID}"))?;

    let mut matches = Vec::new();
    for entry in entries {
        let entry = entry.with_context(|| format!("failed to inspect {INPUT_BY_ID}"))?;
        let path = entry.path();

        let Ok(target) = fs::canonicalize(&path) else {
            continue;
        };

        if target == event_target {
            matches.push(path);
        }
    }

    matches.sort();
    matches.dedup();

    match matches.as_slice() {
        [stable] => Ok(stable.clone()),
        [] => bail!(
            "observed device {} has no stable /dev/input/by-id alias; refusing to persist volatile event path",
            event_path.display()
        ),
        _ => {
            let choices = matches
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            bail!(
                "observed device {} has multiple /dev/input/by-id aliases ({choices}); refusing to choose silently",
                event_path.display()
            )
        }
    }
}

fn config_key_name(key: KeyCode) -> String {
    const KEY_MACRO1_CODE: u16 = 0x290;
    const KEY_MACRO30_CODE: u16 = 0x2ad;

    let code = key.code();
    if (KEY_MACRO1_CODE..=KEY_MACRO30_CODE).contains(&code) {
        return format!("KEY_MACRO{}", code - KEY_MACRO1_CODE + 1);
    }

    let debug_name = format!("{key:?}");
    if debug_name.starts_with("unknown key:") {
        code.to_string()
    } else {
        debug_name
    }
}

fn default_config_path() -> Result<PathBuf> {
    let home = env::var_os("HOME")
        .context("HOME is not set; cannot determine default hyperkeyd config path")?;
    Ok(PathBuf::from(home).join(".config/hyperkeyd/config.toml"))
}

fn write_config_atomic(path: &Path, config: &FileConfig) -> Result<()> {
    let parent = path
        .parent()
        .context("configuration path has no parent directory")?;
    fs::create_dir_all(parent)
        .with_context(|| format!("failed to create config directory {}", parent.display()))?;

    let contents = toml::to_string_pretty(config)
        .context("failed to serialize enrollment configuration")?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("configuration path does not have a valid UTF-8 file name")?;
    let temp_path = parent.join(format!(".{file_name}.tmp-{}", std::process::id()));

    fs::write(&temp_path, contents)
        .with_context(|| format!("failed to write temporary config {}", temp_path.display()))?;

    if let Err(err) = fs::rename(&temp_path, path) {
        let _ = fs::remove_file(&temp_path);
        bail!("failed to install config {}: {err}", path.display());
    }

    Ok(())
}

fn ui_section(step: &str, title: &str) {
    println!();
    println!(
        "{}",
        paint(format!("━━━ {step} · {title} ━━━━━━━━━━━━━━━━━━━━━━━"), BOLD_CYAN)
    );
}

fn ui_prompt(message: &str, key: &str) -> Result<()> {
    println!();
    println!("{}", paint("▶ ACTION REQUIRED", BOLD_YELLOW));
    println!("  {message}");
    println!("  Expected key: {}", paint(key, BOLD_MAGENTA));
    println!("  {}", paint("Waiting for your keypress…", DIM));
    io::stdout()
        .flush()
        .context("failed to flush setup key prompt")?;
    Ok(())
}

fn ui_success(message: &str) {
    println!("{} {message}", paint("✓", BOLD_GREEN));
}

fn ui_status(label: &str, message: &str) {
    println!("{}: {message}", paint(label, BOLD_CYAN));
}

fn paint(text: impl AsRef<str>, ansi: &str) -> String {
    let text = text.as_ref();
    if color_enabled() {
        format!("\x1b[{ansi}m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

fn color_enabled() -> bool {
    env::var_os("NO_COLOR").is_none() && io::stdout().is_terminal()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn verification_event(path: &str, key: KeyCode, value: i32) -> VerificationEvent {
        VerificationEvent {
            device_path: PathBuf::from(path),
            key,
            value,
        }
    }

    #[test]
    fn config_key_name_preserves_kernel_macro_names() {
        assert_eq!(config_key_name(KeyCode::new(0x290)), "KEY_MACRO1");
        assert_eq!(config_key_name(KeyCode::new(0x29a)), "KEY_MACRO11");
        assert_eq!(config_key_name(KeyCode::new(0x2ad)), "KEY_MACRO30");
    }

    #[test]
    fn config_key_name_uses_evdev_names_when_available() {
        assert_eq!(config_key_name(KeyCode::KEY_A), "KEY_A");
        assert_eq!(config_key_name(KeyCode::KEY_F24), "KEY_F24");
    }

    #[test]
    fn verification_requires_complete_chord_lifecycle() {
        let hyper = KeyCode::KEY_F24;
        let mut state = VerificationState::default();

        assert!(!state.observe(verification_event("/hyper", hyper, 1), hyper));
        assert!(!state.observe(verification_event("/command", KeyCode::KEY_A, 1), hyper));
        assert!(!state.observe(verification_event("/command", KeyCode::KEY_A, 0), hyper));
        assert!(state.observe(verification_event("/hyper", hyper, 0), hyper));
    }

    #[test]
    fn verification_resets_after_premature_hyper_release() {
        let hyper = KeyCode::KEY_F24;
        let mut state = VerificationState::default();

        assert!(!state.observe(verification_event("/hyper", hyper, 1), hyper));
        assert!(!state.observe(verification_event("/command", KeyCode::KEY_A, 1), hyper));
        assert!(!state.observe(verification_event("/hyper", hyper, 0), hyper));
        assert!(!state.observe(verification_event("/command", KeyCode::KEY_A, 0), hyper));

        assert!(!state.observe(verification_event("/hyper", hyper, 1), hyper));
        assert!(!state.observe(verification_event("/command", KeyCode::KEY_A, 1), hyper));
        assert!(!state.observe(verification_event("/command", KeyCode::KEY_A, 0), hyper));
        assert!(state.observe(verification_event("/hyper", hyper, 0), hyper));
    }
}
