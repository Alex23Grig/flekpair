// Fake backend for working on the UI in a browser, without the Tauri shell or a device.
// Pick a scenario with ?mock=device|two|none|unnamed|error|denied|nousbmuxd|stuck
import { mockIPC } from "@tauri-apps/api/mocks";

const scenario = new URLSearchParams(location.search).get("mock") ?? "device";

const iphone = {
  udid: "00008110-000A1C2E3F90801E",
  name: "Alex’s iPhone",
  version: "18.5",
  deviceClass: "iPhone",
};
const ipad = {
  udid: "00008103-001D2B3A4C56701E",
  name: "Studio iPad Pro",
  version: "17.2",
  deviceClass: "iPad",
};

const devices = {
  none: [],
  two: [iphone, ipad],
  unnamed: [{ ...iphone, name: "", version: "", deviceClass: "" }],
}[scenario] ?? [iphone];

let cancel: (() => void) | null = null;

const wait = (ms: number) =>
  new Promise<void>((resolve, reject) => {
    const timer = setTimeout(resolve, ms);
    cancel = () => {
      clearTimeout(timer);
      reject({ type: "canceled", message: "Pairing canceled" });
    };
  });

mockIPC(async (cmd) => {
  switch (cmd) {
    case "plugin:app|version":
      return "1.0.0";
    case "list_devices":
      if (scenario === "nousbmuxd") {
        throw {
          type: "usbmuxd",
          message:
            "Failed to connect to usbmuxd: device socket io failed: Connection refused (os error 61)",
        };
      }
      return devices;
    case "export_pairing_file":
      await wait(scenario === "stuck" ? 600_000 : 2500);
      if (scenario === "error") {
        throw {
          type: "remote_pairing",
          message:
            "Failed to pair with device: unexpected response from device: missing pairing data in pair consent response",
        };
      }
      if (scenario === "denied") {
        throw {
          type: "trust_denied",
          message: "The trust prompt was declined on the device",
        };
      }
      return {
        path: "/Users/alex/Downloads/pairingFile.plist",
        fileName: "pairingFile.plist",
      };
    case "cancel_pairing":
      cancel?.();
      return;
    default:
      console.log("[dev-mock]", cmd);
  }
});
