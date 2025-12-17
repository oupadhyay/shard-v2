#!/bin/bash
# Complete build script for Shard macOS distribution
# This script handles the full build process including dylib bundling and DMG creation

set -e

echo "=========================================="
echo "  Shard macOS Build Script"
echo "=========================================="
echo ""

# Step 1: Run Tauri build (without frameworks - we add them manually)
echo "Step 1: Building Shard..."
npm run tauri build

# Detect the app bundle location
if [ -d "src-tauri/target/release/bundle/macos/Shard.app" ]; then
    APP_PATH="src-tauri/target/release/bundle/macos/Shard.app"
elif [ -d "src-tauri/target/aarch64-apple-darwin/release/bundle/macos/Shard.app" ]; then
    APP_PATH="src-tauri/target/aarch64-apple-darwin/release/bundle/macos/Shard.app"
else
    echo "Error: Could not find Shard.app bundle"
    exit 1
fi

BINARY="$APP_PATH/Contents/MacOS/shard"
FRAMEWORKS="$APP_PATH/Contents/Frameworks"

echo ""
echo "Step 2: Copying frameworks..."

# Create Frameworks directory
mkdir -p "$FRAMEWORKS"

# Dylib sources (Homebrew)
BREW_PREFIX="/opt/homebrew/opt"
DYLIBS=(
    "$BREW_PREFIX/libarchive/lib/libarchive.13.dylib"
    "$BREW_PREFIX/tesseract/lib/libtesseract.5.dylib"
    "$BREW_PREFIX/leptonica/lib/libleptonica.6.dylib"
)

# Copy dylibs with write permissions
for src in "${DYLIBS[@]}"; do
    name=$(basename "$src")
    dst="$FRAMEWORKS/$name"
    echo "  Copying $name..."
    cp "$src" "$dst"
    chmod u+w "$dst"
    xattr -cr "$dst" 2>/dev/null || true
done

echo ""
echo "Step 3: Fixing dylib paths..."

# Dylib mappings: name old_path
fix_dylib_path() {
    local name="$1"
    local old_path="$2"
    local new_path="@executable_path/../Frameworks/$name"

    echo "  $name: rewriting binary path..."
    install_name_tool -change "$old_path" "$new_path" "$BINARY" 2>/dev/null || true
}

fix_dylib_path "libarchive.13.dylib" "$BREW_PREFIX/libarchive/lib/libarchive.13.dylib"
fix_dylib_path "libtesseract.5.dylib" "$BREW_PREFIX/tesseract/lib/libtesseract.5.dylib"
fix_dylib_path "libleptonica.6.dylib" "$BREW_PREFIX/leptonica/lib/libleptonica.6.dylib"

echo ""
echo "Step 4: Fixing inter-library dependencies..."

# Fix each bundled dylib's internal references
fix_inter_deps() {
    local dylib_file="$1"
    local dylib_name=$(basename "$dylib_file")

    # Set the dylib's own ID
    install_name_tool -id "@executable_path/../Frameworks/$dylib_name" "$dylib_file" 2>/dev/null || true

    # Fix references to other bundled dylibs
    install_name_tool -change "$BREW_PREFIX/libarchive/lib/libarchive.13.dylib" "@executable_path/../Frameworks/libarchive.13.dylib" "$dylib_file" 2>/dev/null || true
    install_name_tool -change "$BREW_PREFIX/tesseract/lib/libtesseract.5.dylib" "@executable_path/../Frameworks/libtesseract.5.dylib" "$dylib_file" 2>/dev/null || true
    install_name_tool -change "$BREW_PREFIX/leptonica/lib/libleptonica.6.dylib" "@executable_path/../Frameworks/libleptonica.6.dylib" "$dylib_file" 2>/dev/null || true
}

for dylib_file in "$FRAMEWORKS"/*.dylib; do
    echo "  Fixing $(basename "$dylib_file")..."
    fix_inter_deps "$dylib_file"
done

echo ""
echo "Step 5: Re-signing the app bundle..."
codesign --force --deep --sign - "$APP_PATH"

echo ""
echo "Step 6: Verifying..."
echo "Binary dependencies:"
otool -L "$BINARY" | grep -E "(libarchive|libtesseract|libleptonica)"

echo ""
echo "Step 7: Creating DMG..."
DMG_DIR=$(dirname "$APP_PATH")
DMG_NAME="Shard_0.1.0_aarch64.dmg"
DMG_PATH="$DMG_DIR/$DMG_NAME"
rm -f "$DMG_PATH"
hdiutil create -volname "Shard" -srcfolder "$APP_PATH" -ov -format UDZO "$DMG_PATH"

echo ""
echo "=========================================="
echo "  Build Complete!"
echo "=========================================="
echo ""
echo "Outputs:"
echo "  App: $APP_PATH"
echo "  DMG: $DMG_PATH"
echo ""
echo "To install: Open the DMG and drag Shard to Applications."
