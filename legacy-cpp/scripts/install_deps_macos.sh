#!/usr/bin/env bash
set -euo pipefail

# Ensure Command Line Tools
if ! xcode-select -p >/dev/null 2>&1; then
  xcode-select --install || true
fi

# Install Homebrew if missing
if ! command -v brew >/dev/null 2>&1; then
  /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
fi

brew update
brew install cmake ninja lua pkg-config git

echo "Dependencies installed."
