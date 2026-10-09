// Fake backend for working on the UI in a browser, without the Tauri shell or a device.
// Pick a scenario with ?mock=device|wifi|two|mixed|none|unnamed|error|denied|nousbmuxd|stuck, or with
// &platform=windows one of nodriver|driverfail|driverstopped|storeapp
import { mockIPC } from "@tauri-apps/api/mocks";

const scenario = new URLSearchParams(location.search).get("mock") ?? "device";

const iphone = {
  udid: "00008110-000A1C2E3F90801E",
  name: "Alex’s iPhone",
  version: "18.5",
  deviceClass: "iPhone",
  link: "usb",
};
const ipad = {
  // Devices older than the iPhone XS report the longer, undashed form.
  udid: "3f7a91c2d04e5b68a1f09c3d7e2b4a6c8d0e1f25",
  name: "Studio iPad Pro",
  version: "17.2",
  deviceClass: "iPad",
  link: "usb",
};

const devices = {
  none: [],
  wifi: [{ ...iphone, link: "network" }],
  two: [iphone, ipad],
  // The backend lists cabled devices first.
  mixed: [ipad, { ...iphone, link: "network" }],
  unnamed: [{ ...iphone, name: "", version: "", deviceClass: "" }],
}[scenario] ?? [iphone];

// Windows: Apple's driver starts out absent, stopped, or supplied by a Store app.
const driverScenarios: Record<string, string> = {
  nodriver: "missing",
  driverfail: "missing",
  driverstopped: "stopped",
  storeapp: "store_app",
};
const INSTALLER_BYTES = 202_759_648;
let driverReady = !(scenario in driverScenarios);
let driver = { stage: "idle", received: 0, total: 0 };

let cancel: (() => void) | null = null;

const wait = (ms: number) =>
  new Promise<void>((resolve, reject) => {
    const timer = setTimeout(resolve, ms);
    cancel = () => {
      clearTimeout(timer);
      reject({ type: "canceled", message: "Pairing canceled" });
    };
  });

const setUpDriver = async () => {
  if (scenario !== "driverstopped") {
    driver = { stage: "downloading", received: 0, total: INSTALLER_BYTES };
    for (let step = 1; step <= 20; step++) {
      await wait(180);
      driver.received = (INSTALLER_BYTES * step) / 20;
    }
    if (scenario === "driverfail") {
      throw {
        type: "driver",
        message:
          "Download from Apple failed: error sending request for url (https://www.apple.com/itunes/download/win64): operation timed out",
      };
    }
    for (const stage of ["unpacking", "verifying", "installing"]) {
      driver.stage = stage;
      await wait(900);
    }
  }
  driver.stage = "starting";
  await wait(1200);
  driverReady = true;
};

mockIPC(async (cmd) => {
  switch (cmd) {
    case "plugin:app|version":
      return "1.0.0";
    // Apple's symbols come from macOS itself, so a browser shows the drawn ones.
    case "system_symbol":
      return null;
    case "list_devices":
      if (!driverReady) {
        throw {
          type: "usbmuxd",
          message:
            "Failed to connect to usbmuxd: device socket io failed: No connection could be made because the target machine actively refused it. (os error 10061)",
        };
      }
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
    case "cancel_apple_driver":
      cancel?.();
      return;
    case "apple_driver_state":
      return driverScenarios[scenario] ?? "unsupported";
    case "apple_driver_progress":
      return driver;
    case "install_apple_driver":
      try {
        await setUpDriver();
      } finally {
        driver = { stage: "idle", received: 0, total: 0 };
      }
      return;
    default:
      console.log("[dev-mock]", cmd);
  }
});
