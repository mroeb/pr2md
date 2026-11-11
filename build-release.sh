#!/bin/sh
# Build a pr2md release archive for Linux or macOS:
#   dist/pr2md-<version>-<os>-<arch>.tar.gz
#
#   ./build-release.sh
#   PR2MD_CLIENT_ID=Ov23li... ./build-release.sh   # bake in the OAuth app for "Sign in with GitHub"
#   SKIP_TESTS=1 ./build-release.sh
set -eu
cd "$(dirname "$0")"

# Files that go into the release, next to the binary
files="install.sh README.md RELEASE_NOTES.md"

version=$(sed -n 's/^version *= *"\(.*\)"/\1/p' Cargo.toml | head -n 1)
case "$(uname -s)" in
    Darwin) os=macos ;;
    *) os=linux ;;
esac
case "$(uname -m)" in
    arm64|aarch64) arch=arm64 ;;
    *) arch=x64 ;;
esac
name="pr2md-$version-$os-$arch"

if [ -n "${PR2MD_CLIENT_ID:-}" ]; then
    echo "OAuth client id: baked in"
else
    echo "OAuth client id: none (users sign in via GitHub CLI or a token, or set client_id)"
fi

[ -n "${SKIP_TESTS:-}" ] || cargo test --release
cargo build --release

rm -rf "dist/$name" "dist/$name.tar.gz"
mkdir -p "dist/$name"
cp target/release/pr2md "dist/$name/"
for f in $files; do cp "$f" "dist/$name/"; done
chmod 755 "dist/$name/pr2md" "dist/$name/install.sh"
tar -C dist -czf "dist/$name.tar.gz" "$name"

echo
echo "Release: dist/$name.tar.gz ($(du -h "dist/$name.tar.gz" | cut -f1))"
ls -1 "dist/$name" | sed "s|^|  $name/|"
