#!/usr/bin/env bash
# ava installer script
# usage: curl -sSL https://raw.githubusercontent.com/alextes/ava/main/install.sh | bash

set -euo pipefail

REPO="alextes/ava"
BINARY_NAME="ava"
INSTALL_DIR="${INSTALL_DIR:-${CARGO_HOME:-$HOME/.cargo}/bin}"

# detect platform
detect_platform() {
    local os arch

    case "$(uname -s)" in
        Linux*)  os="unknown-linux-gnu" ;;
        Darwin*) os="apple-darwin" ;;
        *)       echo "error: unsupported OS: $(uname -s)"; exit 1 ;;
    esac

    case "$(uname -m)" in
        x86_64)  arch="x86_64" ;;
        aarch64) arch="aarch64" ;;
        arm64)   arch="aarch64" ;;
        *)       echo "error: unsupported architecture: $(uname -m)"; exit 1 ;;
    esac

    echo "${arch}-${os}"
}

# get latest release version
get_latest_version() {
    curl -sSL "https://api.github.com/repos/${REPO}/releases/latest" \
        | grep '"tag_name":' \
        | sed -E 's/.*"([^"]+)".*/\1/'
}

# main
main() {
    echo "installing ava..."

    local platform version download_url tmp_dir

    platform=$(detect_platform)
    version=$(get_latest_version)

    if [[ -z "$version" ]]; then
        echo "error: could not determine latest version"
        echo ""
        echo "no releases found. install from source instead:"
        echo "  cargo install --git https://github.com/${REPO}.git"
        exit 1
    fi

    echo "  version:  ${version}"
    echo "  platform: ${platform}"

    download_url="https://github.com/${REPO}/releases/download/${version}/ava-${platform}.tar.xz"

    echo "  downloading from: ${download_url}"

    tmp_dir=$(mktemp -d)
    trap 'rm -rf "$tmp_dir"' EXIT

    if ! curl -sSL "$download_url" -o "${tmp_dir}/ava.tar.xz"; then
        echo "error: download failed"
        echo ""
        echo "the release may not have prebuilt binaries yet."
        echo "install from source instead:"
        echo "  cargo install --git https://github.com/${REPO}.git"
        exit 1
    fi

    tar -xJf "${tmp_dir}/ava.tar.xz" -C "$tmp_dir"

    mkdir -p "$INSTALL_DIR"

    # find the binary — cargo-dist extracts to ava-<target>/ subdirectory
    local binary_path
    if [[ -f "${tmp_dir}/ava-${platform}/${BINARY_NAME}" ]]; then
        binary_path="${tmp_dir}/ava-${platform}/${BINARY_NAME}"
    elif [[ -f "${tmp_dir}/${BINARY_NAME}" ]]; then
        binary_path="${tmp_dir}/${BINARY_NAME}"
    else
        echo "error: could not find binary in archive"
        echo "contents of tmp_dir:"
        ls -la "$tmp_dir"
        exit 1
    fi

    mv "$binary_path" "${INSTALL_DIR}/${BINARY_NAME}"
    chmod +x "${INSTALL_DIR}/${BINARY_NAME}"

    echo ""
    echo "installed ${BINARY_NAME} to ${INSTALL_DIR}/${BINARY_NAME}"
    echo ""

    # check if install dir is in PATH
    if [[ ":$PATH:" != *":${INSTALL_DIR}:"* ]]; then
        echo "note: ${INSTALL_DIR} is not in your PATH"
        echo "add it with:"
        echo "  export PATH=\"${INSTALL_DIR}:\$PATH\""
    else
        echo "run 'ava --help' to get started"
    fi
}

main "$@"
