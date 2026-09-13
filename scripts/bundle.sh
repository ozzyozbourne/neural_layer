#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
build_profile="${POC_PROFILE:-release}"
case "$build_profile" in
  release) cargo build --locked --release -p cordis-host ;;
  debug) cargo build --locked -p cordis-host ;;
  *) echo "POC_PROFILE must be debug or release" >&2; exit 1 ;;
esac
bundle_dir="$PWD/target/Harness POC.app"
mkdir -p "$bundle_dir/Contents/MacOS"
cp "target/$build_profile/cordis-host" "$bundle_dir/Contents/MacOS/cordis-host"
cat > "$bundle_dir/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>dev.cordis.harness-poc</string>
<key>CFBundleExecutable</key><string>cordis-host</string>
<key>CFBundleName</key><string>Harness POC</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleVersion</key><string>1</string>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
