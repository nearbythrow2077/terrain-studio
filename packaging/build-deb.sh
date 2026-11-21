#!/bin/sh
set -eu
umask 022

root=$(CDPATH="" cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"
name=terrain-studio
arch=$(dpkg-architecture -qDEB_BUILD_ARCH)
package_id=$(cargo pkgid --offline --locked)
version=${package_id##*[@#]}
target=$(rustc -vV | sed -n 's/^host: //p')
case "$target" in
    *-unknown-linux-gnu) ;;
    *) echo "Build the package with a native GNU/Linux Rust toolchain." >&2; exit 1 ;;
esac

cargo build --release --offline --locked --target-dir "$root/target"
mkdir -p "$root/dist"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
trap 'exit 1' HUP INT TERM
stage="$work/debian/$name"
docs="$stage/usr/share/doc/$name"

install -Dm755 "$root/target/release/$name" "$stage/usr/bin/$name"
install -Dm644 "packaging/$name.desktop" "$stage/usr/share/applications/$name.desktop"
install -Dm644 "packaging/$name.xml" "$stage/usr/share/mime/packages/$name.xml"
install -Dm644 "packaging/$name.png" "$stage/usr/share/icons/hicolor/128x128/apps/$name.png"
install -Dm644 LICENSE "$docs/LICENSE"
{
    cat packaging/copyright
    printf '\n'
    cat LICENSE
} > "$docs/copyright"
install -Dm644 README.md "$docs/README.md"
install -Dm644 docs/disc-analysis.md "$docs/docs/disc-analysis.md"
install -Dm644 docs/performance.md "$docs/docs/performance.md"
install -d "$docs/docs/images"
install -m644 docs/images/*.webp "$docs/docs/images/"
gzip -n -9c packaging/changelog > "$docs/changelog.gz"
install -d "$stage/usr/share/man/man1"
gzip -n -9c "packaging/$name.1" > "$stage/usr/share/man/man1/$name.1.gz"

cargo metadata --offline --locked --format-version 1 --filter-platform "$target" > "$work/dependencies.json"
python3 packaging/collect-licenses.py "$work/dependencies.json" > "$work/third-party-notices"
gzip -n -9c "$work/third-party-notices" > "$docs/third-party-notices.gz"

mkdir -p "$work/debian" "$stage/DEBIAN"
cat > "$work/debian/control" <<CONTROL
Source: $name
Section: graphics
Priority: optional
Maintainer: Terrain Studio Local Build <terrain-studio@example.org>

Package: $name
Architecture: any
Description: Native 3D scene editor and renderer
CONTROL
deps=$(cd "$work" && dpkg-shlibdeps -O -e "$stage/usr/bin/$name")
deps=${deps#shlibs:Depends=}
runtime='libegl1, libgl1, libwayland-client0, libwayland-cursor0, libwayland-egl1, libx11-6, libx11-xcb1, libxcursor1, libxext6, libxi6, libxrandr2, libxkbcommon0, libxkbcommon-x11-0, zenity'
size=$(du -sk "$stage/usr" | cut -f1)
cat > "$stage/DEBIAN/control" <<CONTROL
Package: $name
Version: $version
Architecture: $arch
Maintainer: Terrain Studio Local Build <terrain-studio@example.org>
Section: graphics
Priority: optional
Depends: $deps, $runtime
Recommends: libgl1-mesa-dri
Installed-Size: $size
Description: Native 3D scene editor and renderer
 Compose abstract scenes from geometric primitives and sculpted terrain.
 Edit procedural materials, skies, camera and reflections. Render a PNG
 from the desktop editor or command line. Read TIFF heightmaps from the
 original disc and import editable terrain, selected geometry, and camera
 settings from Bryce 2 scene files.
CONTROL

(cd "$stage" && find usr -type f -print0 | sort -z | xargs -0 md5sum) > "$stage/DEBIAN/md5sums"
archive="${name}_${version}_${arch}.deb"
dpkg-deb --root-owner-group --build "$stage" "$work/$archive"
mv "$work/$archive" "$root/dist/$archive"
printf '%s\n' "$root/dist/$archive"
