#!/bin/bash
# Build script for Shard macOS distribution
# Simplified after removing Tesseract/Leptonica native dependencies

set -e

echo "=========================================="
echo "  Shard macOS Build Script"
echo "=========================================="
echo ""

# Step 1: Run Tauri build
echo "Step 1: Building Shard..."
npm run tauri build

# Detect the app bundle location
if [ -d "src-tauri/target/release/bundle/macos/Shard.app" ]; then
    APP_PATH="src-tauri/target/release/bundle/macos/Shard.app"
elif [ -d "src-tauri/target/aarch64-apple-darwin/release/bundle/macos/Shard.app" ]; then
    APP_PATH="src-tauri/target/aarch64-apple-darwin/release/bundle/macos/Shard.app"
elif [ -d "src-tauri/target/x86_64-apple-darwin/release/bundle/macos/Shard.app" ]; then
    APP_PATH="src-tauri/target/x86_64-apple-darwin/release/bundle/macos/Shard.app"
else
    echo "Error: Could not find Shard.app bundle"
    exit 1
fi

echo ""
echo "Step 2: Copying frameworks..."

# Create Frameworks directory
FRAMEWORKS="$APP_PATH/Contents/Frameworks"
mkdir -p "$FRAMEWORKS"

# Only libarchive is still needed (for zip/archive operations)
BREW_PREFIX="/opt/homebrew/opt"
LIBARCHIVE="$BREW_PREFIX/libarchive/lib/libarchive.13.dylib"

if [ -f "$LIBARCHIVE" ]; then
    BINARY="$APP_PATH/Contents/MacOS/shard"
    dst="$FRAMEWORKS/libarchive.13.dylib"
    echo "  Copying libarchive.13.dylib..."
    cp "$LIBARCHIVE" "$dst"
    chmod u+w "$dst"
    xattr -cr "$dst" 2>/dev/null || true

    # Fix dylib path
    echo "  Fixing dylib path..."
    install_name_tool -change "$LIBARCHIVE" "@executable_path/../Frameworks/libarchive.13.dylib" "$BINARY" 2>/dev/null || true
    install_name_tool -id "@executable_path/../Frameworks/libarchive.13.dylib" "$dst" 2>/dev/null || true
else
    echo "  Warning: libarchive not found, skipping..."
fi

echo ""
echo "Step 3: Re-signing the app bundle..."
# Using "Shard Dev" self-signed certificate for stable identity
codesign --force --deep --sign "Shard Dev" "$APP_PATH" || echo "  Warning: codesign failed (may need signing identity)"

echo ""
echo "Step 4: Creating DMG..."
DMG_DIR=$(dirname "$APP_PATH")

# Detect architecture from path
if [[ "$APP_PATH" == *"aarch64"* ]]; then
    ARCH="aarch64"
elif [[ "$APP_PATH" == *"x86_64"* ]]; then
    ARCH="x86_64"
else
    # Fall back to current machine architecture
    ARCH=$(uname -m)
    if [ "$ARCH" = "arm64" ]; then
        ARCH="aarch64"
    fi
fi

DMG_NAME="Shard_0.1.0_${ARCH}.dmg"
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
echo ""
echo "For cross-architecture builds:"
echo "  Intel: rustup target add x86_64-apple-darwin && npm run tauri build -- --target x86_64-apple-darwin"
echo "  ARM64: rustup target add aarch64-apple-darwin && npm run tauri build -- --target aarch64-apple-darwin"
