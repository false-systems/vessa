#!/bin/sh
# install.sh against a stand-in release on disk. A stub `curl` first on PATH
# serves the release files, so nothing here touches the network.
set -eu

tool=vessa
tag=v0.0.0
root=$(cd "$(dirname "$0")/.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT HUP INT TERM
unset VESSA_REPOSITORY VESSA_INSTALL_DIR

case "$(uname -s)" in
  Linux) platform=linux ;;
  Darwin) platform=macos ;;
  *) echo "no stand-in release for $(uname -s)" >&2; exit 1 ;;
esac
case "$(uname -m)" in
  x86_64|amd64) arch=x86_64 ;;
  *) arch=aarch64 ;;
esac
archive="$tool-$tag-$platform-$arch.tar.gz"
installed="$work/home/.local/bin/$tool"

# The stand-in release: an archive shaped like the workflow's, and its sums.
mkdir "$work/release" "$work/stage" "$work/bin" "$work/home"
printf '#!/bin/sh\necho stand-in\n' > "$work/stage/$tool"
echo licence > "$work/stage/LICENSE"
echo readme > "$work/stage/README.md"
tar -czf "$work/release/$archive" -C "$work/stage" "$tool" LICENSE README.md
sums() {
  (
    cd "$work/release"
    if [ "$platform" = macos ]; then shasum -a 256 "$archive"; else sha256sum "$archive"; fi
  ) > "$work/release/SHA256SUMS"
}
sums

# `curl -fsSLo <file> <url>`: note the URL, serve its last segment from disk.
cat > "$work/bin/curl" <<EOF
#!/bin/sh
echo "\$3" >> "$work/urls"
cp "$work/release/\${3##*/}" "\$2"
EOF
chmod +x "$work/bin/curl"

run() { HOME="$work/home" PATH="$work/bin:$PATH" sh "$root/install.sh" "$@"; }
refuses() {
  if run "$tag" >/dev/null 2>&1; then
    echo "install.sh installed $1" >&2
    exit 1
  fi
  if [ -e "$installed" ]; then
    echo "install.sh refused $1 but left a binary behind" >&2
    exit 1
  fi
}

# A good release installs, from the tool's own releases and nowhere else.
run "$tag" >/dev/null
test "$("$installed")" = stand-in
base="https://github.com/false-systems/$tool/releases/download/$tag"
printf '%s\n' "$base/$archive" "$base/SHA256SUMS" | cmp -s - "$work/urls"
rm -r "$work/home/.local"

# An archive that is not the one the sums name is refused.
echo tampered >> "$work/release/$archive"
refuses "an archive that does not match SHA256SUMS"

# So is an archive the sums do not name at all.
: > "$work/release/SHA256SUMS"
refuses "an archive SHA256SUMS does not list"

# A tag that is not a version is refused before anything is fetched.
code=0
run ../bad >/dev/null 2>&1 || code=$?
test "$code" -eq 2

echo "install.sh: ok"
