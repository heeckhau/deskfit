#!/bin/sh
# Sign the helper so macOS keeps its Bluetooth permission across rebuilds.
#
# An ad-hoc signature identifies the binary by its hash, which changes with
# every build, so macOS asks for Bluetooth access again. A certificate
# identity is stable. Uses $DESKFIT_SIGN_IDENTITY if set, else the first
# "Apple Development" identity in the keychain, else ad-hoc.
set -eu

binary="$1"
identity="${DESKFIT_SIGN_IDENTITY:-}"
if [ -z "$identity" ]; then
	identity=$(security find-identity -v -p codesigning 2>/dev/null |
		sed -n 's/.*\([0-9A-F]\{40\}\) "Apple Development: .*/\1/p' | head -n 1)
fi
if [ -z "$identity" ]; then
	echo "sign-helper: no signing identity found; signing ad-hoc (macOS will ask for Bluetooth access after each rebuild)" >&2
	identity="-"
fi

codesign --force --sign "$identity" --identifier org.eeckhaut.deskfit --timestamp=none "$binary"
codesign --display --requirements - "$binary" 2>&1 | sed -n 's/^designated => /sign-helper: designated requirement: /p'
