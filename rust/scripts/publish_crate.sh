#!/usr/bin/env bash
set -euo pipefail

MODE="${1:-}"
if [[ "$MODE" != "dev" && "$MODE" != "stable" ]]; then
  echo "Usage: $0 <dev|stable>" >&2
  exit 1
fi

if [[ -z "${CARGO_REGISTRY_TOKEN:-}" ]]; then
  echo "CARGO_REGISTRY_TOKEN is required" >&2
  exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RUST_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

CRATE_DIR="$TMP_DIR/rust"
mkdir -p "$CRATE_DIR"

# Build a clean temp crate workspace and avoid carrying local build artifacts.
(cd "$RUST_DIR" && tar --exclude=target -cf - .) | (cd "$CRATE_DIR" && tar -xf -)

cd "$CRATE_DIR"

PACKAGE_NAME="$(awk -F'"' '/^name = / {print $2; exit}' Cargo.toml)"
BASE_VERSION="$(awk -F'"' '/^version = / {print $2; exit}' Cargo.toml)"
if [[ -z "$PACKAGE_NAME" || -z "$BASE_VERSION" ]]; then
  echo "Failed to parse package metadata from Cargo.toml" >&2
  exit 1
fi

if [[ "$MODE" == "dev" ]]; then
  TS="$(date -u +%Y%m%d%H%M%S)"
  RUN_NO="${GITHUB_RUN_NUMBER:-0}"
  TARGET_VERSION="${BASE_VERSION}-dev.${TS}.${RUN_NO}"
else
  REF_NAME="${GITHUB_REF_NAME:-}"
  if [[ -z "$REF_NAME" ]]; then
    echo "GITHUB_REF_NAME is required for stable mode (expected tag like v0.1.0)" >&2
    exit 1
  fi
  if [[ "$REF_NAME" =~ ^v([0-9]+\.[0-9]+\.[0-9]+([-.][0-9A-Za-z.]+)?)$ ]]; then
    TARGET_VERSION="${BASH_REMATCH[1]}"
  else
    echo "Stable mode requires a v* tag (received: $REF_NAME)" >&2
    exit 1
  fi
fi

echo "Preparing publish for package '$PACKAGE_NAME' version '$TARGET_VERSION' ($MODE)"

perl -0777 -i -pe "s/^version\\s*=\\s*\"[^\"]+\"/version = \"$TARGET_VERSION\"/m" Cargo.toml

export CARGO_REGISTRIES_CRATES_IO_PROTOCOL=sparse

cargo package --allow-dirty --no-verify

attempt_publish() {
  local output_file="$1"
  set +e
  cargo publish --token "$CARGO_REGISTRY_TOKEN" --no-verify >"$output_file" 2>&1
  local status=$?
  set -e
  cat "$output_file"
  return "$status"
}

PUBLISH_LOG="$TMP_DIR/publish.log"
if attempt_publish "$PUBLISH_LOG"; then
  echo "Publish succeeded for $PACKAGE_NAME@$TARGET_VERSION"
  exit 0
fi

if [[ "$PACKAGE_NAME" == "mcpway" ]] && \
  grep -qiE "already exists on crates\\.io|crate .+mcpway.+already exists|name.+mcpway.+taken" "$PUBLISH_LOG"; then
  echo "Package name 'mcpway' unavailable. Retrying as 'mcpway-cli'."
  perl -0777 -i -pe 's/^name\s*=\s*"mcpway"/name = "mcpway-cli"/m' Cargo.toml
  PACKAGE_NAME="mcpway-cli"
  cargo package --allow-dirty --no-verify
  RETRY_LOG="$TMP_DIR/publish-retry.log"
  if attempt_publish "$RETRY_LOG"; then
    echo "Publish succeeded for fallback package $PACKAGE_NAME@$TARGET_VERSION"
    exit 0
  fi
fi

echo "Publish failed for $PACKAGE_NAME@$TARGET_VERSION" >&2
exit 1
