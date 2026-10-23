# macOS Bluetooth permission

Tested on 2026-10-23 with Stream Deck 7.4.2.

macOS charges a helper's Bluetooth use to its "responsible process": the app
that launched it. Stream Deck.app has no `NSBluetoothAlwaysUsageDescription`,
so a helper started by the plugin as a plain child process is killed:

    namespace: TCC, signal SIGABRT, responsibleProc: Stream Deck
    "This app has crashed because it attempted to access privacy-sensitive
    data without a usage description."

What works:

1. The helper embeds its own Info.plist (`macos/Info.plist`, linked into the
   `__TEXT,__info_plist` section by `build.rs`) with a usage description.
2. It is ad-hoc signed with identifier `org.eeckhaut.deskfit` so the
   Info.plist is bound to the signature (`npm run build:helper`).
3. With `--disclaim`, the helper re-launches itself using the private
   `responsibility_spawnattrs_setdisclaim` spawn attribute (as LLDB and Qt
   Creator do), so it becomes its own responsible process. stdin, stdout and
   stderr are inherited.

Launched this way from the Stream Deck plugin, the helper saw the adapter
powered on and found the treadmill.

From a terminal, the helper runs under the terminal's own Bluetooth
permission and needs no `--disclaim`.

An ad-hoc signature's designated requirement is the binary's hash, so
macOS asked for Bluetooth access again after every rebuild.
`plugin/scripts/sign-helper.sh` therefore signs with `$DESKFIT_SIGN_IDENTITY`
or the first "Apple Development" identity, giving a stable requirement
(identifier plus certificate). It falls back to ad-hoc when neither exists.
Stale permission entries from earlier builds can be removed under System
Settings > Privacy & Security > Bluetooth.
