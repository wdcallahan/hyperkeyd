// SPDX-License-Identifier: GPL-3.0-or-later

//! Interactive machine-specific keyboard enrollment.
//!
//! Setup deliberately observes evdev passively, just like the daemon. This
//! checkpoint discovers which readable evdev stream and keycode actually
//! produce the user's intended Hyper press, then separately discovers which
//! stream carries a physical `A` command-key press. Later checkpoints will add
//! stable `/dev/input/by-id/` resolution, verification, and configuration
//! persistence.

use anyhow::{bail, Context, Result};
use evdev::{Device, EventSummary, KeyCode};
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;

#[derive(Debug)]
struct ObservedKeyPress {
    device_path: PathBuf,
    device_name: String,
    key: KeyCode,
}

pub fn run() -> Result<()> {
    let candidate_count = readable_key_devices().len();

    if candidate_count == 0 {
        bail!("no readable evdev devices that report key events were found");
    }

    println!("hyperkeyd setup");
    println!(
        "Listening to {candidate_count} readable evdev key-event stream(s)."
    );
    println!("Press and release the physical key you want to use as Hyper.");

    let hyper = observe_press(None)?;

    println!();
    println!("Observed Hyper candidate:");
    print_observation(&hyper);

    println!();
    println!("Now press and release the physical A key.");

    let command = observe_press(Some(KeyCode::KEY_A))?;

    println!();
    println!("Observed command-key stream:");
    print_observation(&command);

    println!();
    if hyper.device_path == command.device_path {
        println!("Enrollment topology: Hyper and command keys share one evdev stream.");
    } else {
        println!("Enrollment topology: Hyper and command keys use separate evdev streams.");
    }
    println!("No configuration has been written yet.");

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

fn print_observation(observed: &ObservedKeyPress) {
    println!("  device: {}", observed.device_path.display());
    println!("  name: {}", observed.device_name);
    println!("  keycode: {}", observed.key.code());
    println!("  evdev: {:?}", observed.key);
}

fn readable_key_devices() -> Vec<(PathBuf, Device)> {
    let mut devices = evdev::enumerate()
        .filter(|(_, device)| device.supported_keys().is_some())
        .collect::<Vec<_>>();

    devices.sort_by(|(left, _), (right, _)| left.cmp(right));
    devices
}
