#!/bin/bash
set -euo pipefail

cd "$(dirname "$0")"

INSTALL=0
for arg in "$@"; do
  case "$arg" in
    --install) INSTALL=1 ;;
    -h|--help)
      echo "Usage: $0 [--install]"
      echo "  --install   Move DplyLocal.app into /Applications and launch it"
      exit 0
      ;;
  esac
done

echo "› Building release binary…"
swift build -c release

# One source of truth for the version: the workspace Cargo.toml. Sparkle
# compares CFBundleVersion between the running app and the appcast, so the
# app, the tag, and the appcast must all agree on this string.
VERSION=$(grep -m1 '^version' ../Cargo.toml | cut -d'"' -f2)
echo "› Version $VERSION (from Cargo.toml)"

# Generate the icon if missing or older than its generator.
if [ ! -f AppIcon.icns ] || [ make-icon.swift -nt AppIcon.icns ]; then
  echo "› Generating AppIcon.icns…"
  swift make-icon.swift
fi

APP="$HOME/Desktop/DplyLocal.app"
echo "› Assembling $APP"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources" "$APP/Contents/Frameworks"

cp .build/release/DplyLocal "$APP/Contents/MacOS/DplyLocal"
cp AppIcon.icns             "$APP/Contents/Resources/AppIcon.icns"

# Sparkle rides in Contents/Frameworks; the binary's rpath
# (@executable_path/../Frameworks, set in Package.swift) finds it there.
cp -R .build/release/Sparkle.framework "$APP/Contents/Frameworks/"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>                 <string>DplyLocal</string>
    <key>CFBundleDisplayName</key>          <string>Dply Local</string>
    <key>CFBundleIdentifier</key>           <string>com.tomshafer.dplylocal</string>
    <key>CFBundleVersion</key>              <string>${VERSION}</string>
    <key>CFBundleShortVersionString</key>   <string>${VERSION}</string>
    <key>CFBundleExecutable</key>           <string>DplyLocal</string>
    <key>CFBundlePackageType</key>          <string>APPL</string>
    <key>CFBundleSupportedPlatforms</key>   <array><string>MacOSX</string></array>
    <key>CFBundleIconFile</key>             <string>AppIcon</string>
    <key>CFBundleIconName</key>             <string>AppIcon</string>
    <key>LSMinimumSystemVersion</key>       <string>14.0</string>
    <key>NSHighResolutionCapable</key>      <true/>
    <key>NSHumanReadableCopyright</key>     <string>© 2026 Tom Shafer</string>
    <!-- Sparkle self-updates: the feed is an asset on the latest GitHub
         Release; updates are EdDSA-signed in CI (release.yml). -->
    <key>SUFeedURL</key>                    <string>https://github.com/shaferllc/dply-local/releases/latest/download/appcast.xml</string>
    <key>SUPublicEDKey</key>                <string>u0R+xh/MzqvsEHAtfH6BV5OuqAU022QuO4KhtGVoZKQ=</string>
    <key>SUEnableAutomaticChecks</key>      <true/>
    <key>SUScheduledCheckInterval</key>     <integer>86400</integer>
</dict>
</plist>
PLIST

# Ad-hoc sign for a stable identity.
codesign --force --deep --sign - "$APP" >/dev/null 2>&1 || true
touch "$APP"

if [ "$INSTALL" = "1" ]; then
  DEST="/Applications/DplyLocal.app"
  echo "› Installing to $DEST (will quit any running DplyLocal first)"
  /usr/bin/pkill -x DplyLocal 2>/dev/null || true
  /bin/sleep 0.3
  rm -rf "$DEST"
  /bin/mv "$APP" "$DEST"
  open "$DEST"
  echo "› Installed and launched."
else
  echo "› Done. Open with:  open '$APP'"
  echo "  Or run:  $0 --install   to drop it in /Applications."
fi
