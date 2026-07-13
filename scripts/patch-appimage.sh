#!/usr/bin/env bash
# Post-build step: patch AppRun in the AppImage to fix EGL on systems without GPU
set -e

APPIMAGE="$1"
if [ -z "$APPIMAGE" ]; then
    APPDIR="src-tauri/target/release/bundle/appimage"
    APPIMAGE=$(ls -t "$APPDIR"/*.AppImage 2>/dev/null | head -1)
fi

if [ ! -f "$APPIMAGE" ]; then
    echo "No AppImage found at $APPIMAGE"
    exit 1
fi

echo "Patching $APPIMAGE ..."

TMPDIR=$(mktemp -d)
cp "$APPIMAGE" "$TMPDIR"/fix.AppImage
chmod +x "$TMPDIR"/fix.AppImage

cd "$TMPDIR"
./fix.AppImage --appimage-extract > /dev/null 2>&1

sed -i '1a\
export WEBKIT_DISABLE_COMPOSITING_MODE=1\
export WEBKIT_DISABLE_ACCELERATED_2D_CANVAS=1\
export GSK_RENDERER=cairo' squashfs-root/AppRun

ARCH=x86_64 appimagetool squashfs-root "$APPIMAGE" > /dev/null 2>&1 || {
    # fallback: use bundled appimagetool
    if [ -f /tmp/appimagetool ]; then
        cd /tmp
        /tmp/appimagetool --appimage-extract > /dev/null 2>&1
        ARCH=x86_64 squashfs-root/AppRun "$TMPDIR"/squashfs-root "$APPIMAGE" > /dev/null 2>&1
    fi
}

rm -rf "$TMPDIR"
echo "Patched: $APPIMAGE"
