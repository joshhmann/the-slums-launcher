#!/usr/bin/env bash
# Post-build step: patch AppRun in the AppImage to fix EGL on systems without GPU
# Self-contained: downloads appimagetool if missing, verifies the patch landed.
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

# Locate appimagetool — download if absent.
APPIMAGETOOL=""
if command -v appimagetool >/dev/null 2>&1; then
    APPIMAGETOOL="appimagetool"
elif [ -x /tmp/appimagetool ]; then
    APPIMAGETOOL="/tmp/appimagetool --appimage-extract-and-run"
else
    echo "Downloading appimagetool..."
    curl -sL -o /tmp/appimagetool \
      https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage
    chmod +x /tmp/appimagetool
    APPIMAGETOOL="/tmp/appimagetool --appimage-extract-and-run"
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

# Verify the patch actually landed before repacking.
if ! grep -q "GSK_RENDERER=cairo" squashfs-root/AppRun; then
    echo "ERROR: AppRun patch did not apply" >&2
    rm -rf "$TMPDIR"
    exit 1
fi

ARCH=x86_64 $APPIMAGETOOL squashfs-root "$APPIMAGE" > /dev/null 2>&1 || {
    echo "ERROR: appimagetool repack failed" >&2
    rm -rf "$TMPDIR"
    exit 1
}

rm -rf "$TMPDIR"
echo "Patched: $APPIMAGE"

# Final verification on the output file.
TMP2=$(mktemp -d)
cp "$APPIMAGE" "$TMP2/verify.AppImage"
chmod +x "$TMP2/verify.AppImage"
cd "$TMP2"
./verify.AppImage --appimage-extract > /dev/null 2>&1
if grep -q "GSK_RENDERER=cairo" squashfs-root/AppRun; then
    echo "Verified: AppRun patched ✅"
    rm -rf "$TMP2"
else
    echo "ERROR: patched AppImage verification failed" >&2
    rm -rf "$TMP2"
    exit 1
fi
