import i18n from "i18next";
import { initReactI18next } from "react-i18next";

import en from "./en.json";
import zhCN from "./zh-CN.json";

const checkedEnglishCatalog: typeof zhCN = en;
const checkedChineseCatalog: typeof en = zhCN;

void i18n.use(initReactI18next).init({
  fallbackLng: "en",
  interpolation: { escapeValue: false },
  lng: "en",
  showSupportNotice: false,
  resources: {
    en: { translation: checkedEnglishCatalog },
    "zh-CN": { translation: checkedChineseCatalog },
  },
});

export default i18n;
