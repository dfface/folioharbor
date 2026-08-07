import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useParams } from "react-router";

import { ApiProblem, isAbortError } from "../../api/problem";
import { useSession } from "../auth/session";
import {
  authorizedResourceHref,
  getPublicationManifest,
  getPublicationResource,
  readerProgressApi,
  ReaderResourceError,
  UnsafeReaderResourceError,
  type PublicationLink,
  type PublicationManifest,
} from "./api";
import { createLocator, type Locator } from "./locator";
import { containModalFocus } from "./modal";
import {
  browserProgressClock,
  getOrCreateDeviceId,
  ProgressSync,
  type ProgressState,
} from "./ProgressSync";
import { ReaderFrame } from "./ReaderFrame";
import { ReadingSettings, type ReaderSettings } from "./ReadingSettings";
import { TableOfContents } from "./TableOfContents";

const settingsKey = "folioharbor.reader.settings.v1";
const defaultSettings: ReaderSettings = { flow: "paginated", fontScale: 100 };

type ReaderError = "inaccessible" | "request" | "unsafe";

function readSettings(): ReaderSettings {
  try {
    const value: unknown = JSON.parse(localStorage.getItem(settingsKey) ?? "null");
    if (typeof value === "object" && value !== null) {
      const candidate = value as Partial<ReaderSettings>;
      if (
        (candidate.flow === "paginated" || candidate.flow === "continuous") &&
        typeof candidate.fontScale === "number" &&
        candidate.fontScale >= 75 &&
        candidate.fontScale <= 200
      ) {
        return { flow: candidate.flow, fontScale: candidate.fontScale };
      }
    }
  } catch {
    // Invalid or unavailable local preferences fall back to accessible defaults.
  }
  return defaultSettings;
}

function classifyError(error: unknown): ReaderError | null {
  if (isAbortError(error)) {
    return null;
  }
  if (error instanceof UnsafeReaderResourceError) {
    return "unsafe";
  }
  if (
    (error instanceof ApiProblem && [401, 403, 404].includes(error.problem.status)) ||
    (error instanceof ReaderResourceError && [401, 403, 404].includes(error.status))
  ) {
    return "inaccessible";
  }
  return "request";
}

function linkBase(href: string): string {
  return href.split("#", 1)[0] ?? href;
}

export function ReaderPage() {
  const { t } = useTranslation();
  const { itemId = "" } = useParams();
  const session = useSession();
  const accountId = session.status === "authenticated" ? session.session.user_id : null;
  const [manifest, setManifest] = useState<PublicationManifest | null>(null);
  const [currentLink, setCurrentLink] = useState<PublicationLink | null>(null);
  const [resource, setResource] = useState<Blob | null>(null);
  const [error, setError] = useState<ReaderError | null>(null);
  const [tocOpen, setTocOpen] = useState(false);
  const [settings, setSettings] = useState(readSettings);
  const [progress, setProgress] = useState<ProgressState>({ status: "idle", version: 0 });
  const tocButtonRef = useRef<HTMLButtonElement>(null);
  const restoreTocFocusRef = useRef(false);
  const frameRef = useRef<HTMLIFrameElement>(null);
  const progressRef = useRef<ProgressSync | null>(null);
  const reducedMotion = useMemo(
    () => typeof matchMedia === "function" && matchMedia("(prefers-reduced-motion: reduce)").matches,
    [],
  );

  useEffect(() => {
    const controller = new AbortController();
    setManifest(null);
    setCurrentLink(null);
    setError(null);
    void getPublicationManifest(itemId, controller.signal)
      .then((nextManifest) => {
        setManifest(nextManifest);
        setCurrentLink(nextManifest.readingOrder[0] ?? null);
      })
      .catch((requestError: unknown) => {
        setError(classifyError(requestError));
      });
    return () => { controller.abort(); };
  }, [itemId]);

  useEffect(() => {
    if (currentLink === null) {
      return;
    }
    const controller = new AbortController();
    setResource(null);
    setError(null);
    void getPublicationResource(itemId, currentLink, controller.signal)
      .then(setResource)
      .catch((requestError: unknown) => {
        setError(classifyError(requestError));
      });
    return () => { controller.abort(); };
  }, [currentLink, itemId]);

  useEffect(() => {
    if (manifest === null || accountId === null) {
      return;
    }
    const sync = new ProgressSync({
      accountId,
      api: readerProgressApi,
      clock: browserProgressClock,
      deviceId: getOrCreateDeviceId(localStorage, accountId),
      manifestationId: manifest.manifestationId,
      mutationId: () => crypto.randomUUID(),
      storage: localStorage,
    });
    progressRef.current = sync;
    const unsubscribe = sync.subscribe((nextProgress) => {
      setProgress(nextProgress);
      if (nextProgress.status === "inaccessible") {
        setError("inaccessible");
        return;
      }
      if (nextProgress.status === "conflict") {
        setTocOpen(false);
      }
      if (nextProgress.status !== "synced" || nextProgress.locator === undefined) {
        return;
      }
      try {
        const href = authorizedResourceHref(itemId, nextProgress.locator.href);
        const matching = manifest.readingOrder.find((link) => linkBase(link.href) === linkBase(href));
        if (matching === undefined) {
          setError("unsafe");
          return;
        }
        setCurrentLink((existing) => existing?.href === href ? existing : { ...matching, href });
      } catch {
        setError("unsafe");
      }
    });
    const flushBounded = () => { void sync.flush({ bounded: true }); };
    const flushWhenHidden = () => {
      if (document.visibilityState === "hidden") {
        flushBounded();
      }
    };
    const retryOnline = () => { void sync.retry(); };
    document.addEventListener("visibilitychange", flushWhenHidden);
    window.addEventListener("pagehide", flushBounded);
    window.addEventListener("online", retryOnline);
    void sync.start();
    return () => {
      document.removeEventListener("visibilitychange", flushWhenHidden);
      window.removeEventListener("pagehide", flushBounded);
      window.removeEventListener("online", retryOnline);
      flushBounded();
      unsubscribe();
      sync.dispose();
      if (progressRef.current === sync) {
        progressRef.current = null;
      }
    };
  }, [accountId, itemId, manifest]);

  const changeSettings = useCallback((nextSettings: ReaderSettings) => {
    setSettings(nextSettings);
    try {
      localStorage.setItem(settingsKey, JSON.stringify(nextSettings));
    } catch {
      // The setting still applies for this session when persistent storage is unavailable.
    }
  }, []);

  const navigate = useCallback((link: PublicationLink) => {
    setCurrentLink(link);
    if (manifest === null) {
      return;
    }
    const index = manifest.readingOrder.findIndex((entry) => linkBase(entry.href) === linkBase(link.href));
    if (index < 0) {
      setError("unsafe");
      return;
    }
    progressRef.current?.report(createLocator({
      href: authorizedResourceHref(itemId, link.href),
      mediaType: link.type,
      position: index + 1,
      totalProgression: index / manifest.readingOrder.length,
    }));
  }, [itemId, manifest]);

  const currentIndex = manifest === null || currentLink === null
    ? -1
    : manifest.readingOrder.findIndex((link) => linkBase(link.href) === linkBase(currentLink.href));
  const navigateToIndex = useCallback((index: number) => {
    const link = manifest?.readingOrder[index];
    if (link !== undefined) {
      navigate(link);
    }
  }, [manifest, navigate]);
  const closeToc = useCallback(() => {
    restoreTocFocusRef.current = true;
    setTocOpen(false);
  }, []);
  useLayoutEffect(() => {
    if (!tocOpen && restoreTocFocusRef.current) {
      restoreTocFocusRef.current = false;
      tocButtonRef.current?.focus();
    }
  }, [tocOpen]);
  const progressConflict = progress.status === "conflict";
  const modalOpen = tocOpen || progressConflict;

  if (error !== null) {
    const message = error === "inaccessible"
      ? t("reader.accessRevoked")
      : error === "unsafe"
        ? t("reader.unsafe")
        : t("reader.loadError");
    return <p role="alert">{message}</p>;
  }
  if (manifest === null) {
    return <p role="status" aria-live="polite">{t("reader.loading")}</p>;
  }

  return (
    <section aria-labelledby="reader-title" onKeyDown={(event) => {
      if (modalOpen) {
        return;
      }
      if (event.target instanceof HTMLInputElement || event.target instanceof HTMLSelectElement) {
        return;
      }
      if (event.key === "ArrowLeft" && currentIndex > 0) {
        navigateToIndex(currentIndex - 1);
      } else if (event.key === "ArrowRight" && currentIndex >= 0 && currentIndex < manifest.readingOrder.length - 1) {
        navigateToIndex(currentIndex + 1);
      }
    }}>
      <div inert={modalOpen ? true : undefined} aria-hidden={modalOpen ? true : undefined}>
        <h3 id="reader-title">{manifest.metadata.title}</h3>
        <div>
          <button ref={tocButtonRef} type="button" onClick={() => { setTocOpen(true); }}>{t("reader.toc")}</button>{" "}
          <button
            type="button"
            disabled={currentIndex <= 0}
            onClick={() => { navigateToIndex(currentIndex - 1); }}
          >
            {t("reader.previous")}
          </button>{" "}
          <button
            type="button"
            disabled={currentIndex < 0 || currentIndex >= manifest.readingOrder.length - 1}
            onClick={() => { navigateToIndex(currentIndex + 1); }}
          >
            {t("reader.next")}
          </button>
        </div>
        <ReadingSettings {...settings} onChange={changeSettings} />
        {progressConflict ? null : (
          <ProgressStatus progress={progress} onResolve={(choice) => { void progressRef.current?.resolveConflict(choice); }} />
        )}
        {resource === null ? <p role="status" aria-live="polite">{t("reader.loadingResource")}</p> : (
          <ReaderFrame
            ref={frameRef}
            blob={resource}
            flow={settings.flow}
            fontScale={settings.fontScale}
            reducedMotion={reducedMotion}
            title={t("reader.frameTitle", { title: manifest.metadata.title })}
          />
        )}
      </div>
      {tocOpen ? (
        <TableOfContents
          links={manifest.toc}
          onClose={closeToc}
          onNavigate={navigate}
        />
      ) : null}
      {progressConflict ? (
        <ProgressStatus progress={progress} onResolve={(choice) => { void progressRef.current?.resolveConflict(choice); }} />
      ) : null}
    </section>
  );
}

function progressPercentage(locator: Locator | null | undefined): number {
  if (locator === null || locator === undefined) {
    return 0;
  }
  return Math.round((locator.locations.totalProgression ?? locator.locations.progression ?? 0) * 100);
}

function ProgressStatus({
  progress,
  onResolve,
}: {
  progress: ProgressState;
  onResolve: (choice: "device" | "global") => void;
}) {
  const { t } = useTranslation();
  const firstChoiceRef = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    if (progress.status === "conflict") {
      firstChoiceRef.current?.focus();
    }
  }, [progress.status]);
  if (progress.status === "conflict") {
    return (
      <div
        role="dialog"
        aria-modal="true"
        aria-label={t("reader.progressConflict")}
        onKeyDown={containModalFocus}
      >
        <p>{t("reader.globalPosition", { percentage: progressPercentage(progress.global.locator) })}</p>
        <p>{t("reader.devicePosition", { percentage: progressPercentage(progress.device.locator) })}</p>
        <button ref={firstChoiceRef} type="button" onClick={() => { onResolve("global"); }}>{t("reader.useGlobal")}</button>{" "}
        <button type="button" onClick={() => { onResolve("device"); }}>{t("reader.useDevice")}</button>
      </div>
    );
  }
  const key = progress.status === "offline"
    ? "reader.progressOffline"
    : progress.status === "dirty" || progress.status === "saving"
      ? "reader.progressSaving"
      : progress.status === "synced"
        ? "reader.progressSynced"
        : "reader.progressIdle";
  return <p role="status" aria-live="polite">{t(key)}</p>;
}
