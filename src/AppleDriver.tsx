import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useTranslation } from "react-i18next";
import { AppError, getErrorSuggestions, toAppError } from "./errors";
import { LinkedText, Suggestions } from "./Suggestions";

type DriverState = "unsupported" | "missing" | "stopped" | "store_app";

type Stage =
  | "idle"
  | "downloading"
  | "unpacking"
  | "verifying"
  | "installing"
  | "starting";

type Progress = {
  stage: Stage;
  received: number;
  total: number;
};

const PROGRESS_INTERVAL_MS = 400;

const megabytes = (bytes: number) => Math.round(bytes / 1_000_000);

/**
 * Shown on Windows while nothing answers as usbmuxd, which there means Apple Mobile Device
 * Support isn't installed or isn't running. Offers to set it up.
 */
export const AppleDriver = ({ error }: { error: AppError }) => {
  const { t } = useTranslation();

  const [state, setState] = useState<DriverState | null>(null);
  const [progress, setProgress] = useState<Progress | null>(null);
  const [failure, setFailure] = useState<AppError | null>(null);

  const refresh = useCallback(() => {
    invoke<DriverState>("apple_driver_state")
      .then(setState)
      .catch(() => setState("unsupported"));
  }, []);

  useEffect(refresh, [refresh]);

  const working = progress !== null;

  useEffect(() => {
    if (!working) return;
    const timer = setInterval(() => {
      invoke<Progress>("apple_driver_progress")
        // A reply that arrives after the setup has ended must not bring the progress back.
        .then((next) => setProgress((current) => current && next))
        .catch(() => {});
    }, PROGRESS_INTERVAL_MS);
    return () => clearInterval(timer);
  }, [working]);

  const setUp = useCallback(
    async (firstStage: Stage) => {
      setFailure(null);
      setProgress({ stage: firstStage, received: 0, total: 0 });
      try {
        await invoke("install_apple_driver");
      } catch (e) {
        const error = toAppError(e);
        if (error.type !== "canceled") setFailure(error);
      } finally {
        setProgress(null);
        refresh();
      }
    },
    [refresh],
  );

  if (progress) {
    const stage = progress.stage === "idle" ? "downloading" : progress.stage;
    const downloading = stage === "downloading";
    const percent =
      progress.total > 0
        ? Math.min(100, Math.round((progress.received / progress.total) * 100))
        : 0;

    return (
      <>
        <p className="driver-stage">
          <span className="spinner muted" aria-hidden="true" />
          {t(`driver.${stage}`)}
        </p>
        {downloading && (
          <>
            <div
              className="meter"
              role="progressbar"
              aria-valuemin={0}
              aria-valuemax={100}
              aria-valuenow={percent}
            >
              <div style={{ width: `${percent}%` }} />
            </div>
            <div className="driver-download">
              <span>
                {progress.total > 0 &&
                  t("driver.download_size", {
                    received: megabytes(progress.received),
                    total: megabytes(progress.total),
                  })}
              </span>
              <button
                className="text-button"
                onClick={() => invoke("cancel_apple_driver").catch(() => {})}
              >
                {t("device.pairing_cancel")}
              </button>
            </div>
          </>
        )}
      </>
    );
  }

  if (state === null) return null;

  if (state === "unsupported") {
    return (
      <>
        <p className="device-empty-title">
          {t("device.unable_load_devices_prefix")}
        </p>
        <Suggestions items={getErrorSuggestions(t, error.type)} />
        <pre className="detail">{error.message}</pre>
      </>
    );
  }

  const action =
    state === "missing" ? (
      <button className="card-button" onClick={() => setUp("downloading")}>
        {t("driver.install")}
      </button>
    ) : state === "stopped" ? (
      <button className="card-button" onClick={() => setUp("starting")}>
        {t("driver.start")}
      </button>
    ) : null;

  // Installing one of Apple's own apps brings the driver too, for anyone who would rather.
  const alternative = state === "missing" && (
    <p className="driver-alternative">
      <LinkedText text={t("driver.alternative")} />
    </p>
  );

  if (failure) {
    const suggestions = getErrorSuggestions(t, failure.type);
    return (
      <>
        <p className="device-empty-title failure">{t("driver.failed")}</p>
        <pre className="detail">{failure.message}</pre>
        {suggestions.length > 0 && <Suggestions items={suggestions} />}
        {action}
        {alternative}
      </>
    );
  }

  return (
    <>
      <p className="device-empty-title">{t(`driver.${state}_title`)}</p>
      <p className="driver-body">{t(`driver.${state}_body`)}</p>
      {action}
      {alternative}
    </>
  );
};
