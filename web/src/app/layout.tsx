import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { History, House, LogOut } from "lucide-react";
import { Link, NavLink, Outlet, useNavigate } from "react-router";

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
        <p className="app-context">{t("app.context")}</p>
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
        <p>{t("app.name")} · {t("app.welcome")}</p>
        <p>{t("app.footer")}</p>
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
    <div className="authenticated-shell">
      <h1 className="visually-hidden">{t("app.name")}</h1>
      <aside className="account-rail" aria-label={t("nav.workspace")}>
        <nav className="account-nav">
          <NavLink to="/" end><House size={18} aria-hidden="true" />{t("nav.home")}</NavLink>
          <NavLink to="/account/sessions"><History size={18} aria-hidden="true" />{t("nav.sessions")}</NavLink>
        </nav>
        <div className="account-rail-bottom">
          <Button type="button" variant="ghost" disabled={mutation.isPending} onClick={() => { mutation.mutate(); }}>
            <LogOut size={17} aria-hidden="true" />{mutation.isPending ? t("nav.loggingOut") : t("nav.logout")}
          </Button>
        </div>
      </aside>
      <div className="authenticated-content">
        {mutation.error === null ? null : <p role="alert">{requestErrorMessage(mutation.error, t)}</p>}
        <Outlet />
      </div>
    </div>
  );
}
