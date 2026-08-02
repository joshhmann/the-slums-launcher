#!/usr/bin/env bash
# Post-build step: patch AppRun in the AppImage to fix EGL on systems without GPU.
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

# Locate appimagetool — download and EXTRACT it if absent. Using the extracted
# binary avoids FUSE and the flaky --appimage-extract-and-run path.
AITOOL_DIR="/tmp/appimagetool-extracted"
if [ -x "$AITOOL_DIR/squashfs-root/AppRun" ]; then
    :
elif command -v appimagetool >/dev/null 2>&1; then
    AITOOL_DIR=""
    APPIMAGETOOL_CMD="appimagetool"
else
    echo "Downloading appimagetool..."
    curl -sL -o /tmp/appimagetool \
      https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage
    chmod +x /tmp/appimagetool
    mkdir -p "$AITOOL_DIR"
    (cd "$AITOOL_DIR" && /tmp/appimagetool --appimage-extract > /dev/null 2>&1)
fi
if [ -z "${APPIMAGETOOL_CMD:-}" ]; then
    APPIMAGETOOL_CMD="$AITOOL_DIR/squashfs-root/AppRun"
fi

echo "Patching $APPIMAGE ..."

TMPDIR=$(mktemp -d)
cp "$APPIMAGE" "$TMPDIR/fix.AppImage"
chmod +x "$TMPDIR/fix.AppImage"

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

# Repack and capture output to a log for debugging.
REPACK_LOG=$(mktemp)
if ! ARCH=x86_64 "$APPIMAGETOOL_CMD" squashfs-root "$APPIMAGE" > "$REPACK_LOG" 2>&1; then
    echo "ERROR: appimagetool repack failed:" >&2
    grep -iE "error|fail" "$REPACK_LOG" | head -10 >&2
    rm -rf "$TMPDIR" "$REPACK_LOG"
    exit 1
fi
rm -f "$REPACK_LOG"

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
