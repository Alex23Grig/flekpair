import { useCallback, useEffect, useState } from "react";
import "./App.css";
import { invoke } from "@tauri-apps/api/core";
import { getVersion } from "@tauri-apps/api/app";
import { useTranslation } from "react-i18next";
import logo from "../app-icon.svg";
import { AppleDriver } from "./AppleDriver";
import { AppError, getErrorSuggestions, platform, toAppError } from "./errors";
import { ExternalLink, Suggestions } from "./Suggestions";

type DeviceInfo = {
  udid: string;
  name: string;
  version: string;
  deviceClass: string;
  link: "usb" | "network";
};

// One of the system's own symbols, as an image whose opaque part is its shape.
type SystemSymbol = {
  url: string;
  ratio: number;
};

// How the app's own look at the network went, where it looks: see `nearby.rs`.
type NetworkSearch = {
  answered: number;
  recognised: number;
};

type ExportedPairing = {
  path: string;
  fileName: string;
};

type Phase =
  | { kind: "idle" }
  | { kind: "working"; udid: string; deviceName: string }
  | { kind: "done"; udid: string; file: ExportedPairing }
  | { kind: "failed"; udid: string; error: AppError };

const POLL_INTERVAL_MS = 1500;
// Exporting again for a device paired earlier takes a moment and needs nothing from the user,
// so the "unlock and tap Trust" hint only shows once the wait gets longer than that.
const HINT_DELAY_MS = 1200;
const COPIED_NOTICE_MS = 1500;

const SOURCE_URL = "https://github.com/Alex23Grig/flekpair";

// Where a device that has trusted the computer before can be used without a cable.
const WIRELESS = platform === "mac" || platform === "windows";

const osName = (deviceClass: string) => {
  if (deviceClass === "iPad") return "iPadOS";
  if (deviceClass === "AppleTV") return "tvOS";
  if (deviceClass === "Watch") return "watchOS";
  return "iOS";
};

const DeviceGlyph = ({ tablet }: { tablet: boolean }) => (
  <svg
    className="device-glyph"
    viewBox="0 0 32 32"
    fill="none"
    stroke="currentColor"
    strokeWidth="2"
    strokeLinecap="round"
    aria-hidden="true"
  >
    {tablet ? (
      <>
        <rect x="5" y="3" width="22" height="26" rx="3.5" />
        <path d="M14 25h4" />
      </>
    ) : (
      <>
        <rect x="9" y="3" width="14" height="26" rx="3.5" />
        <path d="M14.5 25h3" />
      </>
    )}
  </svg>
);

const CopyGlyph = ({ done }: { done: boolean }) => (
  <svg
    viewBox="0 0 16 16"
    fill="none"
    stroke="currentColor"
    strokeWidth="1.4"
    strokeLinecap="round"
    strokeLinejoin="round"
    aria-hidden="true"
  >
    {done ? (
      <path d="M3 8.6l3.2 3.2L13 4.6" />
    ) : (
      <>
        <rect x="5.5" y="5.5" width="9" height="9" rx="2" />
        <path d="M3.5 10.5a2 2 0 0 1-2-2v-5a2 2 0 0 1 2-2h5a2 2 0 0 1 2 2" />
      </>
    )}
  </svg>
);

const LinkGlyph = ({
  link,
  cable,
}: {
  link: DeviceInfo["link"];
  cable: SystemSymbol | null;
}) =>
  link === "usb" && cable ? (
    <span
      className="symbol"
      style={{
        aspectRatio: cable.ratio,
        maskImage: `url(${cable.url})`,
        WebkitMaskImage: `url(${cable.url})`,
      }}
      aria-hidden="true"
    />
  ) : (
    <svg
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.6"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      {link === "network" ? (
        <>
          <path d="M1.8 6.3a9.2 9.2 0 0 1 12.4 0" />
          <path d="M4.3 9a5.5 5.5 0 0 1 7.4 0" />
          <path d="M8 12h.01" />
        </>
      ) : (
        <>
          <path d="M6 1.8v2.7M10 1.8v2.7" />
          <path d="M4.2 4.5h7.6v2.7a3.8 3.8 0 0 1-7.6 0z" />
          <path d="M8 11v3.2" />
        </>
      )}
    </svg>
  );

function App() {
  const { t } = useTranslation();

  // null until usbmuxd has answered once, so launch doesn't flash "No devices found."
  const [devices, setDevices] = useState<DeviceInfo[] | null>(null);
  const [devicesError, setDevicesError] = useState<AppError | null>(null);
  const [search, setSearch] = useState<NetworkSearch | null>(null);
  const [selectedUdid, setSelectedUdid] = useState<string | null>(null);
  const [phase, setPhase] = useState<Phase>({ kind: "idle" });
  const [showHint, setShowHint] = useState(false);
  const [copied, setCopied] = useState(false);
  const [copiedUdid, setCopiedUdid] = useState<string | null>(null);
  const [version, setVersion] = useState("");
  // Apple's symbol for a cable. Only macOS has it; elsewhere the drawn plug stays.
  const [cableSymbol, setCableSymbol] = useState<SystemSymbol | null>(null);

  useEffect(() => {
    getVersion()
      .then(setVersion)
      .catch((e) => console.error("Failed to get app version", e));
  }, []);

  useEffect(() => {
    invoke<number[] | null>("system_symbol", { name: "cable.connector" })
      .then((png) => {
        if (!png) return;
        const bytes = new Uint8Array(png);
        // A PNG states its width and height right after its signature.
        const header = new DataView(bytes.buffer);
        setCableSymbol({
          url: URL.createObjectURL(new Blob([bytes], { type: "image/png" })),
          ratio: header.getUint32(16) / header.getUint32(20),
        });
      })
      .catch((e) => console.error("Failed to get the cable symbol", e));
  }, []);

  useEffect(() => {
    let stopped = false;
    let timer: ReturnType<typeof setTimeout> | undefined;

    const poll = async () => {
      try {
        const list = await invoke<DeviceInfo[]>("list_devices");
        const searched = await invoke<NetworkSearch | null>("network_search");
        if (stopped) return;
        setDevices(list);
        setDevicesError(null);
        setSearch(searched);
      } catch (e) {
        if (stopped) return;
        setDevices([]);
        setDevicesError(toAppError(e));
      }
      timer = setTimeout(poll, POLL_INTERVAL_MS);
    };
    poll();

    return () => {
      stopped = true;
      clearTimeout(timer);
    };
  }, []);

  const working = phase.kind === "working";

  useEffect(() => {
    if (!working) {
      setShowHint(false);
      return;
    }
    const timer = setTimeout(() => setShowHint(true), HINT_DELAY_MS);
    return () => clearTimeout(timer);
  }, [working]);

  const selected =
    devices?.find((device) => device.udid === selectedUdid) ??
    devices?.[0] ??
    null;

  const deviceName = useCallback(
    (device: DeviceInfo) => device.name || t("device.title"),
    [t],
  );

  const exportPairing = useCallback(async () => {
    if (!selected || working) return;
    const udid = selected.udid;

    setCopied(false);
    setPhase({ kind: "working", udid, deviceName: deviceName(selected) });
    try {
      const file = await invoke<ExportedPairing>("export_pairing_file", {
        udid,
      });
      setPhase({ kind: "done", udid, file });
    } catch (e) {
      const error = toAppError(e);
      setPhase(
        error.type === "canceled"
          ? { kind: "idle" }
          : { kind: "failed", udid, error },
      );
    }
  }, [selected, working, deviceName]);

  useEffect(() => {
    if (!copiedUdid) return;
    const timer = setTimeout(() => setCopiedUdid(null), COPIED_NOTICE_MS);
    return () => clearTimeout(timer);
  }, [copiedUdid]);

  const copyUdid = useCallback((udid: string) => {
    navigator.clipboard
      .writeText(udid)
      .then(() => setCopiedUdid(udid))
      .catch((e) => console.error("Failed to copy UDID", e));
  }, []);

  const copyError = useCallback((error: AppError) => {
    navigator.clipboard
      .writeText(error.message)
      .then(() => setCopied(true))
      .catch((e) => console.error("Failed to copy error", e));
  }, []);

  // A result stays up after the device is unplugged, but not once another one is picked.
  const result =
    (phase.kind === "done" || phase.kind === "failed") &&
    (!selected || selected.udid === phase.udid)
      ? phase
      : null;

  return (
    <main className="app">
      <header className="brand">
        <img src={logo} alt="" className="brand-logo" />
        <h1>FlekPair</h1>
      </header>

      <section className="device" aria-live="polite">
        {devicesError?.type === "usbmuxd" && platform === "windows" ? (
          <AppleDriver error={devicesError} />
        ) : devicesError ? (
          <>
            <p className="device-empty-title">
              {t("device.unable_load_devices_prefix")}
            </p>
            <Suggestions
              items={getErrorSuggestions(t, devicesError.type)}
            />
            <pre className="detail">{devicesError.message}</pre>
          </>
        ) : devices === null ? null : !selected ? (
          <>
            <p className="device-empty-title">
              {t("device.no_devices_found_period")}
            </p>
            {WIRELESS && (
              <p className="device-wifi-hint">
                {t("device.wifi_hint", {
                  computer: platform === "mac" ? "Mac" : "PC",
                })}
              </p>
            )}
            <Suggestions items={getErrorSuggestions(t, "no_device")} />
            {search && search.recognised === 0 && (
              <p className="device-search">
                {search.answered === 0
                  ? t("device.wifi_search_none")
                  : t("device.wifi_search_unrecognised")}
              </p>
            )}
          </>
        ) : (
          <div className="device-row">
            <DeviceGlyph tablet={selected.deviceClass === "iPad"} />
            <div className="device-text">
              {devices.length > 1 ? (
                <select
                  className="device-name"
                  aria-label={t("app.select_device")}
                  value={selected.udid}
                  disabled={working}
                  onChange={(event) => setSelectedUdid(event.target.value)}
                >
                  {devices.map((device) => (
                    <option key={device.udid} value={device.udid}>
                      {deviceName(device)}
                    </option>
                  ))}
                </select>
              ) : (
                <span className="device-name">{deviceName(selected)}</span>
              )}
              <span className="device-meta">
                <span className="device-link">
                  <LinkGlyph link={selected.link} cable={cableSymbol} />
                  {selected.link === "network" ? "Wi-Fi" : "USB"}
                </span>
                {selected.version &&
                  `${osName(selected.deviceClass)} ${selected.version}`}
              </span>
              {copiedUdid === selected.udid ? (
                <span className="device-udid copied">
                  <CopyGlyph done />
                  {t("common.copied_success")}
                </span>
              ) : (
                <button
                  className="device-udid"
                  title={t("common.copy_to_clipboard")}
                  onClick={() => copyUdid(selected.udid)}
                >
                  <span className="device-udid-label">UDID</span>
                  <span className="device-udid-value">{selected.udid}</span>
                  <CopyGlyph done={false} />
                </button>
              )}
            </div>
          </div>
        )}
      </section>

      <button
        className="primary"
        disabled={!selected || working}
        onClick={exportPairing}
      >
        {working && <span className="spinner" aria-hidden="true" />}
        {t("pairing.export_pairing_file")}
      </button>

      <section className="status" aria-live="polite">
        {phase.kind === "working" && (
          <div className="progress">
            <p className="progress-title">
              {t("device.pairing_in_progress_header", {
                device: phase.deviceName,
              })}
            </p>
            {showHint && (
              <p className="progress-hint">
                {t("device.pairing_in_progress_hint")}
              </p>
            )}
            <button
              className="text-button"
              onClick={() => invoke("cancel_pairing").catch(() => {})}
            >
              {t("device.pairing_cancel")}
            </button>
          </div>
        )}

        {result?.kind === "done" && (
          <div className="result">
            <p className="result-title success">
              <svg viewBox="0 0 20 20" aria-hidden="true">
                <circle cx="10" cy="10" r="10" fill="currentColor" />
                <path
                  d="M5.8 10.4l2.8 2.8 5.6-6"
                  fill="none"
                  stroke="var(--surface)"
                  strokeWidth="2"
                  strokeLinecap="round"
                  strokeLinejoin="round"
                />
              </svg>
              {t("pairing.pairing_file_exported_success")}
            </p>
            <button
              className="file"
              title={result.file.path}
              onClick={() => invoke("reveal_pairing_file").catch(() => {})}
            >
              <span className="file-name">{result.file.fileName}</span>
              <span className="file-folder">
                {result.file.path.slice(0, -result.file.fileName.length - 1)}
              </span>
            </button>
          </div>
        )}

        {result?.kind === "failed" && (
          <div className="result">
            <div className="result-header">
              <p className="result-title failure">
                {t("pairing.failed_export_pairing_file")}
              </p>
              <button
                className="text-button"
                onClick={() => copyError(result.error)}
              >
                {copied
                  ? t("common.copied_success")
                  : t("common.copy_to_clipboard")}
              </button>
            </div>
            <pre className="detail">{result.error.message}</pre>
            {getErrorSuggestions(t, result.error.type).length > 0 && (
              <>
                <h2>{t("error.suggestions_heading")}</h2>
                <Suggestions
                  items={getErrorSuggestions(t, result.error.type)}
                />
              </>
            )}
          </div>
        )}
      </section>

      <footer>
        <span>
          {t("version")} {version}
        </span>
        <ExternalLink url={SOURCE_URL}>{t("app.github")}</ExternalLink>
      </footer>
    </main>
  );
}

export default App;
