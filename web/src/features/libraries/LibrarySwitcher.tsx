import { useTranslation } from "react-i18next";

import type { Library } from "./api";

interface LibrarySwitcherProps {
  currentLibraryId: string;
  libraries: readonly Library[];
  onChange: (libraryId: string) => void;
}

export function LibrarySwitcher({ currentLibraryId, libraries, onChange }: LibrarySwitcherProps) {
  const { t } = useTranslation();
  return (
    <div className="library-switcher">
      <label htmlFor="library-switcher">{t("libraries.current")}</label>
      <select
        id="library-switcher"
        value={currentLibraryId}
        onChange={(event) => { onChange(event.currentTarget.value); }}
      >
        {libraries.map((library) => (
          <option key={library.library_id} value={library.library_id}>{library.name}</option>
        ))}
      </select>
    </div>
  );
}
