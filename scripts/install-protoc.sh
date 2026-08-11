#!/usr/bin/env bash
set -euo pipefail

VERSION="23.4"
INSTALL_ROOT="${RUNNER_TEMP:?RUNNER_TEMP is required}/protoc-$VERSION"
OS="$(uname -s)"
ARCH="$(uname -m)"

case "$OS/$ARCH" in
    Linux/x86_64)
        ASSET="protoc-$VERSION-linux-x86_64.zip"
        EXPECTED_SHA256="0502f286ac9ed860b629a7965a14527b1f2dd131e4283fa23c2d7f184672aa9a"
        ;;
    Linux/aarch64|Linux/arm64)
        ASSET="protoc-$VERSION-linux-aarch_64.zip"
        EXPECTED_SHA256="1c7750b6e038305b5a7fc3d0cda1ebefdf106a4f30a787bf826ed2fc47c3967d"
        ;;
    Darwin/x86_64)
        ASSET="protoc-$VERSION-osx-x86_64.zip"
        EXPECTED_SHA256="07e5fdcf1b0708d3367dc5e6eb8d135de7e407d75316c93155cfd8ab362eec80"
        ;;
    Darwin/arm64)
        ASSET="protoc-$VERSION-osx-aarch_64.zip"
        EXPECTED_SHA256="8c7afae8626b6811e7b5897d16d940c2dbf50b1e135ed958a01db6566bdda726"
        ;;
    *)
        echo "ERROR: unsupported protoc host: $OS/$ARCH" >&2
        exit 1
        ;;
esac

ARCHIVE="$RUNNER_TEMP/$ASSET"
URL="https://github.com/protocolbuffers/protobuf/releases/download/v$VERSION/$ASSET"
curl --fail --location --silent --show-error "$URL" --output "$ARCHIVE"

if command -v sha256sum >/dev/null 2>&1; then
    ACTUAL_SHA256=$(sha256sum "$ARCHIVE" | awk '{print $1}')
else
    ACTUAL_SHA256=$(shasum -a 256 "$ARCHIVE" | awk '{print $1}')
fi

if [ "$ACTUAL_SHA256" != "$EXPECTED_SHA256" ]; then
    echo "ERROR: checksum mismatch for $ASSET" >&2
    exit 1
fi

mkdir -p "$INSTALL_ROOT"
unzip -q -o "$ARCHIVE" -d "$INSTALL_ROOT"
echo "$INSTALL_ROOT/bin" >> "${GITHUB_PATH:?GITHUB_PATH is required}"
"$INSTALL_ROOT/bin/protoc" --version
