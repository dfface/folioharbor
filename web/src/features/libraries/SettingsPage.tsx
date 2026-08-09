import { useEffect, useState, type SyntheticEvent } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Navigate } from "react-router";

import { requestErrorMessage, useRequestController } from "../auth/form";
import { updateLibrarySettings } from "./api";
import { useCurrentLibrary } from "./LibraryLayout";
import { librariesQueryKey, libraryQueryKey } from "./queries";

export function SettingsPage() {
  const { t } = useTranslation();
  const library = useCurrentLibrary();
  const queryClient = useQueryClient();
  const requestSignal = useRequestController();
  const [readerDownload, setReaderDownload] = useState(library.reader_download_enabled);
  const [saved, setSaved] = useState(false);
  useEffect(() => { setReaderDownload(library.reader_download_enabled); }, [library.reader_download_enabled]);

  const mutation = useMutation({
    mutationFn: () => updateLibrarySettings(library.library_id, {
      name: library.name,
      reader_download_enabled: readerDownload,
    }, requestSignal()),
    onSuccess: () => {
      setSaved(true);
      void Promise.all([
        queryClient.invalidateQueries({ queryKey: librariesQueryKey }),
        queryClient.invalidateQueries({ queryKey: libraryQueryKey(library.library_id) }),
      ]);
    },
  });

  if (!library.capabilities.can_manage_settings) {
    return <Navigate to={`/libraries/${encodeURIComponent(library.library_id)}/books`} replace />;
  }

  function submit(event: SyntheticEvent<HTMLFormElement>) {
    event.preventDefault();
    setSaved(false);
    mutation.mutate();
  }

  return (
    <section className="page-section" aria-labelledby="settings-title">
      <h3 id="settings-title">{t("settings.title")}</h3>
      {mutation.error === null ? null : <p role="alert">{requestErrorMessage(mutation.error, t)}</p>}
      {saved ? <p role="status">{t("settings.saved")}</p> : null}
      <form onSubmit={submit}>
        <label>
          <input
            type="checkbox"
            checked={readerDownload}
            onChange={(event) => { setReaderDownload(event.currentTarget.checked); }}
          />
          {t("settings.readerDownload")}
        </label>
        <p>{t("settings.readSeparate")}</p>
        <button type="submit" disabled={mutation.isPending}>{t("settings.save")}</button>
      </form>
    </section>
  );
}
