#!/bin/sh
# Installs the latest Novo release:
#   curl -fsSL https://raw.githubusercontent.com/Simonf8/novo/master/install.sh | sh
# Set NOVO_VERSION (e.g. v0.2.0) to pick a release and NOVO_HOME to choose
# where it goes (default ~/.novo; the executable lands in $NOVO_HOME/bin).
set -eu

repo="Simonf8/novo"
home="${NOVO_HOME:-$HOME/.novo}"

case "$(uname -s)-$(uname -m)" in
    Linux-x86_64) target=x86_64-unknown-linux-gnu ;;
    Linux-aarch64 | Linux-arm64) target=aarch64-unknown-linux-gnu ;;
    Darwin-arm64) target=aarch64-apple-darwin ;;
    *)
        echo "novo: no prebuilt release for $(uname -s) $(uname -m); build from source with cargo" >&2
        exit 1
        ;;
esac

if [ -n "${NOVO_VERSION:-}" ]; then
    version="$NOVO_VERSION"
else
    version=$(curl -fsSL "https://api.github.com/repos/$repo/releases/latest" |
        sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n 1)
fi
if [ -z "$version" ]; then
    echo "novo: could not find the latest release" >&2
    exit 1
fi

name="novo-$version-$target"
base="https://github.com/$repo/releases/download/$version"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

echo "Downloading Novo $version for $target"
curl -fsSL "$base/$name.tar.gz" -o "$tmp/$name.tar.gz"
curl -fsSL "$base/SHA256SUMS" -o "$tmp/SHA256SUMS"
expected=$(grep " $name.tar.gz\$" "$tmp/SHA256SUMS" | cut -d ' ' -f 1)
if command -v sha256sum > /dev/null; then
    actual=$(sha256sum "$tmp/$name.tar.gz" | cut -d ' ' -f 1)
else
    actual=$(shasum -a 256 "$tmp/$name.tar.gz" | cut -d ' ' -f 1)
fi
if [ -z "$expected" ] || [ "$expected" != "$actual" ]; then
    echo "novo: checksum mismatch for $name.tar.gz" >&2
    exit 1
fi

tar xzf "$tmp/$name.tar.gz" -C "$tmp"
mkdir -p "$home/bin"
mv "$tmp/$name/novo" "$home/bin/novo"
chmod +x "$home/bin/novo"
echo "Installed $("$home/bin/novo" --version) to $home/bin/novo"

case ":$PATH:" in
    *":$home/bin:"*) ;;
    *) echo "Add it to your PATH:  export PATH=\"$home/bin:\$PATH\"" ;;
esac
