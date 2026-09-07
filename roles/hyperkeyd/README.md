# hyperkeyd Ansible role

This local role owns installation plumbing for the per-user `hyperkeyd` daemon. It does not own keyboard semantics or command actions.

## Current checkpoint

The role currently:

- verifies that it is running as a non-root desktop user;
- verifies the expected Cargo checkout and Cargo availability;
- ensures `~/.local/bin`, `~/.hyper`, and `~/.config/hyperkeyd` exist;
- builds with `cargo build --release --locked`;
- installs the resulting binary as `~/.local/bin/hyperkeyd` only when its contents differ; and
- preserves an existing `~/.config/hyperkeyd/config.toml` without rewriting it.

The role creates `~/.hyper` when missing but never enumerates, owns, edits, or deletes the user's command scripts inside it.

Systemd user-service management is deliberately deferred to the next installer checkpoint. The service will consume the machine-local configuration produced by `hyperkeyd setup` rather than duplicating keyboard policy in Ansible.

## Variables

All variables have current-user defaults and may be overridden by callers:

- `hyperkeyd_source_dir` — source checkout; defaults to the repository root containing the calling playbook.
- `hyperkeyd_home` — target user's home directory.
- `hyperkeyd_install_dir` — user-local binary directory.
- `hyperkeyd_binary_path` — installed daemon path.
- `hyperkeyd_script_dir` — user-owned command-script directory.
- `hyperkeyd_config_dir` — machine configuration directory.
- `hyperkeyd_config_path` — setup-generated TOML configuration path.

## Check mode

Directory state and enrollment-preservation checks run normally in check mode. Rust compilation and binary replacement are explicitly skipped because a fresh build is required to know the resulting binary checksum truthfully.
