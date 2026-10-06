import type { TFunction } from "i18next";

export const errorSuggestionKeys = {
  usbmuxd: [
    "error.suggestions.usbmuxd",
    "error.suggestions.device_coms",
    "error.suggestions.trust",
  ],
  no_device: ["error.suggestions.device_coms"],
  device_coms: ["error.suggestions.device_coms", "error.suggestions.trust"],
  lockdown_pairing: ["error.suggestions.trust", "error.suggestions.pairing"],
  remote_pairing: ["error.suggestions.trust", "error.suggestions.pairing"],
  trust_denied: ["error.suggestions.trust"],
  canceled: [],
  filesystem: ["error.suggestions.filesystem"],
  driver: ["error.suggestions.usbmuxd"],
  misc: ["error.suggestions.misc"],
} as const;

export type ErrorVariant = keyof typeof errorSuggestionKeys;

export type AppError = {
  type: ErrorVariant;
  message: string;
};

export type Platform = "mac" | "windows" | "linux";

const userAgent = navigator.userAgent;
// ?platform= lets the browser preview show what another system would.
const previewed = import.meta.env.DEV
  ? new URLSearchParams(location.search).get("platform")
  : null;
export const platform: Platform =
  previewed === "mac" || previewed === "windows" || previewed === "linux"
    ? previewed
    : userAgent.includes("Mac")
      ? "mac"
      : userAgent.includes("Linux")
        ? "linux"
        : "windows";

export const isErrorVariant = (value: string): value is ErrorVariant => {
  return value in errorSuggestionKeys;
};

// Commands reject with `{ type, message }`; anything else is a failure outside our own code.
export const toAppError = (e: unknown): AppError => {
  if (typeof e === "object" && e !== null) {
    const { type, message } = e as { type?: unknown; message?: unknown };
    if (typeof message === "string") {
      return {
        type: typeof type === "string" && isErrorVariant(type) ? type : "misc",
        message,
      };
    }
  }
  return { type: "misc", message: String(e) };
};

const dedupeSuggestions = (suggestions: string[]) => {
  return [...new Set(suggestions)];
};

const getSuggestionBlock = (t: TFunction, key: string) => {
  const rawSuggestions = t(key, {
    returnObjects: true,
    defaultValue: [],
  }) as unknown;

  if (!Array.isArray(rawSuggestions)) {
    return [];
  }

  return rawSuggestions
    .filter((suggestion): suggestion is string => {
      if (typeof suggestion !== "string") {
        return false;
      }
      if (suggestion.startsWith("[platform::")) {
        const platformEnd = suggestion.indexOf("]");
        return (
          platformEnd !== -1 && suggestion.substring(11, platformEnd) === platform
        );
      }
      return true;
    })
    .map((s) => s.replace(/^\[platform::.*?\]/, "").trim());
};

export const getErrorSuggestions = (
  t: TFunction,
  type: ErrorVariant,
): string[] => {
  return dedupeSuggestions(
    errorSuggestionKeys[type].flatMap((key) => getSuggestionBlock(t, key)),
  );
};

export const parseLinkToken = (
  token: string,
): { url: string; text: string } | null => {
  const doubleColonMatch = token.match(/^\(\(link::([^)]+)\)\)$/);
  if (doubleColonMatch) {
    const url = doubleColonMatch[1].trim();
    return url ? { url, text: url } : null;
  }

  const singleColonMatch = token.match(/^\(\(link:([^)]+)\)\)$/);
  if (!singleColonMatch) {
    return null;
  }

  const payload = singleColonMatch[1].trim();
  if (!payload) {
    return null;
  }

  const lastColon = payload.lastIndexOf(":");
  if (lastColon > 0) {
    const possibleUrl = payload.slice(0, lastColon).trim();
    const possibleText = payload.slice(lastColon + 1).trim();
    if (possibleText && /^[a-z][a-z0-9+.-]*:\/\//i.test(possibleUrl)) {
      return { url: possibleUrl, text: possibleText };
    }
  }

  return { url: payload, text: payload };
};
