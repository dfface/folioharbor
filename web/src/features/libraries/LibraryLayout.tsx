import { createContext, useContext } from "react";
import { useTranslation } from "react-i18next";
import { Link, Navigate, Outlet, useNavigate, useParams } from "react-router";

import { requestErrorMessage } from "../auth/form";
import type { Library } from "./api";
import { LibrarySwitcher } from "./LibrarySwitcher";
import { useLibraries, useLibrary } from "./queries";

const LibraryContext = createContext<Library | null>(null);

export function useCurrentLibrary(): Library {
  const library = useContext(LibraryContext);
  if (library === null) {
    throw new Error("Library route context is unavailable");
  }
  return library;
}

export function LibraryLayout() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const { libraryId = "" } = useParams();
  const libraries = useLibraries();
  const current = useLibrary(libraryId);

  if (libraries.isPending || current.isPending) {
    return <p role="status" aria-live="polite">{t("libraries.loading")}</p>;
  }
  if (libraries.isError) {
    return <p role="alert">{requestErrorMessage(libraries.error, t)}</p>;
  }
  if (current.isError) {
    return <p role="alert">{requestErrorMessage(current.error, t)}</p>;
  }

  const library = current.data;
  if (!libraries.data.some(({ library_id }) => library_id === library.library_id)) {
    return <Navigate to="/" replace />;
  }
  const base = `/libraries/${encodeURIComponent(library.library_id)}`;

  return (
    <LibraryContext.Provider value={library}>
      <section className="library-shell" aria-labelledby="current-library-title">
        <div className="library-intro">
          <LibrarySwitcher
            currentLibraryId={library.library_id}
            libraries={libraries.data}
            onChange={(nextLibrary) => { void navigate(`/libraries/${encodeURIComponent(nextLibrary)}/books`); }}
          />
          <h2 id="current-library-title">{library.name}</h2>
          <p className="library-eyebrow">{t("libraries.role", { role: t(`roles.${library.role}`) })}</p>
        </div>
        <nav className="library-nav" aria-label={t("libraries.navigation")}>
          <Link to={`${base}/books`}>{t("libraries.books")}</Link>{" "}
          {library.capabilities.can_upload ? <Link to={`${base}/uploads`}>{t("libraries.uploads")}</Link> : null}{" "}
          {library.capabilities.can_invite_members || library.capabilities.can_manage_members
            ? <Link to={`${base}/members`}>{t("libraries.members")}</Link>
            : null}{" "}
          {library.capabilities.can_manage_settings ? <Link to={`${base}/settings`}>{t("libraries.settings")}</Link> : null}
        </nav>
        <Outlet />
      </section>
    </LibraryContext.Provider>
  );
}

export function LibraryHome() {
  const { t } = useTranslation();
  const libraries = useLibraries();
  if (libraries.isPending) {
    return <p role="status">{t("libraries.loading")}</p>;
  }
  if (libraries.isError) {
    return <p role="alert">{requestErrorMessage(libraries.error, t)}</p>;
  }
  const first = libraries.data[0];
  if (first === undefined) {
    return <section><h2>{t("libraries.emptyTitle")}</h2><p>{t("libraries.empty")}</p></section>;
  }
  return <Navigate to={`/libraries/${encodeURIComponent(first.library_id)}/books`} replace />;
}
