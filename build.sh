#!/bin/bash
set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
BINARY="$HOME/.cargo/target/release/kova"
BUNDLE="${KOVA_BUNDLE:-/Applications/Kova.app}"
BUNDLE_BIN="$BUNDLE/Contents/MacOS/kova"
# The identity the last build signed with: its SHA-1, then its name.
SIGN_STATE="${KOVA_SIGN_STATE:-$HOME/.config/kova/sign-identity}"

# --- Which identity signs -------------------------------------------------
# macOS ties the grants of Privacy & Security (Accessibility, Screen Recording,
# event posting) to the certificate that signed the app. A build signed with
# another certificate keeps its switch ON in System Settings and loses the
# grant all the same. So the identity never changes by itself: it is the one
# of the last build (SIGN_STATE, else the bundle's own signature), and a
# certificate that appears in the keychain meanwhile (one created by other
# tooling, say) is not picked up. Changing it takes KOVA_SIGN_IDENTITY, a
# name or a SHA-1 of `security find-identity -v -p codesigning`.
# With no identity at all the build is left unsigned, as before.
# `./build.sh --identity` stops after this choice and prints it.

identities() { security find-identity -v -p codesigning 2>/dev/null | grep -E '^ *[0-9]+\) [0-9A-F]{40} "' || true; }
# The SHA-1 of the valid identities whose SHA-1 or name starts with $1.
sha_of() {
    local want="$1" upper
    upper="$(printf '%s' "$want" | tr 'a-f' 'A-F')"
    identities | while read -r _ sha rest; do
        name="${rest#\"}"; name="${name%\"}"
        if [ "$sha" = "$upper" ] || [ "${name#"$want"}" != "$name" ]; then echo "$sha"; fi
    done
}
name_of() { identities | while read -r _ sha rest; do if [ "$sha" = "$1" ]; then n="${rest#\"}"; echo "${n%\"}"; fi; done; }
# The SHA-1 of the certificate that signed the bundle, if it is signed with one.
bundle_sha() {
    local tmp sha=""
    [ -d "$BUNDLE" ] || return 0
    tmp="$(mktemp -d)"
    if codesign -d --extract-certificates="$tmp/c" "$BUNDLE" 2>/dev/null && [ -f "$tmp/c0" ]; then
        sha="$(shasum -a 1 "$tmp/c0" | cut -d' ' -f1 | tr 'a-f' 'A-F')"
    fi
    rm -rf "$tmp"
    echo "$sha"
}
loud() {
    echo "" >&2
    echo "############################################################" >&2
    for arg in "$@"; do printf '%s\n' "$arg" | while IFS= read -r line; do echo "# $line" >&2; done; done
    echo "############################################################" >&2
    echo "" >&2
}

LAST_SHA=""
if [ -f "$SIGN_STATE" ]; then
    LAST_SHA="$(head -1 "$SIGN_STATE" | cut -d' ' -f1 | tr 'a-f' 'A-F')"
fi
[ -n "$LAST_SHA" ] || LAST_SHA="$(bundle_sha)"
LAST_NAME=""
[ -z "$LAST_SHA" ] || LAST_NAME="$(name_of "$LAST_SHA")"
last_label="$LAST_SHA${LAST_NAME:+ \"$LAST_NAME\"}"

SIGN_SHA=""
if [ -n "${KOVA_SIGN_IDENTITY:-}" ]; then
    matches="$(sha_of "$KOVA_SIGN_IDENTITY")"
    if [ "$(printf '%s\n' "$matches" | grep -c .)" != "1" ]; then
        loud "KOVA_SIGN_IDENTITY=\"$KOVA_SIGN_IDENTITY\" names $(printf '%s\n' "$matches" | grep -c .) valid signing identities, not one." \
             "Valid identities:" "$(identities)"
        exit 3
    fi
    SIGN_SHA="$matches"
    if [ -n "$LAST_SHA" ] && [ "$SIGN_SHA" != "$LAST_SHA" ]; then
        loud "SIGNING IDENTITY CHANGES, as KOVA_SIGN_IDENTITY asks:" \
             "  last build: $last_label" \
             "  this build: $SIGN_SHA \"$(name_of "$SIGN_SHA")\"" \
             "Kova's grants in System Settings > Privacy & Security are void after this build," \
             "even with their switch ON: remove Kova from each list and add it again."
    fi
elif [ -n "$LAST_SHA" ]; then
    if [ -n "$LAST_NAME" ]; then
        SIGN_SHA="$LAST_SHA"
    else
        loud "THE SIGNING IDENTITY OF THE LAST BUILD IS GONE, nothing is built:" \
             "  last build: $LAST_SHA (not a valid identity of the keychain: removed or expired)" \
             "  available:" "$(identities)" \
             "Signing with another one voids Kova's grants in System Settings > Privacy & Security." \
             "To do it on purpose: KOVA_SIGN_IDENTITY=<name or SHA-1> ./build.sh"
        exit 3
    fi
else
    # A first build: one identity is a choice, several are not.
    count="$(identities | grep -c . || true)"
    if [ "$count" = "1" ]; then
        SIGN_SHA="$(identities | awk '{print $2}')"
    elif [ "$count" != "0" ]; then
        loud "SEVERAL SIGNING IDENTITIES and no build to follow, nothing is built:" "$(identities)" \
             "Choose one: KOVA_SIGN_IDENTITY=<name or SHA-1> ./build.sh"
        exit 3
    fi
fi
# A certificate that is not the one in use is named, never taken.
if [ -n "$SIGN_SHA" ]; then
    others="$(identities | grep -v " $SIGN_SHA " || true)"
    if [ -n "$others" ]; then
        echo "Signing with $SIGN_SHA \"$(name_of "$SIGN_SHA")\"; other identities in the keychain, not used:"
        echo "$others"
    fi
fi
if [ "${1:-}" = "--identity" ]; then
    echo "${SIGN_SHA:+$SIGN_SHA \"$(name_of "$SIGN_SHA")\"}"
    exit 0
fi

cargo build --release

# Update bundle contents (binary + Info.plist for version)
if [ -d "$BUNDLE" ]; then
    rm -f "$BUNDLE_BIN"
    cp "$BINARY" "$BUNDLE_BIN"
    cp "$SCRIPT_DIR/Info.plist" "$BUNDLE/Contents/Info.plist"
fi

# Sign the entire .app bundle so macOS preserves TCC permissions across rebuilds.
# An ad-hoc signature changes on every build, which silently voids the TCC grants.
if [ -z "$SIGN_SHA" ]; then
    echo "Build done (codesign skipped — no certificate found)"
    exit 0
fi
if [ -d "$BUNDLE" ]; then
    TARGET="$BUNDLE"; WHAT="bundle"
else
    TARGET="$BINARY"; WHAT="binary"
fi
if ! codesign --force --sign "$SIGN_SHA" --identifier com.micktaiwan.kova --entitlements "$SCRIPT_DIR/Kova.entitlements" "$TARGET"; then
    loud "CODESIGN FAILED with $SIGN_SHA \"$(name_of "$SIGN_SHA")\":" \
         "the $WHAT is built and NOT signed with it; its grants will not hold."
    exit 4
fi
mkdir -p "$(dirname "$SIGN_STATE")"
echo "$SIGN_SHA $(name_of "$SIGN_SHA")" > "$SIGN_STATE"
echo "Build + codesign ($WHAT) done"
