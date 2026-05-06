#!/usr/bin/env bash
# Example hyperkeyd command script.
#
# Install with:
#   mkdir -p ~/.hyper
#   cp examples/a.sh ~/.hyper/a.sh
#   chmod +x ~/.hyper/a.sh

printf 'hyperkeyd: key=%s event=%s device=%s time=%s\n' \
  "${HYPERKEYD_KEY:-unknown}" \
  "${HYPERKEYD_EVENT:-unknown}" \
  "${HYPERKEYD_DEVICE:-unknown}" \
  "$(date --iso-8601=seconds)" >> "$HOME/.hyper/test.log"
