// SPDX-License-Identifier: GPL-3.0-or-later

/*
 * hyperkeyd - Hyper Key Command Dispatcher
 *
 * Copyright 2026 W. D. Callahan II
 *
 * This program is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * This program is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with this program. If not, see <https://www.gnu.org/licenses/>.
 */

//! hyperkeyd: a small Hyper-key command dispatcher for Linux.
//!
//! This program implements the contract described by the Hyper Key Command
//! Dispatcher document:
//!
//! - When the configured Hyper key is down, the daemon is "armed".
//! - While armed, every alphanumeric key press is a complete command event.
//! - The command event launches the matching executable script from a script
//!   directory, such as `~/.hyper/a.sh` or `~/.hyper/1.sh`.
//! - Commands are launched immediately. There is no command buffer, no prefix
//!   grammar, no timeout, and no waiting for a possible second key.
//!
//! The implementation deliberately uses Linux evdev instead of X11 hotkey APIs.
//! That makes the event source independent of X11 and Wayland compositors, at
//! the cost of needing permission to read `/dev/input/event*` devices.

use anyhow::{bail, Context, Result};
use clap::Parser;
use evdev::{Device, EventSummary, KeyCode};
use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::str::FromStr;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use tracing::{debug, error, info, warn};
use tracing_subscriber::EnvFilter;

/// Command line arguments for the daemon.
///
/// The defaults are intentionally conservative:
///
/// - `KEY_F24` is a useful default Hyper trigger because many programmable
///   keyboards can emit it, and desktop environments usually do not bind it.
/// - `~/.hyper` is the default user-owned command directory.
/// - `.sh` is the default script extension because the design document used
///   examples such as `a.sh` and `1.sh`.
#[derive(Debug, Parser)]
#[command(author, version, about)]
struct Args {
    /// Print evdev input devices and exit.
    ///
    /// Use this first to find the keyboard device path to pass to --device.
    #[arg(long)]
    list_devices: bool,

    /// Evdev device path to read, such as /dev/input/event7.
    ///
    /// This option may be provided more than once. If several devices are
    /// supplied, Hyper state is shared across them. That means Hyper on one
    /// selected keyboard and `a` on another selected keyboard will dispatch
    /// `a.sh`. For most personal setups, passing one keyboard is clearest.
    #[arg(short, long, value_name = "PATH")]
    device: Vec<PathBuf>,

    /// Evdev key code that acts as the Hyper trigger.
    ///
    /// Accepts names such as KEY_F24, F24, KEY_LEFTMETA, LEFTMETA, CAPSLOCK,
    /// or numeric codes like 194. Numeric codes are provided mostly as an
    /// escape hatch for unusual keyboards.
    #[arg(long, default_value = "KEY_F24", value_parser = parse_key_code)]
    hyper_key: KeyCode,

    /// Directory containing command scripts.
    ///
    /// Defaults to ~/.hyper. A command key maps directly to a file inside this
    /// directory. With the default extension, Hyper+a launches ~/.hyper/a.sh.
    #[arg(long, value_name = "DIR")]
    script_dir: Option<PathBuf>,

    /// Script extension to append to command names.
    ///
    /// The default is "sh", producing names such as a.sh and 1.sh. Use an empty
    /// string to launch extensionless files such as ~/.hyper/a.
    #[arg(long, default_value = "sh")]
    extension: String,

    /// Log missing scripts instead of ignoring them silently.
    ///
    /// The dispatch model allows missing scripts. Silent ignore is the default
    /// because a user should not have to create all 36 possible commands.
    #[arg(long)]
    log_missing: bool,

    /// Suppress key-repeat dispatches.
    ///
    /// By default, evdev repeat events dispatch commands, matching the core
    /// design: recognized keydown while armed means dispatch. This flag is an
    /// optional implementation convenience, not part of the semantic contract.
    #[arg(long)]
    suppress_repeat: bool,

    /// Do not actually launch scripts; only log what would be launched.
    ///
    /// This is useful while selecting devices, choosing the Hyper key, or
    /// testing command mappings before allowing scripts to run.
    #[arg(long)]
    dry_run: bool,
}

/// Runtime configuration after command-line arguments have been normalized.
///
/// Keeping this separate from `Args` lets the rest of the program avoid
/// repeatedly handling optional/default values.
#[derive(Debug, Clone)]
struct Config {
    hyper_key: KeyCode,
    script_dir: PathBuf,
    extension: String,
    log_missing: bool,
    suppress_repeat: bool,
    dry_run: bool,
}

/// A normalized key event sent from device-listener threads to the dispatcher.
///
/// evdev provides many event types. This daemon only cares about key events,
/// so listener threads discard everything else before sending to the main loop.
#[derive(Debug, Clone)]
struct KeyEvent {
    device_path: PathBuf,
    key: KeyCode,
    value: KeyValue,
}

/// The only key values the dispatcher cares about.
///
/// Linux evdev key events use numeric values:
///
/// - 0: release
/// - 1: press
/// - 2: repeat
///
/// Other values are preserved for diagnostics instead of being silently folded
/// into one of the known categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeyValue {
    Release,
    Press,
    Repeat,
    Other(i32),
}

impl KeyValue {
    fn from_evdev_value(value: i32) -> Self {
        match value {
            0 => Self::Release,
            1 => Self::Press,
            2 => Self::Repeat,
            other => Self::Other(other),
        }
    }

    fn as_env_value(self) -> &'static str {
        match self {
            Self::Release => "release",
            Self::Press => "press",
            Self::Repeat => "repeat",
            Self::Other(_) => "other",
        }
    }
}

/// The complete semantic state machine for the daemon.
///
/// There are intentionally only two states:
///
/// - idle: Hyper is not down
/// - armed: Hyper is down
///
/// Everything else is ordinary event handling. There is no pending command
/// state, because commands are never prefixes and the daemon never waits to
/// see whether another key follows.
#[derive(Debug)]
struct Dispatcher {
    armed: bool,
    config: Config,
}

impl Dispatcher {
    fn new(config: Config) -> Self {
        Self {
            armed: false,
            config,
        }
    }

    /// Handle one normalized key event.
    ///
    /// This method is the heart of the daemon. If you are learning the program,
    /// start here: every input event is reduced to one of three possibilities:
    ///
    /// 1. It is the Hyper trigger key, so update armed/idle state.
    /// 2. It is a command key while armed, so dispatch a script.
    /// 3. It is irrelevant, so ignore it.
    fn handle_event(&mut self, event: KeyEvent) {
        if event.key == self.config.hyper_key {
            self.handle_hyper_event(event.value, &event.device_path);
            return;
        }

        if !self.armed {
            // While idle, ordinary keys are outside this daemon's domain.
            return;
        }

        if !self.should_dispatch_for_value(event.value) {
            return;
        }

        let Some(command_char) = command_char_for_key(event.key) else {
            debug!(
                key = ?event.key,
                device = %event.device_path.display(),
                "armed but key is not an alphanumeric command key"
            );
            return;
        };

        self.dispatch(command_char, event.value, &event.device_path);
    }

    /// Update the armed state when the configured Hyper key changes state.
    fn handle_hyper_event(&mut self, value: KeyValue, device_path: &Path) {
        match value {
            KeyValue::Press => {
                if !self.armed {
                    info!(device = %device_path.display(), "Hyper pressed; dispatcher armed");
                }
                self.armed = true;
            }
            KeyValue::Release => {
                if self.armed {
                    info!(device = %device_path.display(), "Hyper released; dispatcher idle");
                }
                self.armed = false;
            }
            KeyValue::Repeat => {
                // A repeated Hyper key does not change the state machine.
                debug!(device = %device_path.display(), "Hyper repeat ignored");
            }
            KeyValue::Other(raw) => {
                debug!(
                    raw_value = raw,
                    device = %device_path.display(),
                    "Hyper key produced an unrecognized evdev value"
                );
            }
        }
    }

    /// Decide whether a key event value counts as a command dispatch.
    ///
    /// Press always dispatches. Repeat dispatches by default because the core
    /// model says each recognized keydown event while armed is a command event.
    /// Release never dispatches.
    fn should_dispatch_for_value(&self, value: KeyValue) -> bool {
        match value {
            KeyValue::Press => true,
            KeyValue::Repeat => !self.config.suppress_repeat,
            KeyValue::Release | KeyValue::Other(_) => false,
        }
    }

    /// Launch the matching script for a command character.
    ///
    /// This function does not invoke a shell. The script path is executed
    /// directly. That avoids shell quoting bugs and avoids treating any input as
    /// a command string. The user's script may itself be a shell script, Python
    /// script, compiled binary, or anything else executable by the kernel.
    fn dispatch(&self, command_char: char, value: KeyValue, device_path: &Path) {
        let script_path = script_path_for(
            &self.config.script_dir,
            command_char,
            &self.config.extension,
        );

        match executable_status(&script_path) {
            Ok(ExecutableStatus::Executable) => {
                if self.config.dry_run {
                    info!(
                        key = %command_char,
                        script = %script_path.display(),
                        "dry run: would launch script"
                    );
                    return;
                }

                info!(
                    key = %command_char,
                    script = %script_path.display(),
                    "launching script"
                );

                let mut command = Command::new(&script_path);

                // A daemon should not let child processes accidentally read from
                // its stdin. stdout/stderr are inherited so that, under systemd,
                // script output goes to the journal.
                command.stdin(Stdio::null());

                // These environment variables are not required by the semantic
                // model, but they are useful for scripts that want context.
                command
                    .env("HYPERKEYD_KEY", command_char.to_string())
                    .env("HYPERKEYD_EVENT", value.as_env_value())
                    .env("HYPERKEYD_DEVICE", device_path.as_os_str());

                if let Err(err) = command.spawn() {
                    error!(
                        error = %err,
                        script = %script_path.display(),
                        "failed to launch script"
                    );
                }
            }
            Ok(ExecutableStatus::Missing) => {
                if self.config.log_missing {
                    info!(
                        key = %command_char,
                        script = %script_path.display(),
                        "script missing; ignoring command"
                    );
                }
            }
            Ok(ExecutableStatus::NotAFile) => {
                warn!(
                    key = %command_char,
                    script = %script_path.display(),
                    "command target exists but is not a regular file; ignoring"
                );
            }
            Ok(ExecutableStatus::NotExecutable) => {
                warn!(
                    key = %command_char,
                    script = %script_path.display(),
                    "command target exists but is not executable; ignoring"
                );
            }
            Err(err) => {
                warn!(
                    error = %err,
                    key = %command_char,
                    script = %script_path.display(),
                    "could not inspect command target; ignoring"
                );
            }
        }
    }
}

/// File-state result for a possible script path.
///
/// Separating these cases makes logging precise. "Missing" is normal. A
/// directory or a non-executable file at a command path is more suspicious and
/// worth warning about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExecutableStatus {
    Executable,
    Missing,
    NotAFile,
    NotExecutable,
}

fn main() -> Result<()> {
    init_logging();

    let args = Args::parse();

    if args.list_devices {
        list_devices();
        return Ok(());
    }

    if args.device.is_empty() {
        bail!("no input devices specified; run `hyperkeyd --list-devices`, then pass one or more --device PATH values");
    }

    let script_dir = match args.script_dir {
        Some(path) => path,
        None => default_script_dir()?,
    };

    let config = Config {
        hyper_key: args.hyper_key,
        script_dir,
        extension: normalize_extension(&args.extension),
        log_missing: args.log_missing,
        suppress_repeat: args.suppress_repeat,
        dry_run: args.dry_run,
    };

    info!(hyper_key = ?config.hyper_key, script_dir = %config.script_dir.display(), "starting hyperkeyd");

    let (tx, rx) = mpsc::channel::<KeyEvent>();
    let mut handles = Vec::new();

    for path in args.device {
        handles.push(spawn_device_listener(path, tx.clone()));
    }

    // Drop the main sender so the receiver closes if all listener threads exit.
    // Without this, rx.recv() would wait forever even after every device thread
    // had ended.
    drop(tx);

    run_dispatch_loop(config, rx);

    // If the dispatch loop exits, every listener has stopped. Joining gives the
    // threads a chance to finish cleanly before main returns.
    for handle in handles {
        if let Err(err) = handle.join() {
            error!(?err, "device listener thread panicked");
        }
    }

    warn!("all device listeners stopped; hyperkeyd exiting");
    Ok(())
}

/// Initialize logging.
///
/// `tracing_subscriber` writes to stdout/stderr. That is perfect for both
/// foreground testing and systemd service use, because systemd captures those
/// streams into the journal.
///
/// Set RUST_LOG for more detail, for example:
///
///     RUST_LOG=hyperkeyd=debug hyperkeyd --device /dev/input/event7 --dry-run
fn init_logging() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("hyperkeyd=info"));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();
}

/// The central receive loop.
///
/// Device threads do blocking reads from evdev and send normalized key events to
/// this loop. Keeping the state machine in one thread avoids locks: only this
/// function owns and mutates `Dispatcher::armed`.
fn run_dispatch_loop(config: Config, rx: Receiver<KeyEvent>) {
    let mut dispatcher = Dispatcher::new(config);

    while let Ok(event) = rx.recv() {
        dispatcher.handle_event(event);
    }
}

/// Start one listener thread for one evdev input device.
///
/// A thread-per-device model is deliberately simple. We do not need an async
/// runtime because the daemon watches only a small number of devices, and each
/// listener spends almost all of its time blocked in `fetch_events()`.
fn spawn_device_listener(path: PathBuf, tx: Sender<KeyEvent>) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        if let Err(err) = device_listener_loop(path.clone(), tx) {
            error!(device = %path.display(), error = %err, "device listener stopped");
        }
    })
}

fn device_listener_loop(path: PathBuf, tx: Sender<KeyEvent>) -> Result<()> {
    let mut device = Device::open(&path)
        .with_context(|| format!("failed to open evdev device {}", path.display()))?;

    let device_name = device.name().unwrap_or("unnamed device").to_string();
    info!(device = %path.display(), name = %device_name, "listening to input device");

    loop {
        let events = device
            .fetch_events()
            .with_context(|| format!("failed while reading events from {}", path.display()))?;

        for event in events {
            if let EventSummary::Key(_, key, raw_value) = event.destructure() {
                let key_event = KeyEvent {
                    device_path: path.clone(),
                    key,
                    value: KeyValue::from_evdev_value(raw_value),
                };

                if tx.send(key_event).is_err() {
                    // The receiver exits only when the daemon is shutting down.
                    // Once it is gone, this listener has no useful work left.
                    return Ok(());
                }
            }
        }
    }
}

/// Print evdev devices.
///
/// This is intentionally a diagnostic helper rather than auto-selection. The
/// first version of this daemon should make device choice explicit, because the
/// wrong input device can be confusing or unsafe.
fn list_devices() {
    let mut devices = evdev::enumerate().collect::<Vec<_>>();
    devices.sort_by(|(left, _), (right, _)| left.cmp(right));

    for (path, device) in devices {
        let name = device.name().unwrap_or("unnamed device");
        let looks_keyboard = device
            .supported_keys()
            .map(|keys| keys.contains(KeyCode::KEY_A) && keys.contains(KeyCode::KEY_Z))
            .unwrap_or(false);

        let marker = if looks_keyboard { "keyboard-ish" } else { "input" };
        println!("{}\t{}\t{}", path.display(), marker, name);
    }
}

/// Return the default script directory, `~/.hyper`.
///
/// We intentionally read HOME directly instead of adding a directory helper
/// dependency. For a user daemon on Linux, HOME should be present. If systemd is
/// used later, `%h` can be passed explicitly via `--script-dir` too.
fn default_script_dir() -> Result<PathBuf> {
    let home = env::var_os("HOME").context("HOME is not set; pass --script-dir explicitly")?;
    Ok(PathBuf::from(home).join(".hyper"))
}

/// Normalize the extension stored in configuration.
///
/// Users may reasonably type either `--extension sh` or `--extension .sh`.
/// Internally, we store it without the leading dot. An empty extension means
/// scripts are named `a`, `b`, `1`, etc., instead of `a.sh`, `b.sh`, `1.sh`.
fn normalize_extension(extension: &str) -> String {
    extension.trim_start_matches('.').to_string()
}

/// Build the script path for a command character.
fn script_path_for(script_dir: &Path, command_char: char, extension: &str) -> PathBuf {
    let file_name = if extension.is_empty() {
        command_char.to_string()
    } else {
        format!("{command_char}.{extension}")
    };

    script_dir.join(file_name)
}

/// Inspect whether a possible command target is launchable.
fn executable_status(path: &Path) -> Result<ExecutableStatus> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ExecutableStatus::Missing);
        }
        Err(err) => return Err(err).with_context(|| format!("metadata failed for {}", path.display())),
    };

    if !metadata.is_file() {
        return Ok(ExecutableStatus::NotAFile);
    }

    let mode = metadata.permissions().mode();
    if mode & 0o111 == 0 {
        return Ok(ExecutableStatus::NotExecutable);
    }

    Ok(ExecutableStatus::Executable)
}

/// Parse a key-code argument.
///
/// `evdev::KeyCode` implements FromStr for canonical names such as `KEY_F24`.
/// This helper adds friendly aliases:
///
/// - `F24` becomes `KEY_F24`
/// - `LEFTMETA` becomes `KEY_LEFTMETA`
/// - `CAPSLOCK` becomes `KEY_CAPSLOCK`
/// - `194` becomes `KeyCode::new(194)`
fn parse_key_code(input: &str) -> std::result::Result<KeyCode, String> {
    let trimmed = input.trim();

    if trimmed.is_empty() {
        return Err("key code must not be empty".to_string());
    }

    if let Ok(code) = trimmed.parse::<u16>() {
        return Ok(KeyCode::new(code));
    }

    let upper = trimmed.to_ascii_uppercase();
    let canonical = if upper.starts_with("KEY_") || upper.starts_with("BTN_") {
        upper
    } else {
        format!("KEY_{upper}")
    };

    KeyCode::from_str(&canonical)
        .map_err(|err| format!("could not parse key code {input:?} as {canonical:?}: {err:?}"))
}

/// Map evdev physical key codes to the command namespace.
///
/// This is deliberately layout-insensitive. `KEY_A` means the physical/logical
/// evdev key code named A, not "whatever character the current desktop layout
/// would type". That matches the first-version design goal: alphanumeric command
/// keys, normalized to lowercase, with punctuation and layout-sensitive behavior
/// left out.
fn command_char_for_key(key: KeyCode) -> Option<char> {
    match key {
        KeyCode::KEY_A => Some('a'),
        KeyCode::KEY_B => Some('b'),
        KeyCode::KEY_C => Some('c'),
        KeyCode::KEY_D => Some('d'),
        KeyCode::KEY_E => Some('e'),
        KeyCode::KEY_F => Some('f'),
        KeyCode::KEY_G => Some('g'),
        KeyCode::KEY_H => Some('h'),
        KeyCode::KEY_I => Some('i'),
        KeyCode::KEY_J => Some('j'),
        KeyCode::KEY_K => Some('k'),
        KeyCode::KEY_L => Some('l'),
        KeyCode::KEY_M => Some('m'),
        KeyCode::KEY_N => Some('n'),
        KeyCode::KEY_O => Some('o'),
        KeyCode::KEY_P => Some('p'),
        KeyCode::KEY_Q => Some('q'),
        KeyCode::KEY_R => Some('r'),
        KeyCode::KEY_S => Some('s'),
        KeyCode::KEY_T => Some('t'),
        KeyCode::KEY_U => Some('u'),
        KeyCode::KEY_V => Some('v'),
        KeyCode::KEY_W => Some('w'),
        KeyCode::KEY_X => Some('x'),
        KeyCode::KEY_Y => Some('y'),
        KeyCode::KEY_Z => Some('z'),
        KeyCode::KEY_0 => Some('0'),
        KeyCode::KEY_1 => Some('1'),
        KeyCode::KEY_2 => Some('2'),
        KeyCode::KEY_3 => Some('3'),
        KeyCode::KEY_4 => Some('4'),
        KeyCode::KEY_5 => Some('5'),
        KeyCode::KEY_6 => Some('6'),
        KeyCode::KEY_7 => Some('7'),
        KeyCode::KEY_8 => Some('8'),
        KeyCode::KEY_9 => Some('9'),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_keys_are_lowercase_letters_and_digits() {
        assert_eq!(command_char_for_key(KeyCode::KEY_A), Some('a'));
        assert_eq!(command_char_for_key(KeyCode::KEY_Z), Some('z'));
        assert_eq!(command_char_for_key(KeyCode::KEY_0), Some('0'));
        assert_eq!(command_char_for_key(KeyCode::KEY_9), Some('9'));
        assert_eq!(command_char_for_key(KeyCode::KEY_SPACE), None);
    }

    #[test]
    fn script_paths_use_configured_extension() {
        let dir = PathBuf::from("/home/nova/.hyper");
        assert_eq!(script_path_for(&dir, 'a', "sh"), dir.join("a.sh"));
        assert_eq!(script_path_for(&dir, '1', ""), dir.join("1"));
    }

    #[test]
    fn extension_normalization_accepts_optional_dot() {
        assert_eq!(normalize_extension("sh"), "sh");
        assert_eq!(normalize_extension(".sh"), "sh");
        assert_eq!(normalize_extension(""), "");
    }

    #[test]
    fn key_parser_accepts_canonical_names_and_aliases() {
        assert_eq!(parse_key_code("KEY_F24").unwrap(), KeyCode::KEY_F24);
        assert_eq!(parse_key_code("F24").unwrap(), KeyCode::KEY_F24);
        assert_eq!(parse_key_code("capslock").unwrap(), KeyCode::KEY_CAPSLOCK);
    }
}
