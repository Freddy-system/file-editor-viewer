#!/bin/sh
# Build step of the herdr plugin: puts the mdview binary at
# target/release/mdview. Downloads the prebuilt binary for this release when
# there is one for the platform, and compiles from source otherwise.
# MDVIEW_BUILD_FROM_SOURCE=1 skips the download.
set -eu

repo="Freddy-system/herdr-mdview"
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
    archive="mdview-$target.tar.gz"
    url="https://github.com/$repo/releases/download/v$version/$archive"
    tmp=$(mktemp -d)
    trap 'rm -rf "$tmp"' EXIT
    curl -fsSL "$url" -o "$tmp/$archive" 2>/dev/null || return 1
    curl -fsSL "$url.sha256" -o "$tmp/$archive.sha256" 2>/dev/null || return 1
    [ "$(checksum "$tmp/$archive")" = "$(cut -d ' ' -f 1 "$tmp/$archive.sha256")" ] || {
        echo "mdview: checksum mismatch for $archive" >&2
        return 1
    }
    tar -xzf "$tmp/$archive" -C "$tmp" || return 1
    "$tmp/mdview" --version >/dev/null 2>&1 || return 1
    mkdir -p "$dest"
    mv "$tmp/mdview" "$dest/mdview"
}

if [ -n "$target" ] && [ "${MDVIEW_BUILD_FROM_SOURCE:-0}" != "1" ] && command -v curl >/dev/null 2>&1; then
    if download; then
        echo "mdview: installed prebuilt $version for $target"
        exit 0
    fi
    echo "mdview: no prebuilt binary for $version on $target, building from source"
fi

exec cargo build --release --bin mdview
