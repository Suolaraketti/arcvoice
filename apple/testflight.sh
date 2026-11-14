#!/bin/bash
# Builds the iPhone app and uploads it to TestFlight.
#
# Needs: Xcode signed in to the Apple Developer account (Xcode > Settings >
# Accounts), and the app created in App Store Connect with the bundle ID
# com.kaczmarowski.arcvoice. Each upload gets a fresh build number from the
# current time, so TestFlight never rejects it as a duplicate.
set -euo pipefail
cd "$(dirname "$0")"

./build-core.sh
xcodegen
build_number="$(date +%Y%m%d%H%M)"
archive="build/ArcVoice-$build_number.xcarchive"

# Archived unsigned: development signing would need a registered iPhone.
# The export below signs it for App Store distribution, which does not.
xcodebuild archive \
    -project ArcVoice.xcodeproj -scheme ArcVoice -configuration Release \
    -destination "generic/platform=iOS" -archivePath "$archive" \
    CODE_SIGNING_ALLOWED=NO CURRENT_PROJECT_VERSION="$build_number"

xcodebuild -exportArchive \
    -archivePath "$archive" -exportOptionsPlist ExportOptions.plist \
    -exportPath build/export -allowProvisioningUpdates

echo "Uploaded build $build_number. It appears in TestFlight once Apple finishes processing it."
