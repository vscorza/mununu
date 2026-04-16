#!/usr/bin/env bash
# Install CIRCT pre-built binary (macOS or Linux)
# Downloads to /tmp/circt and adds to PATH.
#
# Usage: source tools/install_circt.sh
# After: circt-verilog, circt-opt available in PATH

set -euo pipefail

CIRCT_VERSION="firtool-1.144.0"
INSTALL_DIR="/tmp/circt"

ARCH=$(uname -m)
OS=$(uname -s)

if [ "$OS" = "Darwin" ]; then
    if [ "$ARCH" = "x86_64" ]; then PLATFORM="macos-x64"
    else PLATFORM="macos-arm64"
    fi
elif [ "$OS" = "Linux" ]; then
    PLATFORM="linux-x64"
else
    echo "Unsupported OS: $OS" >&2
    exit 1
fi

URL="https://github.com/llvm/circt/releases/download/${CIRCT_VERSION}/circt-full-shared-${PLATFORM}.tar.gz"

if [ -f "$INSTALL_DIR/$CIRCT_VERSION/bin/circt-opt" ]; then
    echo "CIRCT already installed at $INSTALL_DIR/$CIRCT_VERSION"
else
    echo "Downloading CIRCT ($PLATFORM) from $URL ..."
    mkdir -p "$INSTALL_DIR"
    cd "$INSTALL_DIR"
    curl -L "$URL" -o circt.tar.gz
    tar xzf circt.tar.gz
    rm circt.tar.gz
    echo "Installed CIRCT to $INSTALL_DIR/$CIRCT_VERSION"
fi

export PATH="$INSTALL_DIR/$CIRCT_VERSION/bin:$PATH"
if [ "$OS" = "Darwin" ]; then
    export DYLD_LIBRARY_PATH="$INSTALL_DIR/$CIRCT_VERSION/lib:${DYLD_LIBRARY_PATH:-}"
else
    export LD_LIBRARY_PATH="$INSTALL_DIR/$CIRCT_VERSION/lib:${LD_LIBRARY_PATH:-}"
fi

echo "circt-opt version: $(circt-opt --version 2>&1 | head -1)"
echo "circt-verilog available: $(which circt-verilog)"
