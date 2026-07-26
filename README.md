# hyperkeyd

`hyperkeyd` is a small Linux daemon that turns a dedicated Hyper key into a personal command dispatcher.

When the configured Hyper key is held, each alphanumeric key press launches the matching executable from a user-owned script directory:

```text
Hyper down       -> dispatcher armed
Hyper + a        -> ~/.hyper/a.sh
Hyper + 1        -> ~/.hyper/1.sh
Hyper up         -> dispatcher idle
```

There are no sequences, no prefixes, no command buffers, and no timing rules. Every physical alphanumeric key press while armed is a complete command event. Kernel-generated key-repeat events are always ignored: holding a command key launches it once. To launch it again, release and press that command key again; Hyper may remain held the entire time.

## Why evdev?

Modern Wayland desktops intentionally restrict arbitrary global keyboard interception. This daemon reads Linux evdev input devices directly, below the compositor layer.

That makes the approach independent of X11-style global hotkey APIs, but it also means the daemon needs permission to read `/dev/input/event*` devices.

## Important limitation: this daemon listens; it does not grab

Version 0.1 is a passive listener. It does **not** grab the keyboard and it does **not** suppress key events before they reach your compositor or applications.

That is intentional for the first version. A daemon that grabs input must usually create a virtual keyboard with `uinput` and forward non-command input back to the desktop. That is a much larger and more dangerous design.

For clean behavior, the Hyper trigger should be set up so that holding it does not type ordinary characters into applications. Good deployment options include:

- a real modifier mapping, such as an XKB Hyper/Super-style modifier;
- a firmware-level key that your desktop treats as a modifier;
- a dedicated keycode that your compositor does not use, plus compositor/keymap configuration that prevents command letters from leaking as text;
- or accepting passive-listener behavior during testing with `--dry-run`.

If you bind Hyper to a plain non-modifier key such as `KEY_F24`, then pressing `F24+a` may still send `a` to the active application unless your desktop/keymap treats that trigger as a modifier or otherwise consumes it.

If the matching command script does not exist, `hyperkeyd` silently takes no action. Because version 0.1 is only a listener, the presence or absence of a script does not determine whether the desktop also receives the command letter. Empty default scripts would not suppress those desktop key events.

## Nova keyboard integration status

`hyperkeyd` is currently an experimental component, not a production-critical
part of MACE's accepted keyboard state. Its generic default remains
`KEY_F24`.

Nova's physical Hyper position now emits `PB_11`, which Linux exposes below
XKB as `KEY_MACRO11`; XKB separately names the same transport `Hyper_L`. A
test against the current board should therefore use
`--hyper-key KEY_MACRO11`, because this daemon listens at evdev and never sees
the later XKB keysym.

The canonical whole-system status and boundaries live in
[`x1_keyboard_layout`](https://github.com/wdcallahan/x1_keyboard_layout):

- [guided tour](https://github.com/wdcallahan/x1_keyboard_layout/blob/main/docs/nova-keyboard-input-architecture.md)
- [technical architecture](https://github.com/wdcallahan/x1_keyboard_layout/blob/main/docs/keyboard-architecture.md)

## Build

```bash
cargo build --release
```

The resulting binary will be:

```bash
target/release/hyperkeyd
```

Install it somewhere in your user path, for example:

```bash
install -Dm755 target/release/hyperkeyd ~/.local/bin/hyperkeyd
```

## Find your keyboard device

Run:

```bash
hyperkeyd --list-devices
```

You will see lines like:

```text
/dev/input/event7    keyboard-ish    Example Keyboard
```

For a systemd service, prefer a stable symlink under `/dev/input/by-id/` if your keyboard exposes one:

```bash
ls -l /dev/input/by-id/
```

Then use that stable path with `--device`.

## Basic test run

Start with `--dry-run` so no scripts are launched yet:

```bash
RUST_LOG=hyperkeyd=debug hyperkeyd --device /dev/input/event7 --hyper-key KEY_F24 --dry-run
```

Press and hold your configured Hyper key, then press `a`, `b`, or a digit. You should see log messages showing which script would have launched.

## Create command scripts

Create the script directory:

```bash
mkdir -p ~/.hyper
```

Create a test command:

```bash
cat > ~/.hyper/a.sh <<'SCRIPT'
#!/usr/bin/env bash
printf 'hyperkeyd launched a.sh at %s\n' "$(date)" >> "$HOME/.hyper/test.log"
SCRIPT

chmod +x ~/.hyper/a.sh
```

Run without `--dry-run`:

```bash
hyperkeyd --device /dev/input/event7 --hyper-key KEY_F24
```

Then hold Hyper and press `a`.

## CLI reference

```text
--list-devices
    Print evdev input devices and exit.

--device PATH
    Evdev device to read. May be passed more than once.

--hyper-key KEY
    Trigger key. Default: KEY_F24.
    Examples: KEY_F24, F24, KEY_LEFTMETA, LEFTMETA, CAPSLOCK, 194.

--script-dir DIR
    Command script directory. Default: ~/.hyper.

--extension EXT
    Script extension. Default: sh.
    Use --extension '' for extensionless command files.

--log-missing
    Log missing scripts instead of silently ignoring them.

--dry-run
    Log what would launch without executing scripts.
```

## Script environment

When a script is launched, `hyperkeyd` sets a few informational environment variables:

```text
HYPERKEYD_KEY
    The command character, such as a or 1.

HYPERKEYD_EVENT
    press.

HYPERKEYD_DEVICE
    The evdev device path that produced the command key event.
```

The script is executed directly. `hyperkeyd` does not invoke a shell and does not evaluate command strings.

## Permissions

Reading `/dev/input/event*` often requires additional permission. Depending on your distribution, common approaches include:

- adding your user to an input-related group;
- using a udev rule to grant read access to a specific keyboard;
- running a small privileged input component later and keeping script execution unprivileged.

For the first personal version, the simplest approach is usually to grant your user read access to the selected keyboard device. Avoid running the whole daemon as root if you can, because the daemon launches user scripts.

## systemd user service

A udev-rule example is provided in:

```text
contrib/72-hyperkeyd-keyboard.rules.example
```

A template service is provided in:

```text
contrib/hyperkeyd.service
```

Copy it to:

```bash
mkdir -p ~/.config/systemd/user
cp contrib/hyperkeyd.service ~/.config/systemd/user/hyperkeyd.service
```

Edit the `ExecStart=` line so `--device` and `--hyper-key` match your system.

Then enable it:

```bash
systemctl --user daemon-reload
systemctl --user enable --now hyperkeyd.service
journalctl --user -u hyperkeyd.service -f
```

## Design boundaries

`hyperkeyd` should stay boring.

It should not become:

- a macro system;
- a command palette;
- a sequence parser;
- a keyboard remapper;
- an OBS controller;
- a lighting controller;
- a shell language;
- or a desktop environment.

Its one job is:

```text
key event -> executable script
```

Everything action-specific belongs in the scripts.

## Development provenance

`hyperkeyd` is developed openly as a collaboration between W. D. Callahan II and Pixel, the OpenAI assistant working with him through ChatGPT Work. Commits containing code written directly by Pixel carry a `Pixel-Authored-By` trailer naming the Pixel version and a `Human-Directed-By` trailer naming the person who requested and accepted the work.

These trailers preserve who typed each change instead of concealing the use of AI. Human and assistant both remain accountable for reviewing, testing, and learning from the result.

## License

hyperkeyd is free software licensed under the GNU General Public License,
version 3 or (at your option) any later version.

Copyright 2026 W. D. Callahan II

See the file COPYING for details.
