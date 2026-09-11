#!/usr/bin/env bash
# Convenience entry point when the mobile checkout is a sibling of `app`.
# The canonical implementation lives with the mobile workflow so that its CI
# does not depend on an unreleased commit in the desktop repository.
set -euo pipefail

MOBILE_DIR="${KNOTQ_MOBILE_DIR:-mobile}"
exec "${MOBILE_DIR}/.github/scripts/run-mobile-ws-integration.sh"
