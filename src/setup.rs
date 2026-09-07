// SPDX-License-Identifier: GPL-3.0-or-later

//! Interactive machine-specific keyboard enrollment.
//!
//! Setup deliberately observes evdev passively, just like the daemon. The first
//! checkpoint only discovers which readable evdev stream and keycode actually
//! produce the user's intended Hyper press. Later checkpoints will add command
//! key discovery, stable `/dev/input/by-id/` resolution, verification, and
//! configuration persistence.

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
    let devices = readable_key_devices();

    if devices.is_empty() {
        bail!("no readable evdev devices that report key events were found");
    }

    println!("hyperkeyd setup");
    println!(
        "Listening to {} readable evdev key-event stream(s).",
        devices.len()
    );
    println!("Press and release the physical key you want to use as Hyper.");

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
                if let EventSummary::Key(_, key, 1) = event.destructure() {
                    let _ = tx.send(ObservedKeyPress {
                        device_path: path,
                        device_name,
                        key,
                    });
                    return;
                }
            }
        });
    }

    drop(tx);

    let observed = rx
        .recv()
        .context("all candidate input streams closed before a key press was observed")?;

    println!();
    println!("Observed Hyper candidate:");
    println!("  device: {}", observed.device_path.display());
    println!("  name: {}", observed.device_name);
    println!("  keycode: {}", observed.key.code());
    println!("  evdev: {:?}", observed.key);
    println!();
    println!("No configuration has been written yet.");

    Ok(())
}

fn readable_key_devices() -> Vec<(PathBuf, Device)> {
    let mut devices = evdev::enumerate()
        .filter(|(_, device)| device.supported_keys().is_some())
        .collect::<Vec<_>>();

    devices.sort_by(|(left, _), (right, _)| left.cmp(right));
    devices
}
