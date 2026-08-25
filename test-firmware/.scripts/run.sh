#!/bin/sh
# Cargo runner for this firmware.
#
# This is invoked as:
#     run.sh <console|monitor> <path-to-elf>
#
# Cargo appends the built ELF as the final argument when this is used in `cargo run`. so that's how that works
#
# It checks that the tool for the requested mode is installed, offers to install it
# when run interactively, and then hands over to it. Cargo gives the runner a real
# terminal on all three streams, so the prompt works and `cargo install` progress is
# visible as it happens.

set -eu

REPO="https://github.com/bjackson312006/defmt-monitor"

# This needs to be defined once in .cargo/config.toml's [env] table. Cargo passes that through to this script.
CHIP="${FIRMWARE_CHIP:?FIRMWARE_CHIP is not set; see .cargo/config.toml}"

mode="${1:-}"
[ -n "$mode" ] || { echo "usage: run.sh <console|monitor> <elf>" >&2; exit 2; }
shift

case "$mode" in
    console) tool="probe-rs" ;;
    monitor) tool="defmt-monitor-tui" ;;
    *) echo "run.sh: unknown mode '$mode' (expected console or monitor)" >&2; exit 2 ;;
esac

install_hint() {
    case "$1" in
        probe-rs)          echo "cargo install probe-rs-tools --locked" ;;
        defmt-monitor-tui) echo "cargo install --git $REPO defmt-monitor-tui --locked" ;;
    esac
}

install_tool() {
    case "$1" in
        probe-rs)          cargo install probe-rs-tools --locked ;;
        defmt-monitor-tui) cargo install --git "$REPO" defmt-monitor-tui --locked ;;
    esac
}

if ! command -v "$tool" >/dev/null 2>&1; then
    {
        echo
        echo "  $tool is required for '$mode' mode but was not found on PATH."
        echo
        echo "      $(install_hint "$tool")"
        echo
    } >&2

    # this only prompts when it is a person! for CI it will not hang
    if [ -t 0 ] && [ -r /dev/tty ]; then
        printf '  Install it now? [y/N] ' >&2
        read -r reply </dev/tty || reply=""
        case "$reply" in
            [Yy]*)
                echo >&2
                install_tool "$tool"
                echo >&2
                ;;
            *)
                echo "  Aborted." >&2
                exit 1
                ;;
        esac
    else
        echo "  Not a terminal, so not prompting. Run the command above, then retry." >&2
        exit 1
    fi
fi

case "$mode" in
    console) exec probe-rs run --chip "$CHIP" "$@" ;;
    monitor) exec defmt-monitor-tui --chip "$CHIP" "$@" ;;
esac
