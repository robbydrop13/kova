#!/bin/bash
set -e
cargo build --release

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
BINARY="$HOME/.cargo/target/release/kova"
BUNDLE="/Applications/Kova.app"
BUNDLE_BIN="$BUNDLE/Contents/MacOS/kova"

# Update bundle contents (binary + Info.plist for version)
if [ -d "$BUNDLE" ]; then
    rm -f "$BUNDLE_BIN"
    cp "$BINARY" "$BUNDLE_BIN"
    cp "$SCRIPT_DIR/Info.plist" "$BUNDLE/Contents/Info.plist"
fi

# Codesign (optional — requires a local signing certificate)
# Sign the entire .app bundle so macOS preserves TCC permissions across rebuilds.
# An ad-hoc signature changes on every build, which silently voids the TCC grants.
# Uses KOVA_SIGN_IDENTITY, else "Apple Development", else the self-signed "Kova Local Code Signing".
SIGN_IDENTITY="${KOVA_SIGN_IDENTITY:-}"
if [ -z "$SIGN_IDENTITY" ]; then
    for candidate in "Apple Development" "Kova Local Code Signing"; do
        if security find-identity -v -p codesigning | grep -q "\"$candidate"; then
            SIGN_IDENTITY="$candidate"
            break
        fi
    done
fi
SIGN_IDENTITY="${SIGN_IDENTITY:-Apple Development}"
if [ -d "$BUNDLE" ]; then
    if codesign --force --sign "$SIGN_IDENTITY" --identifier com.micktaiwan.kova --entitlements "$SCRIPT_DIR/Kova.entitlements" "$BUNDLE" 2>/dev/null; then
        echo "Build + codesign (bundle) done"
    else
        echo "Build done (codesign skipped — no certificate found)"
    fi
elif codesign --force --sign "$SIGN_IDENTITY" --identifier com.micktaiwan.kova --entitlements "$SCRIPT_DIR/Kova.entitlements" "$BINARY" 2>/dev/null; then
    echo "Build + codesign (binary) done"
else
    echo "Build done (codesign skipped — no certificate found)"
fi
