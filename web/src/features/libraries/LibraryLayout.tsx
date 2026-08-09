import { createContext, useContext } from "react";
import { useTranslation } from "react-i18next";
import { BookOpen, Settings2, Upload, Users } from "lucide-react";
import { NavLink, Navigate, Outlet, useNavigate, useParams } from "react-router";

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
          <div className="library-title-row">
            <div>
              <p className="library-eyebrow">{t("libraries.workspace")}</p>
              <h2 id="current-library-title">{library.name}</h2>
            </div>
            <p className="library-role">{t("libraries.role", { role: t(`roles.${library.role}`) })}</p>
          </div>
        </div>
        <nav className="library-nav" aria-label={t("libraries.navigation")}>
          <NavLink to={`${base}/books`}><BookOpen size={16} aria-hidden="true" />{t("libraries.books")}</NavLink>
          {library.capabilities.can_upload ? <NavLink to={`${base}/uploads`}><Upload size={16} aria-hidden="true" />{t("libraries.uploads")}</NavLink> : null}
          {library.capabilities.can_invite_members || library.capabilities.can_manage_members
            ? <NavLink to={`${base}/members`}><Users size={16} aria-hidden="true" />{t("libraries.members")}</NavLink>
            : null}{" "}
          {library.capabilities.can_manage_settings ? <NavLink to={`${base}/settings`}><Settings2 size={16} aria-hidden="true" />{t("libraries.settings")}</NavLink> : null}
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
