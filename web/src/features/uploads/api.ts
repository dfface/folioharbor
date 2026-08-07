import { apiClient } from "../../api/client";
import type { components } from "../../api/generated";

export type CreateUploadRequest = components["schemas"]["CreateUploadRequest"];
export type Upload = components["schemas"]["UploadStatus"];

function signalOption(signal: AbortSignal | undefined): { signal?: AbortSignal } {
  return signal === undefined ? {} : { signal };
}

export function createUpload(libraryId: string, input: CreateUploadRequest, signal?: AbortSignal): Promise<Upload> {
  return apiClient.request(`/api/v1/libraries/${encodeURIComponent(libraryId)}/uploads`, {
    body: input,
    method: "POST",
    ...signalOption(signal),
  });
}

export function getUploadStatus(libraryId: string, uploadId: string, signal?: AbortSignal): Promise<Upload> {
  return apiClient.request(
    `/api/v1/libraries/${encodeURIComponent(libraryId)}/uploads/${encodeURIComponent(uploadId)}`,
    signalOption(signal),
  );
}

export interface UploadProgress {
  sentBytes: number;
  totalBytes: number;
}

export function transferUploadContent(
  libraryId: string,
  uploadId: string,
  file: File,
  onProgress: (progress: UploadProgress) => void,
  signal: AbortSignal,
): Promise<Upload> {
  return new Promise((resolve, reject) => {
    const request = new XMLHttpRequest();
    request.open(
      "PUT",
      `/api/v1/libraries/${encodeURIComponent(libraryId)}/uploads/${encodeURIComponent(uploadId)}/content`,
    );
    request.withCredentials = true;
    request.responseType = "json";
    request.setRequestHeader("Content-Type", file.type || "application/epub+zip");
    const csrf = csrfToken();
    if (csrf !== undefined) {
      request.setRequestHeader("X-CSRF-Token", csrf);
    }
    request.upload.addEventListener("progress", (event) => {
      onProgress({ sentBytes: event.loaded, totalBytes: file.size });
    });
    request.addEventListener("load", () => {
      if (request.status >= 200 && request.status < 300) {
        resolve(request.response as Upload);
      } else {
        reject(new Error("upload_transfer_failed"));
      }
    });
    request.addEventListener("error", () => { reject(new Error("upload_transfer_failed")); });
    request.addEventListener("abort", () => { reject(new DOMException("Upload canceled", "AbortError")); });
    signal.addEventListener("abort", () => { request.abort(); }, { once: true });
    request.send(file);
  });
}

function csrfToken(): string | undefined {
  const prefix = "folioharbor_csrf=";
  const cookie = document.cookie.split(";").map((part) => part.trim()).find((part) => part.startsWith(prefix));
  return cookie === undefined ? undefined : decodeURIComponent(cookie.slice(prefix.length));
}
