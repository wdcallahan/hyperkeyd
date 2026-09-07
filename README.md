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
test against the current board should therefore use `KEY_MACRO11`, because
this daemon listens at evdev and never sees the later XKB keysym.

The `evdev` crate version currently used by `hyperkeyd` does not yet assign a
symbolic Rust constant to the kernel's newer `KEY_MACRO1` through
`KEY_MACRO30` range. `hyperkeyd` therefore provides a narrow parser fallback
for those canonical Linux names. `KEY_MACRO11` maps to kernel keycode `0x29a`
(decimal `666`). The crate's debug formatter may still display that value as
`unknown key: 666`; that display does not mean parsing failed.

Interactive `hyperkeyd setup` enrollment has been hardware-proven against the
current Lemokey X2. It independently discovers the X2's split Hyper and command
streams from physical keypresses, resolves both to stable `/dev/input/by-id/`
paths, verifies a complete Hyper+A chord, and writes the resulting machine
configuration.

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

## Install with Ansible

The repository includes a local Ansible installer for the current desktop user:

```bash
ansible-playbook playbooks/install.yml
```

The installer builds the locked release binary, installs it under `~/.local/bin`, creates the required user directories, installs the config-driven systemd user unit, and preserves any existing machine enrollment at `~/.config/hyperkeyd/config.toml`.

On a fresh machine with no enrollment yet, the first playbook run installs the binary and unit but deliberately leaves the service disabled and stopped. At the successful end of the run it tells you to enroll the keyboard:

```bash
~/.local/bin/hyperkeyd setup
```

After setup succeeds, rerun:

```bash
ansible-playbook playbooks/install.yml
```

The second run sees the verified machine configuration, enables and starts `hyperkeyd.service`, and verifies that the managed user service reaches the active state. Existing command scripts under `~/.hyper` are never enumerated, edited, or deleted by the role.

## Enroll a keyboard

The normal machine-enrollment path is the interactive setup wizard:

```bash
hyperkeyd setup
```

Setup observes readable evdev key-event streams passively; it does not grab the
keyboard. The wizard asks you to identify the intended Hyper key with a physical
keypress, then asks for the physical `A` key so it can discover the command-key
stream. This works with both ordinary single-stream keyboards and split-interface
keyboards where Hyper and normal typing arrive on different evdev devices.

After discovery, setup resolves the observed `/dev/input/eventN` nodes to stable
`/dev/input/by-id/` aliases. It deliberately refuses to persist a volatile event
number, and it also refuses to choose silently if an observed device has multiple
stable aliases.

Before writing anything, setup verifies the enrollment on those stable paths. It
guides the verification one physical action at a time:

```text
hold Hyper
press and release A
release Hyper
```

There is no verification timeout. If Hyper is released too early, setup explains
what happened and restarts only the verification sequence.

Only after successful verification does setup atomically write:

```text
~/.config/hyperkeyd/config.toml
```

The write uses a same-directory temporary file followed by rename, so a partial
configuration is not installed. `script_dir` is intentionally omitted from the
generated file; the runtime therefore keeps its normal `$HOME/.hyper` default.

Running `hyperkeyd setup` is an explicit re-enrollment operation. When you replace
a keyboard or intentionally choose a different Hyper key, run setup again.

## Inspect keyboard devices manually

For troubleshooting or manual configuration, run:

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

Then use that stable path with `--device` or store it in a machine configuration file. Do not treat advertised key capabilities alone as proof that a particular physical key emits on that stream; `hyperkeyd setup` observes the actual physical keypresses for that reason.

## Machine configuration file

`hyperkeyd setup` is the recommended way to create machine-specific configuration, but the TOML format can also be written or maintained manually and loaded with `--config`.

A split-interface keyboard may need more than one evdev stream: one device can carry the Hyper transport while another carries ordinary command keys. List every required stable device path:

```toml
hyper_key = "KEY_MACRO11"
devices = [
    "/dev/input/by-id/usb-Keychron_Lemokey_X2-event-if01",
    "/dev/input/by-id/usb-Keychron_Lemokey_X2-if01-event-kbd",
]
```

With that file saved as `~/.config/hyperkeyd/config.toml`, run:

```bash
hyperkeyd --config ~/.config/hyperkeyd/config.toml
```

`script_dir` is optional. If it is omitted, `hyperkeyd` uses `$HOME/.hyper`:

```toml
hyper_key = "KEY_MACRO11"
devices = ["/dev/input/by-id/example-keyboard-event-kbd"]
script_dir = "/home/example/.hyper"
```

Paths in TOML are literal paths. `hyperkeyd` does **not** shell-expand `~` inside the file, so either omit `script_dir` to use the default or store an absolute path.

Explicit command-line values override the corresponding file values:

- one or more `--device` arguments replace the file's `devices` list;
- `--hyper-key` replaces the file's `hyper_key`;
- `--script-dir` replaces the file's `script_dir` or the default.

If neither the command line nor the configuration file supplies a Hyper key, the generic runtime default remains `KEY_F24`.

## Basic test run

Start with `--dry-run` so no scripts are launched yet:

```bash
RUST_LOG=hyperkeyd=debug hyperkeyd --device /dev/input/event7 --hyper-key KEY_F24 --dry-run
```

Or test a complete machine configuration:

```bash
RUST_LOG=hyperkeyd=debug hyperkeyd --config ~/.config/hyperkeyd/config.toml --dry-run
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
setup
    Interactively discover the physical Hyper and command-key streams,
    resolve stable device paths, verify Hyper+A, and atomically write the
    default machine configuration file.

--list-devices
    Print evdev input devices and exit.

--device PATH
    Evdev device to read. May be passed more than once.
    Explicit values replace devices from --config.

--config PATH
    Read machine-specific hyper_key, devices, and optional script_dir values
    from a TOML file.

--hyper-key KEY
    Trigger key. Default: KEY_F24 when not supplied by --config.
    Examples: KEY_F24, F24, KEY_LEFTMETA, LEFTMETA, CAPSLOCK,
    KEY_MACRO11, 194, 666.

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

A config-driven template service is provided in:

```text
contrib/hyperkeyd.service
```

For manual installation, first create a verified machine configuration with:

```bash
hyperkeyd setup
```

Then install and enable the unit:

```bash
mkdir -p ~/.config/systemd/user
cp contrib/hyperkeyd.service ~/.config/systemd/user/hyperkeyd.service
systemctl --user daemon-reload
systemctl --user enable --now hyperkeyd.service
journalctl --user -u hyperkeyd.service -f
```

The service reads machine-specific keyboard identity from `~/.config/hyperkeyd/config.toml`; it does not duplicate device paths or Hyper keycodes in `ExecStart=`.

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
