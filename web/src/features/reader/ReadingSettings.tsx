import { useTranslation } from "react-i18next";

export type ReadingFlow = "continuous" | "paginated";

export interface ReaderSettings {
  flow: ReadingFlow;
  fontScale: number;
}

interface ReadingSettingsProps extends ReaderSettings {
  onChange: (settings: ReaderSettings) => void;
}

export function ReadingSettings({ flow, fontScale, onChange }: ReadingSettingsProps) {
  const { t } = useTranslation();
  return (
    <fieldset>
      <legend>{t("reader.settings")}</legend>
      <label htmlFor="reader-font-size">{t("reader.fontSize")}</label>{" "}
      <input
        id="reader-font-size"
        type="number"
        min="75"
        max="200"
        step="25"
        value={fontScale}
        onChange={(event) => {
          const value = event.currentTarget.valueAsNumber;
          if (Number.isFinite(value)) {
            onChange({ flow, fontScale: Math.min(200, Math.max(75, value)) });
          }
        }}
      />{" "}
      <span aria-hidden="true">%</span>{" "}
      <label htmlFor="reader-flow">{t("reader.flow")}</label>{" "}
      <select
        id="reader-flow"
        value={flow}
        onChange={(event) => { onChange({ flow: event.currentTarget.value as ReadingFlow, fontScale }); }}
      >
        <option value="paginated">{t("reader.paginated")}</option>
        <option value="continuous">{t("reader.continuous")}</option>
      </select>
    </fieldset>
  );
}
