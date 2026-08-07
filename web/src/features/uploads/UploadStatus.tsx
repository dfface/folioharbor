import { useTranslation } from "react-i18next";
import { Link } from "react-router-dom";

import type { Upload } from "./api";

export function UploadStatus({ status }: { status: Upload }) {
  const { t } = useTranslation();
  return (
    <section aria-labelledby="upload-status-title" aria-live="polite">
      <h4 id="upload-status-title">{status.file_name}</h4>
      <p>{t(`uploads.states.${status.state}`)}</p>
      {status.state === "duplicate" && status.item_id != null ? (
        <Link to={`/libraries/${encodeURIComponent(status.library_id)}/items/${encodeURIComponent(status.item_id)}`}>
          {t("uploads.openExisting")}
        </Link>
      ) : null}
      {status.state === "ready" && status.item_id != null ? (
        <Link to={`/libraries/${encodeURIComponent(status.library_id)}/items/${encodeURIComponent(status.item_id)}`}>
          {t("uploads.openBook")}
        </Link>
      ) : null}
    </section>
  );
}
