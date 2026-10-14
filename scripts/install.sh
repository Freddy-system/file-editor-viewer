#!/bin/sh
# Build step of the herdr plugin: puts the fev binary at
# target/release/fev. Downloads the prebuilt binary for this release when
# there is one for the platform, and compiles from source otherwise.
# FEV_BUILD_FROM_SOURCE=1 skips the download.
set -eu

repo="Freddy-system/file-editor-viewer"
version=$(sed -n 's/^version = "\(.*\)"$/\1/p' herdr-plugin.toml | head -n 1)
dest="target/release"

case "$(uname -s)-$(uname -m)" in
    Linux-x86_64) target="x86_64-unknown-linux-musl" ;;
    Linux-aarch64 | Linux-arm64) target="aarch64-unknown-linux-musl" ;;
    Darwin-x86_64) target="x86_64-apple-darwin" ;;
    Darwin-arm64) target="aarch64-apple-darwin" ;;
    *) target="" ;;
esac

checksum() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | cut -d ' ' -f 1
    else
        shasum -a 256 "$1" | cut -d ' ' -f 1
    fi
}

download() {
    archive="fev-$target.tar.gz"
    url="https://github.com/$repo/releases/download/v$version/$archive"
    tmp=$(mktemp -d)
    trap 'rm -rf "$tmp"' EXIT
    curl -fsSL "$url" -o "$tmp/$archive" 2>/dev/null || return 1
    curl -fsSL "$url.sha256" -o "$tmp/$archive.sha256" 2>/dev/null || return 1
    [ "$(checksum "$tmp/$archive")" = "$(cut -d ' ' -f 1 "$tmp/$archive.sha256")" ] || {
        echo "fev: checksum mismatch for $archive" >&2
        return 1
    }
    tar -xzf "$tmp/$archive" -C "$tmp" || return 1
    "$tmp/fev" --version >/dev/null 2>&1 || return 1
    mkdir -p "$dest"
    mv "$tmp/fev" "$dest/fev"
}

if [ -n "$target" ] && [ "${FEV_BUILD_FROM_SOURCE:-0}" != "1" ] && command -v curl >/dev/null 2>&1; then
    if download; then
        echo "fev: installed prebuilt $version for $target"
        exit 0
    fi
    echo "fev: no prebuilt binary for $version on $target, building from source"
fi

exec cargo build --release --bin fev
