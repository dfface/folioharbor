import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Link, Outlet, useNavigate } from "react-router";

import { Button } from "../components/ui/button";
import i18n from "../i18n";
import { logout } from "../features/auth/api";
import { requestErrorMessage, useRequestController } from "../features/auth/form";
import { resetAuthIdentityQueries } from "../features/auth/session";

export function AppLayout() {
  const { t } = useTranslation();

  function changeLanguage(language: string) {
    document.documentElement.lang = language;
    void i18n.changeLanguage(language);
  }

  return (
    <div className="app-shell">
      <header className="app-header">
        <p className="wordmark"><Link to="/">{t("app.name")}</Link></p>
        <div className="locale-control">
          <label htmlFor="locale">{t("app.language")}</label>
          <select id="locale" value={i18n.resolvedLanguage ?? "en"} onChange={(event) => { changeLanguage(event.currentTarget.value); }}>
            <option value="en">{t("app.english")}</option>
            <option value="zh-CN">{t("app.chinese")}</option>
          </select>
        </div>
      </header>
      <main className="app-main"><Outlet /></main>
      <footer className="app-footer">
        <p className="wordmark">{t("app.name")}</p>
        <p>{t("app.welcome")}</p>
      </footer>
    </div>
  );
}

export function AuthenticatedLayout() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const requestSignal = useRequestController();
  const mutation = useMutation({
    mutationFn: () => logout(requestSignal()),
    onSuccess: () => {
      void resetAuthIdentityQueries(queryClient).then(() => {
        void navigate("/login", { replace: true });
      });
    },
  });

  return (
    <>
      <h1 className="visually-hidden">{t("app.name")}</h1>
      <nav className="account-nav" aria-label="Account">
        <Link to="/">{t("nav.home")}</Link>{" "}
        <Link to="/account/sessions">{t("nav.sessions")}</Link>{" "}
        <Button type="button" variant="ghost" disabled={mutation.isPending} onClick={() => { mutation.mutate(); }}>{t("nav.logout")}</Button>
      </nav>
      {mutation.error === null ? null : <p role="alert">{requestErrorMessage(mutation.error, t)}</p>}
      <Outlet />
    </>
  );
}
