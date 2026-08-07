import { useEffect, useRef, useState, type SyntheticEvent } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Navigate, useSearchParams } from "react-router";

import { isAbortError } from "../../api/problem";
import { requestErrorMessage, useRequestController } from "../auth/form";
import { useCurrentLibrary } from "../libraries/LibraryLayout";
import { createUpload, transferUploadContent, type UploadProgress } from "./api";
import { uploadStatusQueryKey, useUploadStatus } from "./queries";
import { UploadStatus } from "./UploadStatus";

const MAX_UPLOAD_BYTES = 1_073_741_824;
type TransferPhase = "idle" | "transferring" | "processing" | "canceled";

export function UploadPage() {
  const { t } = useTranslation();
  const library = useCurrentLibrary();
  const queryClient = useQueryClient();
  const requestSignal = useRequestController();
  const [searchParams, setSearchParams] = useSearchParams();
  const [file, setFile] = useState<File | null>(null);
  const [progress, setProgress] = useState<UploadProgress | null>(null);
  const [phase, setPhase] = useState<TransferPhase>("idle");
  const [localError, setLocalError] = useState<string | null>(null);
  const transferController = useRef<AbortController | null>(null);
  const uploadId = searchParams.get("upload");
  const status = useUploadStatus(
    library.library_id,
    uploadId,
    uploadId !== null && phase !== "transferring" && phase !== "canceled",
  );

  useEffect(() => () => { transferController.current?.abort(); }, []);

  const mutation = useMutation({
    mutationFn: async (selectedFile: File) => {
      const created = await createUpload(library.library_id, {
        file_name: selectedFile.name,
        media_type: selectedFile.type === "application/epub+zip" ? "application/epub+zip" : "application/octet-stream",
        declared_bytes: selectedFile.size,
      }, requestSignal());
      setPhase("transferring");
      setSearchParams({ upload: created.upload_id }, { replace: true });
      const controller = new AbortController();
      transferController.current = controller;
      return transferUploadContent(
        library.library_id,
        created.upload_id,
        selectedFile,
        setProgress,
        controller.signal,
      );
    },
    onError: (error) => {
      if (isAbortError(error)) {
        setPhase("canceled");
      } else {
        setPhase("idle");
      }
    },
    onSuccess: (completed) => {
      setPhase("processing");
      queryClient.setQueryData(uploadStatusQueryKey(library.library_id, completed.upload_id), completed);
      void queryClient.invalidateQueries({ queryKey: uploadStatusQueryKey(library.library_id, completed.upload_id) });
    },
  });

  if (!library.capabilities.can_upload) {
    return <Navigate to={`/libraries/${encodeURIComponent(library.library_id)}/books`} replace />;
  }

  function submit(event: SyntheticEvent<HTMLFormElement>) {
    event.preventDefault();
    setLocalError(null);
    if (file === null) {
      setLocalError(t("uploads.fileRequired"));
      return;
    }
    if (file.size > MAX_UPLOAD_BYTES) {
      setLocalError(t("uploads.tooLarge"));
      return;
    }
    setProgress({ sentBytes: 0, totalBytes: file.size });
    setPhase("transferring");
    mutation.mutate(file);
  }

  const percentage = progress === null || progress.totalBytes === 0
    ? 0
    : Math.min(100, Math.round((progress.sentBytes / progress.totalBytes) * 100));

  return (
    <section aria-labelledby="uploads-title">
      <h3 id="uploads-title">{t("uploads.title")}</h3>
      {localError === null ? null : <p role="alert">{localError}</p>}
      {mutation.error === null || isAbortError(mutation.error) ? null : <p role="alert">{requestErrorMessage(mutation.error, t)}</p>}
      <form onSubmit={submit}>
        <label htmlFor="epub-file">{t("uploads.file")}</label>
        <input
          id="epub-file"
          type="file"
          accept=".epub,application/epub+zip,application/octet-stream"
          onChange={(event) => { setFile(event.currentTarget.files?.[0] ?? null); }}
        />
        <button type="submit" disabled={phase === "transferring"}>{t("uploads.submit")}</button>
      </form>
      {phase === "transferring" && progress !== null ? (
        <div aria-live="polite">
          <progress
            aria-label={t("uploads.progress")}
            aria-valuenow={percentage}
            max={100}
            value={percentage}
          >
            {percentage}%
          </progress>
          <p>{t("uploads.bytes", { sent: progress.sentBytes, total: progress.totalBytes, percentage })}</p>
          <button type="button" onClick={() => { transferController.current?.abort(); }}>{t("uploads.cancel")}</button>
        </div>
      ) : null}
      {phase === "processing" ? <p role="status">{t("uploads.background")}</p> : null}
      {phase === "canceled" ? <p role="status">{t("uploads.canceled")}</p> : null}
      {status.isError ? <p role="alert">{requestErrorMessage(status.error, t)}</p> : null}
      {status.data === undefined ? null : <UploadStatus status={status.data} />}
    </section>
  );
}
