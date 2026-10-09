<img align="left" width="90" height="90" src="/app-icon.svg" alt="">

# FlekPair

One-click pairing file exporter for iPhone and iPad, on macOS and Windows.

---

Plug in your device, click **Export Pairing File**, and FlekPair saves `pairingFile.plist` to your Downloads folder and shows it to you. That is the whole app.

FlekPair is an unofficial fork of [iloader](https://github.com/nab138/iloader) with everything except pairing-file export removed. It is not affiliated with or endorsed by iloader or its author.

## How to use

1. Connect your iPhone or iPad with a USB cable and unlock it. The device must have a passcode set.
2. Open FlekPair and click **Export Pairing File**.
3. If the device asks, tap **Trust** and enter your passcode.

The file lands in your Downloads folder as `pairingFile.plist` and is selected in Finder or Explorer. An existing file is never replaced: a second one is saved as `pairingFile (1).plist`.

Only devices connected by cable are listed. With more than one connected, pick the device from the list above the button.

The device's UDID is shown under its name as soon as it is plugged in, before it is trusted. Click it to copy.

### Windows: Apple's driver

Windows can't talk to an iPhone until Apple's device driver ("Apple Mobile Device Support") is installed. If you have iTunes, you already have it.

If it is missing, FlekPair says so and offers **Install Apple driver**. That downloads Apple's iTunes installer from apple.com (about 200 MB), takes only the driver package out of it, checks that Windows sees Apple's signature on it, and installs just that. Windows asks for permission once. Apple's license doesn't allow shipping the driver inside FlekPair, which is why it is fetched from Apple instead.

If you would rather install it yourself, the same screen links to Apple's [iTunes download](https://www.apple.com/itunes/download/win64) and to [Apple Devices](https://apps.microsoft.com/detail/9np83lwlpz9k) in the Microsoft Store. Either brings the driver with it, and FlekPair notices on its own once it is there.

If iTunes or Apple Devices from the Microsoft Store is installed, FlekPair asks you to open it instead of installing anything. The Store versions bring their own copy of the driver, which works while that app is running.

### Good to know

- macOS may ask once whether FlekPair can access your Downloads folder. If you decline, the file is saved to FlekPair's own data folder instead and shown there.
- Released Mac builds are signed and notarized by Apple, so macOS only asks once whether to open an app downloaded from the internet. The Windows installer is not signed, so Windows warns before the first launch; choose **More info → Run anyway**.
- The interface follows the system language where a full translation exists (18 languages inherited from iloader) and is English otherwise.

## What is in the file

The same combined pairing file iloader exports, for apps that ask for one, such as SideStore, StikDebug and LiveContainer:

- this computer's lockdown pairing record for the device, and
- on iOS 17.4 and later, a remote pairing (RPPairing) record created for this export.

Exporting also turns on Wi-Fi debugging on the device (`EnableWifiDebugging`), as iloader does, which those apps rely on.

**Treat the file like a password.** Whoever holds it can connect to your device. On macOS and Linux it is written readable by your user only.

## Troubleshooting

The app lists suggestions next to any error, and **Copy to clipboard** copies the technical message.

For more detail, start FlekPair from a terminal with `FLEKPAIR_LOG=debug` to print the device conversation to stderr. That output includes key material, so don't post it publicly.

## Building from source

1. Install [Node.js](https://nodejs.org) and [Rust](https://www.rust-lang.org/tools/install)
2. Clone the repository and `cd` into it
3. Run `npm install`

For development with hot reload: `npm run tauri dev`
Make a production build: `npm run tauri build`

To work on the interface without a device, run `npm run dev` and open <http://localhost:1420> in a browser. A stand-in backend takes over there; choose what it simulates with `?mock=device`, `two`, `none`, `unnamed`, `error`, `denied`, `nousbmuxd` or `stuck`, and the language with `&lng=ru`.

The app icon is generated from `app-icon.svg`: `npm run tauri icon app-icon.svg`.

`cargo test` in `src-tauri` covers unpacking Apple's installer. If the driver setup stops working, `cargo test -- --ignored apples_installer` downloads the real installer and shows whether Apple has changed how it is packaged.

## Releasing

Every push to `main` and every pull request builds a universal macOS `.dmg` and a Windows installer, downloadable from the workflow run. Publishing a GitHub release attaches them to that release.

macOS builds are signed with a Developer ID and notarized when these repository secrets exist: `DEV_ID_P12_BASE64`, `DEV_ID_P12_PASSWORD`, `DEV_IDENTITY_NAME`, `NOTARIZE_APPLE_ID`, `NOTARIZE_APP_SPECIFIC_PASS` and `NOTARIZE_TEAM_ID`. Without them the app is ad-hoc signed.

## Credits

- [iloader](https://github.com/nab138/iloader) by [nab138](https://github.com/nab138), which FlekPair is cut down from. If FlekPair is useful to you, consider [sponsoring nab138](https://github.com/sponsors/nab138).
- [idevice](https://github.com/jkcoxson/idevice) by [jkcoxson](https://github.com/jkcoxson) for communicating with iOS devices
- [idevice_pair](https://github.com/jkcoxson/idevice_pair) was used as a reference for pairing file management
- The [translators of iloader](https://github.com/nab138/iloader#translators), whose work the interface text comes from
- App made with [tauri](https://tauri.app)

## License

The source code of this repository is licensed under the MIT License. See the [LICENSE](/LICENSE) file for the full text.

The name "iloader" and its logo belong to nab138 and are covered by iloader's own [branding notice](https://github.com/nab138/iloader/blob/main/LICENSE-BRANDING). They are not part of FlekPair, which names iloader only to credit it.
