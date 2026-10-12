<img align="left" width="90" height="90" src="/app-icon.svg" alt="">

# FlekPair

One-click pairing file exporter for iPhone and iPad, on macOS and Windows.

---

Plug in your device, click **Send to FlekDeck**, and FlekPair uploads the device's pairing file for the FlekDeck app on it to collect. It can save the file to your Downloads folder instead. That is the whole app.

FlekPair is an unofficial fork of [iloader](https://github.com/nab138/iloader) with everything except pairing-file export removed. It is not affiliated with or endorsed by iloader or its author.

## How to use

1. Connect your iPhone or iPad with a USB cable and unlock it. The device must have a passcode set.
2. Open FlekPair and click **Send to FlekDeck**.
3. If the device asks, tap **Trust** and enter your passcode.
4. When FlekPair says the file was uploaded, open FlekDeck on the device and tap **Refresh**.

The upload goes to FlekStore's server, under the device's UDID, and it is not encrypted. Read [what that means](#what-is-in-the-file) before using it for a device you care about.

A build that was given no server to upload to has no upload: its button reads **Export Pairing File** and saves the file, as below.

### Keeping the file on this computer

**Save to Downloads instead**, under the button, uploads nothing. The file lands in your Downloads folder as `pairingFile.plist` and is selected in Finder or Explorer. An existing file is never replaced: a second one is saved as `pairingFile (1).plist`.

With more than one device connected, pick the one you want from the list above the button.

The device's UDID is shown under its name as soon as it is plugged in, before it is trusted. Click it to copy.

### Without a cable

If you have connected the device to this computer before and both are on the same network, unlock it and it should appear in FlekPair marked **Wi-Fi**, with no cable. If it doesn't appear, connect it with a cable as usual. The empty list says the same.

The first connection, where you tap **Trust**, always needs a cable, and so does a device that has stopped trusting the computer. The device also has to be set to show up on Wi-Fi: **Show this iPhone when on Wi-Fi** in Finder, or **Sync with this iPhone over Wi-Fi** in iTunes or Apple Devices.

On a Mac, macOS finds the device. On Windows FlekPair searches the network itself and answers the device's heartbeat, so Apple's driver is enough and neither iTunes nor Bonjour is needed. To recognise a device there, it keeps the UDIDs of devices it has seen on a cable in its data folder.

A phone that nothing is talking to drops off the network within a minute. If yours doesn't show up, wake and unlock it, and give it a minute or two after it joins the network. Once FlekPair sees the phone it keeps a connection to it open, so it stays listed. It stops doing that ten minutes after you last used FlekPair, so that an app left open doesn't keep a phone awake.

If a device is both plugged in and on Wi-Fi, FlekPair uses the cable, and cabled devices come first in the list.

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

**Treat the file like a password.** Whoever holds it can connect to your device from the same network, with the standing of a computer you trusted.

**Send to FlekDeck** uploads the file as it is to FlekStore's server. The server gives it to anyone who asks for the device's UDID, and a UDID is not a secret. **Save to Downloads instead** keeps the file on this computer; on macOS and Linux it is written readable by your user only.

## How FlekDeck collects the file

FlekPair sends `POST <server>/<UDID>` with the pairing file as the body. The server takes nothing but property lists.

`GET` on the same address returns that file, or 404 when there is none. That is all FlekDeck's Refresh has to do.

`<server>` is not written in this repository. It is given to a build in `FLEKPAIR_SERVER`, and FlekDeck has to be given the same one.

## Troubleshooting

The app lists suggestions next to any error, and **Copy to clipboard** copies the technical message.

For more detail, start FlekPair from a terminal with `FLEKPAIR_LOG=debug` to print the device conversation to stderr. That output includes key material, so don't post it publicly.

## Building from source

1. Install [Node.js](https://nodejs.org) and [Rust](https://www.rust-lang.org/tools/install)
2. Clone the repository and `cd` into it
3. Run `npm install`

For development with hot reload: `npm run tauri dev`
Make a production build: `npm run tauri build`

Where pairing files are uploaded to is compiled in from the `FLEKPAIR_SERVER` environment variable, for example `FLEKPAIR_SERVER=https://files.example.com/pairing-files npm run tauri build`. Without it the build says so in a warning, and the app only saves the file.

To work on the interface without a device, run `npm run dev` and open <http://localhost:1420> in a browser. A stand-in backend takes over there; choose what it simulates with `?mock=device`, `wifi`, `two`, `mixed`, `none`, `unnamed`, `error`, `denied`, `sendfail`, `nosend`, `nousbmuxd` or `stuck`, and the language with `&lng=ru`.

The app icon is generated from `app-icon.svg`: `npm run tauri icon app-icon.svg`.

`cargo test` in `src-tauri` covers unpacking Apple's installer. If the driver setup stops working, `cargo test -- --ignored apples_installer` downloads the real installer and shows whether Apple has changed how it is packaged.

`FLEKPAIR_UDID=<udid> cargo test -- --ignored reaches_a_trusted_device` tries the network search for real. It needs that device to trust the computer and to be awake on the same network.

## Releasing

Every push to `main` and every pull request builds a universal macOS `.dmg` and a Windows installer, downloadable from the workflow run. Publishing a GitHub release attaches them to that release.

Released builds upload to the address in the `PAIRING_SERVER` repository secret, and a release fails to build without it. It is a secret rather than a variable only so that the public build logs don't print it; anyone with the app can still find it.

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
