import i18n from "i18next";
import { initReactI18next } from "react-i18next";
import LanguageDetector from "i18next-browser-languagedetector";

type TranslationResource = Record<string, unknown>;

const localeModules = import.meta.glob<{ default: TranslationResource }>(
  "./locales/*.json",
  {
    eager: true,
  },
);

const resources = Object.fromEntries(
  Object.entries(localeModules).flatMap(([path, module]) => {
    const lang = path.match(/\/([\w-]+)\.json$/)?.[1];
    if (!lang) return [];

    return [[lang, { translation: module.default }]];
  }),
);

// Locale files keep iloader's names, which aren't always what the system reports.
const localeAliases: Record<string, string> = {
  "de-ch": "de_ch",
  pt: "pt_br",
  km: "kh",
};

const toLocale = (detected: string) => {
  const tag = detected.toLowerCase().replace(/_/g, "-");
  const language = tag.split("-")[0];
  return localeAliases[tag] ?? localeAliases[language] ?? language;
};

i18n
  .use(LanguageDetector)
  .use(initReactI18next)
  .init({
    fallbackLng: "en",
    interpolation: {
      escapeValue: false,
    },
    resources,
    // There is no language picker: follow the system (or ?lng= while developing).
    detection: {
      order: ["querystring", "navigator"],
      caches: [],
      convertDetectedLanguage: toLocale,
    },
  });

document.documentElement.lang = i18n.resolvedLanguage ?? "en";
document.documentElement.dir = i18n.dir();

export default i18n;
