#!/bin/bash
set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
BINARY="$HOME/.cargo/target/release/kova"
BUNDLE="${KOVA_BUNDLE:-/Applications/Kova.app}"
BUNDLE_BIN="$BUNDLE/Contents/MacOS/kova"
# The ONE identity every build signs with: its SHA-1 and name on the first
# line, then `dr <designated requirement>` once a build was signed with it.
SIGN_STATE="${KOVA_SIGN_STATE:-$HOME/.config/kova/sign-identity}"
BUNDLE_ID="com.micktaiwan.kova"

# --- Which identity signs -------------------------------------------------
# macOS ties each privacy grant (Accessibility, Input Monitoring, Screen
# Recording, event posting) to the designated requirement of the signature it
# was given to. A build signed with another certificate keeps its switch ON in
# System Settings and loses the grant all the same: on 15 November 2026 the
# Accessibility and Screen Recording grants were tied to one certificate and
# the builds of the last days were signed with another, so every relaunch
# asked again.
# So there is ONE identity, pinned in SIGN_STATE, and nothing changes it by
# accident: not a new certificate in the keychain, not an environment
# variable, not the signature the bundle happens to carry.
#   ./build.sh --pin <name or SHA-1>   pins it, on purpose, and builds nothing
#   ./build.sh --identity              prints the pin and builds nothing
# No pin, or a pinned identity that left the keychain: nothing is built.
# There is no ad hoc fallback. A machine with no certificate at all builds
# unsigned with KOVA_UNSIGNED=1, and says so.
# After signing, the designated requirement is compared with the one recorded
# for the pin: a difference fails the build (exit 4).
identities() { security find-identity -v -p codesigning 2>/dev/null | grep -E '^ *[0-9]+\) [0-9A-F]{40} "' || true; }
# The SHA-1 of the valid identities whose SHA-1 or name starts with $1.
sha_of() {
    local want="$1" upper
    upper="$(printf '%s' "$want" | tr 'a-f' 'A-F')"
    identities | while read -r _ sha rest; do
        name="${rest#\"}"; name="${name%\"}"
        if [ "${sha#"$upper"}" != "$sha" ] || [ "${name#"$want"}" != "$name" ]; then echo "$sha"; fi
    done
}
name_of() { identities | while read -r _ sha rest; do if [ "$sha" = "$1" ]; then n="${rest#\"}"; echo "${n%\"}"; fi; done; }
# The designated requirement of a signed bundle or binary, on one line.
dr_of() { codesign -d -r- "$1" 2>/dev/null | sed -n 's/^designated => //p' | head -1; }
loud() {
    echo "" >&2
    echo "############################################################" >&2
    for arg in "$@"; do printf '%s\n' "$arg" | while IFS= read -r line; do echo "# $line" >&2; done; done
    echo "############################################################" >&2
    echo "" >&2
}

PIN_SHA=""; PIN_NAME=""; PIN_DR=""
if [ -f "$SIGN_STATE" ]; then
    PIN_SHA="$(head -1 "$SIGN_STATE" | cut -d' ' -f1 | tr 'a-f' 'A-F')"
    PIN_NAME="$(head -1 "$SIGN_STATE" | cut -d' ' -f2-)"
    PIN_DR="$(sed -n 's/^dr //p' "$SIGN_STATE" | head -1)"
fi

if [ "${1:-}" = "--pin" ]; then
    want="${2:-}"
    [ -n "$want" ] || { echo "usage: ./build.sh --pin <name or SHA-1>" >&2; identities >&2; exit 2; }
    matches="$(sha_of "$want")"
    if [ "$(printf '%s\n' "$matches" | grep -c .)" != "1" ]; then
        loud "\"$want\" names $(printf '%s\n' "$matches" | grep -c .) valid signing identities, not one." "$(identities)"
        exit 3
    fi
    if [ -n "$PIN_SHA" ] && [ "$PIN_SHA" != "$matches" ]; then
        loud "THE SIGNING IDENTITY CHANGES, on purpose:" \
             "  was: $PIN_SHA \"$PIN_NAME\"" \
             "  now: $matches \"$(name_of "$matches")\"" \
             "Grants given to the old signature stop holding at the next build: in" \
             "System Settings > Privacy & Security, remove Kova from each list and add it" \
             "again, unless they were given to the new one (kova doctor tells)."
    fi
    mkdir -p "$(dirname "$SIGN_STATE")"
    # Same identity pinned again: its recorded requirement is kept.
    if [ "$PIN_SHA" = "$matches" ] && [ -n "$PIN_DR" ]; then
        printf '%s %s\ndr %s\n' "$matches" "$(name_of "$matches")" "$PIN_DR" > "$SIGN_STATE"
    else
        printf '%s %s\n' "$matches" "$(name_of "$matches")" > "$SIGN_STATE"
    fi
    echo "Pinned: $matches \"$(name_of "$matches")\""
    exit 0
fi

if [ "${1:-}" = "--identity" ]; then
    echo "${PIN_SHA:+$PIN_SHA \"$PIN_NAME\"}"
    exit 0
fi

if [ -n "${KOVA_SIGN_IDENTITY:-}" ]; then
    loud "KOVA_SIGN_IDENTITY no longer chooses the identity, nothing is built." \
         "The pin is $SIGN_STATE${PIN_SHA:+: $PIN_SHA \"$PIN_NAME\"}." \
         "To change it on purpose: ./build.sh --pin <name or SHA-1>"
    exit 3
fi

SIGN_SHA=""
if [ -n "$PIN_SHA" ]; then
    if [ -z "$(name_of "$PIN_SHA")" ]; then
        loud "THE PINNED SIGNING IDENTITY IS GONE, nothing is built:" \
             "  pinned: $PIN_SHA \"$PIN_NAME\" (not a valid identity of the keychain: removed or expired)" \
             "  valid identities:" "$(identities)" \
             "Signing with another one voids Kova's grants in System Settings > Privacy & Security." \
             "To do it on purpose: ./build.sh --pin <name or SHA-1>"
        exit 3
    fi
    SIGN_SHA="$PIN_SHA"
elif [ "${KOVA_UNSIGNED:-}" = "1" ]; then
    loud "KOVA_UNSIGNED=1: this build is NOT signed. macOS will not keep any privacy grant for it."
else
    loud "NO SIGNING IDENTITY IS PINNED, nothing is built:" \
         "  valid identities:" "$(identities)" \
         "Pin one, once: ./build.sh --pin <name or SHA-1>" \
         "(a machine with no certificate: KOVA_UNSIGNED=1 ./build.sh)"
    exit 3
fi

# The bundle id is part of the designated requirement: it never changes either.
plist_id="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$SCRIPT_DIR/Info.plist" 2>/dev/null || true)"
if [ "$plist_id" != "$BUNDLE_ID" ]; then
    loud "Info.plist carries the bundle id \"$plist_id\", not \"$BUNDLE_ID\": nothing is built." \
         "A new bundle id is a new app for macOS, with no grant."
    exit 3
fi

cargo build --release

# Update bundle contents (binary + Info.plist for version)
if [ -d "$BUNDLE" ]; then
    rm -f "$BUNDLE_BIN"
    cp "$BINARY" "$BUNDLE_BIN"
    cp "$SCRIPT_DIR/Info.plist" "$BUNDLE/Contents/Info.plist"
fi

# Sign the entire .app bundle so macOS preserves TCC permissions across rebuilds.
if [ -z "$SIGN_SHA" ]; then
    echo "Build done, NOT signed (KOVA_UNSIGNED=1)"
    exit 0
fi
if [ -d "$BUNDLE" ]; then
    TARGET="$BUNDLE"; WHAT="bundle"
else
    TARGET="$BINARY"; WHAT="binary"
fi
if ! codesign --force --sign "$SIGN_SHA" --identifier "$BUNDLE_ID" --entitlements "$SCRIPT_DIR/Kova.entitlements" "$TARGET"; then
    loud "CODESIGN FAILED with $SIGN_SHA \"$PIN_NAME\":" \
         "the $WHAT is built and NOT signed with it; its grants will not hold."
    exit 1
fi

# What macOS will compare the grants with. It must be the one of the pin.
NOW_DR="$(dr_of "$TARGET")"
if [ -z "$NOW_DR" ]; then
    loud "The signed $WHAT has no designated requirement to read: its grants cannot be checked."
    exit 4
fi
case "$NOW_DR" in
    *"identifier \"$BUNDLE_ID\""*) ;;
    *) loud "The signature does not carry the bundle id \"$BUNDLE_ID\":" "  $NOW_DR"; exit 4 ;;
esac
if [ -n "$PIN_DR" ] && [ "$NOW_DR" != "$PIN_DR" ]; then
    loud "THE DESIGNATED REQUIREMENT CHANGED, with the same pinned identity:" \
         "  recorded: $PIN_DR" \
         "  this build: $NOW_DR" \
         "macOS will not apply the existing grants to this build." \
         "If the change is wanted, pin again: ./build.sh --pin $SIGN_SHA, after removing the dr line of $SIGN_STATE."
    exit 4
fi
mkdir -p "$(dirname "$SIGN_STATE")"
printf '%s %s\ndr %s\n' "$SIGN_SHA" "$(name_of "$SIGN_SHA")" "$NOW_DR" > "$SIGN_STATE"
if [ -n "$PIN_DR" ]; then note="requirement unchanged"; else note="requirement recorded"; fi
echo "Build + codesign ($WHAT) done, signed by \"$(name_of "$SIGN_SHA")\", $note"
