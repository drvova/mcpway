#!/usr/bin/env bash
set -euo pipefail

MODE="${1:-}"
if [[ "$MODE" != "stable" ]]; then
  echo "Usage: $0 stable" >&2
  exit 1
fi

if [[ -z "${CARGO_REGISTRY_TOKEN:-}" ]]; then
  echo "CARGO_REGISTRY_TOKEN is required" >&2
  exit 1
fi

REF_NAME="${MCPWAY_RELEASE_TAG:-${RELEASE_TAG:-${GITHUB_REF_NAME:-}}}"
if [[ -z "$REF_NAME" ]]; then
  echo "Release tag is required via MCPWAY_RELEASE_TAG, RELEASE_TAG, or GITHUB_REF_NAME (expected v0.1.0)" >&2
  exit 1
fi

if [[ "$REF_NAME" =~ ^v([0-9]+\.[0-9]+\.[0-9]+([-.][0-9A-Za-z.]+)?)$ ]]; then
  TAG_VERSION="${BASH_REMATCH[1]}"
else
  echo "Stable mode requires a v* tag (received: $REF_NAME)" >&2
  exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RUST_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$RUST_DIR"

PACKAGE_NAME="$(awk -F'"' '/^name = / {print $2; exit}' Cargo.toml)"
MANIFEST_VERSION="$(awk -F'"' '/^version = / {print $2; exit}' Cargo.toml)"
if [[ -z "$PACKAGE_NAME" || -z "$MANIFEST_VERSION" ]]; then
  echo "Failed to parse package metadata from Cargo.toml" >&2
  exit 1
fi
if [[ "$MANIFEST_VERSION" != "$TAG_VERSION" ]]; then
  echo "Tag version ($TAG_VERSION) does not match Cargo.toml version ($MANIFEST_VERSION)." >&2
  exit 1
fi

echo "Publishing $PACKAGE_NAME@$MANIFEST_VERSION from tag $REF_NAME"
export CARGO_REGISTRIES_CRATES_IO_PROTOCOL=sparse

cargo package --list
cargo publish --token "$CARGO_REGISTRY_TOKEN"
echo "Publish succeeded for $PACKAGE_NAME@$MANIFEST_VERSION"
