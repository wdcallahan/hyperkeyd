#!/usr/bin/env bash
#
# Check the local assumptions hyperkeyd depends on. This is a diagnostic tool,
# not an installer, and it never starts hyperkeyd or changes the system.
#
# Authored by Pixel-5.6 (OpenAI ChatGPT Work), under the direction of
# W. D. Callahan II. Both accept responsibility for this contribution.

set -u

PROGRAM_NAME="${0##*/}"
DEVICE=""
HYPER_KEY=""
SCRIPT_DIR="${HOME}/.hyper"
EXTENSION="sh"
COMMAND_KEY="a"
BINARY=""
INTERACTIVE=1
SECONDS_TO_LISTEN=6

PASS_COUNT=0
WARN_COUNT=0
FAIL_COUNT=0
WARN_REASONS=()
FAIL_REASONS=()
TMP_FILES=()

cleanup() {
    if ((${#TMP_FILES[@]})); then
        rm -f -- "${TMP_FILES[@]}"
    fi
}
trap cleanup EXIT HUP INT TERM

if [[ -t 1 ]]; then
    GREEN=$'\033[32m'; YELLOW=$'\033[33m'; RED=$'\033[31m'
    BLUE=$'\033[34m'; BOLD=$'\033[1m'; DIM=$'\033[2m'; RESET=$'\033[0m'
else
    GREEN=""; YELLOW=""; RED=""; BLUE=""; BOLD=""; DIM=""; RESET=""
fi

usage() {
    cat <<EOF
Usage:
  ${PROGRAM_NAME} --device PATH --hyper-key KEY_NAME [options]

Required:
  --device PATH          Evdev path, preferably /dev/input/by-id/*-event-kbd
  --hyper-key KEY_NAME   Evdev key name used as Hyper, such as KEY_F21

Options:
  --script-dir DIR       Script directory (default: ~/.hyper)
  --extension EXT        Script extension (default: sh)
  --command-key KEY      Command script to check (default: a)
  --binary PATH          hyperkeyd binary (default: auto-detect)
  --seconds N            Interactive test duration (default: 6)
  --no-interactive       Skip the evtest keypress test
  -h, --help             Show this help
EOF
}

pass() { PASS_COUNT=$((PASS_COUNT + 1)); printf '%s[OK]%s   %s\n' "$GREEN" "$RESET" "$*"; }
warn() { WARN_COUNT=$((WARN_COUNT + 1)); WARN_REASONS+=("$*"); printf '%s[WARN]%s %s\n' "$YELLOW" "$RESET" "$*"; }
fail() { FAIL_COUNT=$((FAIL_COUNT + 1)); FAIL_REASONS+=("$*"); printf '%s[FAIL]%s %s\n' "$RED" "$RESET" "$*"; }
note() { printf '%s[NOTE]%s %s\n' "$BLUE" "$RESET" "$*"; }
have() { command -v "$1" >/dev/null 2>&1; }

need_value() {
    if (($# < 2)) || [[ -z "$2" ]]; then
        printf 'Missing value for %s\n' "$1" >&2
        usage >&2
        exit 2
    fi
}

while (($#)); do
    case "$1" in
        --device|--hyper-key|--script-dir|--extension|--command-key|--binary|--seconds)
            need_value "$@"
            case "$1" in
                --device) DEVICE="$2" ;;
                --hyper-key) HYPER_KEY="$2" ;;
                --script-dir) SCRIPT_DIR="$2" ;;
                --extension) EXTENSION="$2" ;;
                --command-key) COMMAND_KEY="$2" ;;
                --binary) BINARY="$2" ;;
                --seconds) SECONDS_TO_LISTEN="$2" ;;
            esac
            shift 2
            ;;
        --no-interactive) INTERACTIVE=0; shift ;;
        -h|--help) usage; exit 0 ;;
        *) printf 'Unknown option: %s\n' "$1" >&2; usage >&2; exit 2 ;;
    esac
done

if ! [[ "$SECONDS_TO_LISTEN" =~ ^[1-9][0-9]*$ ]]; then
    printf '%s: --seconds must be a positive integer\n' "$PROGRAM_NAME" >&2
    exit 2
fi

printf '%s\n%s\n\n' "${BOLD}hyperkeyd environment sanity check${RESET}" \
    "${DIM}This command only inspects the system; it makes no changes.${RESET}"

[[ "$(uname -s)" == Linux ]] && pass "Running on Linux." || fail "hyperkeyd requires Linux evdev."
[[ -d /dev/input ]] && pass "/dev/input exists." || fail "/dev/input does not exist."
have evtest && pass "evtest is installed." || warn "evtest is unavailable; raw key checks will be skipped."
have timeout && pass "timeout is installed." || warn "timeout is unavailable; raw key checks will be skipped."
have cargo && pass "cargo is installed." || warn "cargo is unavailable; an existing binary can still be used."

if [[ -z "$DEVICE" ]]; then
    fail "No --device was provided."
    if [[ -d /dev/input/by-id ]]; then
        note "Candidate keyboard devices:"
        find /dev/input/by-id -maxdepth 1 -name '*-event-kbd' -ls 2>/dev/null || true
    fi
elif [[ ! -e "$DEVICE" ]]; then
    fail "Device path does not exist: $DEVICE"
else
    pass "Device path exists: $DEVICE"
    REAL_DEVICE="$(readlink -f -- "$DEVICE" 2>/dev/null || printf '%s' "$DEVICE")"
    note "Device resolves to: $REAL_DEVICE"
    note "Device permissions: $(ls -l -- "$REAL_DEVICE")"
    if [[ -r "$REAL_DEVICE" ]]; then
        pass "Current user can read the device."
    else
        fail "Current user cannot read the device: $REAL_DEVICE"
        note "Use a device-specific udev rule based on contrib/72-hyperkeyd-keyboard.rules.example."
        note "Do not run hyperkeyd as root: it launches scripts from your configuration."
        note "Membership in the input group grants broad access to raw keyboard events and is not recommended."
    fi
fi

[[ -n "$HYPER_KEY" ]] || fail "No --hyper-key was provided (example: KEY_F21)."

if [[ -n "$DEVICE" && -n "$HYPER_KEY" && -r "$DEVICE" ]] && have evtest && have timeout; then
    SUPPORTED="$(mktemp)"; TMP_FILES+=("$SUPPORTED")
    timeout 1s evtest "$DEVICE" >"$SUPPORTED" 2>&1 || true
    if grep -Fq "($HYPER_KEY)" "$SUPPORTED"; then
        pass "Device reports support for $HYPER_KEY."
    else
        warn "Could not confirm that this device supports $HYPER_KEY."
    fi

    if ((INTERACTIVE)); then
        EVENTS="$(mktemp)"; TMP_FILES+=("$EVENTS")
        printf '\nPress Enter, then press and release Hyper within %s seconds (Ctrl-C cancels).\n' "$SECONDS_TO_LISTEN"
        read -r _
        timeout "${SECONDS_TO_LISTEN}s" evtest "$DEVICE" >"$EVENTS" 2>&1 || true
        if grep -F "$HYPER_KEY" "$EVENTS" | grep -q 'value 1'; then
            pass "Observed a $HYPER_KEY press on the selected device."
        else
            fail "Did not observe a $HYPER_KEY press on the selected device."
        fi
    else
        note "Skipping interactive key test."
    fi
fi

if [[ -d "$SCRIPT_DIR" ]]; then
    pass "Script directory exists: $SCRIPT_DIR"
else
    warn "Script directory does not exist: $SCRIPT_DIR"
fi

if [[ -n "$EXTENSION" ]]; then TEST_SCRIPT="$SCRIPT_DIR/$COMMAND_KEY.$EXTENSION"; else TEST_SCRIPT="$SCRIPT_DIR/$COMMAND_KEY"; fi
if [[ -x "$TEST_SCRIPT" ]]; then
    pass "Test command exists and is executable: $TEST_SCRIPT"
elif [[ -e "$TEST_SCRIPT" ]]; then
    fail "Test command exists but is not executable: $TEST_SCRIPT"
    note "Fix with: chmod +x \"$TEST_SCRIPT\""
else
    warn "No test command found at: $TEST_SCRIPT"
    note "A missing command is intentionally ignored; its ordinary key may still reach applications."
fi

if [[ -z "$BINARY" ]]; then
    [[ -x ./target/debug/hyperkeyd ]] && BINARY=./target/debug/hyperkeyd
    [[ -z "$BINARY" && -x ./target/release/hyperkeyd ]] && BINARY=./target/release/hyperkeyd
    [[ -z "$BINARY" ]] && BINARY="$(command -v hyperkeyd 2>/dev/null || true)"
fi

if [[ -x "$BINARY" ]]; then
    pass "hyperkeyd binary found: $BINARY"
    "$BINARY" --help >/dev/null 2>&1 && pass "hyperkeyd responds to --help." || warn "hyperkeyd --help failed."
elif [[ -n "$BINARY" ]]; then
    fail "Configured binary is not executable: $BINARY"
else
    warn "No hyperkeyd binary found; build one with cargo build."
fi

printf '\n%sSummary%s\n  %sOK:%s %s  %sWARN:%s %s  %sFAIL:%s %s\n' \
    "$BOLD" "$RESET" "$GREEN" "$RESET" "$PASS_COUNT" "$YELLOW" "$RESET" "$WARN_COUNT" "$RED" "$RESET" "$FAIL_COUNT"

if ((WARN_COUNT)); then
    printf '\n%sWarnings:%s\n' "$YELLOW" "$RESET"
    printf '  - %s\n' "${WARN_REASONS[@]}"
fi
if ((FAIL_COUNT)); then
    printf '\n%sFailures:%s\n' "$RED" "$RESET"
    printf '  - %s\n' "${FAIL_REASONS[@]}"
    exit 1
fi

printf '\n%sBasic checks passed.%s\n' "$GREEN" "$RESET"
printf '%s\n' 'Important: current passive operation observes keys but cannot universally suppress them.'
printf '%s\n' 'Before daily use, test Hyper+command in Firefox, a terminal, a text editor, and Signal/Chromium.'
