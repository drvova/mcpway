#!/usr/bin/env bash
set -euo pipefail

SHA="${1:-96ca0de3bc1c22d8ad3ce91b7f068facdaf4851d}"
REPO="anomalyco/opencode"
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_DIR="${ROOT_DIR}/.reference/opencode/${SHA}"

FILES=(
  "packages/ui/src/components/button.tsx"
  "packages/ui/src/components/button.css"
  "packages/ui/src/components/text-field.tsx"
  "packages/ui/src/components/text-field.css"
  "packages/ui/src/components/select.tsx"
  "packages/ui/src/components/select.css"
  "packages/ui/src/components/icon-button.tsx"
  "packages/ui/src/components/icon-button.css"
  "packages/ui/src/components/card.tsx"
  "packages/ui/src/components/card.css"
  "packages/ui/src/components/scroll-view.tsx"
  "packages/ui/src/components/scroll-view.css"
  "packages/ui/src/styles/index.css"
  "packages/ui/src/styles/base.css"
  "packages/ui/src/styles/theme.css"
  "packages/ui/src/styles/colors.css"
  "packages/app/src/pages/error.tsx"
)

if ! command -v gh >/dev/null 2>&1; then
  echo "error: gh CLI is required" >&2
  exit 1
fi

mkdir -p "${OUT_DIR}"

echo "Validating tree for ${REPO}@${SHA}"
TREE_JSON="$(gh api "repos/${REPO}/git/trees/${SHA}?recursive=1")"

for path in "${FILES[@]}"; do
  if ! jq -e --arg path "$path" '.tree[] | select(.path == $path)' >/dev/null <<<"${TREE_JSON}"; then
    echo "error: missing path in tree: ${path}" >&2
    exit 1
  fi

done

echo "Fetching ${#FILES[@]} files to ${OUT_DIR}"
for path in "${FILES[@]}"; do
  target="${OUT_DIR}/${path}"
  mkdir -p "$(dirname "${target}")"
  curl -fsSL "https://raw.githubusercontent.com/${REPO}/${SHA}/${path}" -o "${target}"
  echo "fetched ${path}"
done

echo "done"
