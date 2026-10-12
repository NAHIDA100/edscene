#!/usr/bin/env bash
# Build a .deb from an already-linked binary. No rust, no network, no fancy
# tooling: dpkg-deb is enough, which keeps the package reproducible.
#
#   ./packaging/build-deb.sh target/release/edscene edscene_0.1.0_amd64.deb
#
# VERSIONED=<ver> overrides the version, otherwise it is read from Cargo.toml.
set -euo pipefail

BIN="${1:?usage: build-deb.sh <binary> <out.deb>}"
OUT="${2:?usage: build-deb.sh <binary> <out.deb>}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VER="${VERSIONED:-$(sed -n 's/^version = "\([^"]*\)".*/\1/p' "$ROOT/Cargo.toml" | head -1)}"
ARCH="${DEB_ARCH:-amd64}"

command -v dpkg-deb >/dev/null || { echo "需要 dpkg-deb（Debian/Ubuntu）或 dpkg（macOS brew）" >&2; exit 1; }
[ -x "$BIN" ] || { echo "找不到可执行文件：$BIN" >&2; exit 1; }

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
PKG="$STAGE/edscene_${VER}_${ARCH}"

install -Dm755 "$BIN" "$PKG/usr/bin/edscene"
install -Dm644 /dev/null "$PKG/usr/share/doc/edscene/copyright"
install -Dm644 "$ROOT/NOTICE" "$PKG/usr/share/doc/edscene/copyright"
install -Dm644 /dev/null "$PKG/usr/share/metainfo/io.github.edscene.appdata.xml"
install -Dm644 /dev/null "$PKG/usr/share/applications/io.github.edscene.desktop"

cat > "$PKG/usr/share/applications/io.github.edscene.desktop" <<'EOF'
[Desktop Entry]
Type=Application
Name=edscene 播报设置
Name[en]=edscene settings
Comment=集成工业播报终端：登录、解锁与电源事件的语音播报
Exec=edscene
Icon=edscene
Terminal=false
Categories=Utility;
EOF

mkdir -p "$PKG/DEBIAN"
SIZE=$(du -sk "$PKG" | cut -f1)
cat > "$PKG/DEBIAN/control" <<EOF
Package: edscene
Version: $VER
Section: utils
Priority: optional
Architecture: $ARCH
Installed-Size: $SIZE
Maintainer: edscene contributors <noreply@example.invalid>
Homepage: https://github.com/edscene/edscene
Description: 明日方舟：终末地 风格的系统事件语音播报（同人二创）
 Fan-made system-event voice announcer styled after Arknights: Endfield.
 It watches logon, unlock, lock, screensaver, battery-saver, low-battery and
 AC adapter changes over D-Bus and plays a clip from a sound pack you supply.
 No game asset is included or redistributed; the package is useless until you
 run "edscene --sounds <folder>". Non-commercial fan work, not affiliated with
 or endorsed by Hypergryph; every related name, mark and artwork belongs to
 its own rights holder.
EOF

dpkg-deb --root-owner-group --build "$PKG" "$OUT"
echo "已生成 $OUT"
