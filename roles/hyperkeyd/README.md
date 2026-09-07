# hyperkeyd Ansible role

This local role owns installation plumbing for the per-user `hyperkeyd` daemon. It does not own keyboard semantics or command actions.

## Current checkpoint

The role currently:

- verifies that it is running as a non-root desktop user;
- verifies the expected Cargo checkout and Cargo availability;
- ensures `~/.local/bin`, `~/.hyper`, `~/.config/hyperkeyd`, and the systemd user-unit directory exist;
- builds with `cargo build --release --locked`;
- installs the resulting binary as `~/.local/bin/hyperkeyd` only when its contents differ;
- preserves an existing `~/.config/hyperkeyd/config.toml` without rewriting it;
- installs a config-driven `hyperkeyd.service` user unit;
- reloads the user systemd manager only when the unit changes;
- restarts the daemon only when the installed binary or unit changes;
- enables and starts the service only when a non-empty machine enrollment file exists; and
- verifies that a live managed service reaches the `active` state.

The role creates `~/.hyper` when missing but never enumerates, owns, edits, or deletes the user's command scripts inside it.

Keyboard identity remains machine-local state produced by `hyperkeyd setup`. Ansible preserves that state and points the service at it rather than duplicating keyboard policy in inventory or role variables.

## Fresh installation

On a machine with no existing enrollment, the first live playbook run installs the binary and user-service unit but deliberately leaves the service disabled and stopped. After the successful run, the final task tells the user to run:

```bash
~/.local/bin/hyperkeyd setup
```

Setup performs the interactive physical-key enrollment and writes the verified machine configuration. Then rerun:

```bash
ansible-playbook playbooks/install.yml
```

The second run sees the enrollment, enables and starts the service, and verifies that it reaches the `active` state.

## Variables

All variables have current-user defaults and may be overridden by callers:

- `hyperkeyd_source_dir` — source checkout; defaults to the repository root containing the calling playbook.
- `hyperkeyd_home` — target user's home directory.
- `hyperkeyd_install_dir` — user-local binary directory.
- `hyperkeyd_binary_path` — installed daemon path.
- `hyperkeyd_script_dir` — user-owned command-script directory.
- `hyperkeyd_config_dir` — machine configuration directory.
- `hyperkeyd_config_path` — setup-generated TOML configuration path.
- `hyperkeyd_systemd_user_dir` — current user's systemd unit directory.
- `hyperkeyd_service_name` — managed user-service unit name.
- `hyperkeyd_service_path` — complete managed unit path.
- `hyperkeyd_xdg_runtime_dir` — per-user runtime directory used to reach the user systemd bus.

## Check mode

Directory, enrollment, unit-file, and already-existing service state are evaluated in check mode. Rust compilation and binary replacement are explicitly skipped because a fresh build is required to know the resulting binary checksum truthfully.

When the managed unit does not yet exist, check mode can predict that the template would be installed but cannot ask systemd about that future unit. The live run performs the daemon-reload and service-state management after writing it.
