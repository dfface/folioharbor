import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Link, Outlet, useNavigate } from "react-router";

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
    <>
      <header>
        <h1>{t("app.name")}</h1>
        <label htmlFor="locale">{t("app.language")}</label>
        <select id="locale" value={i18n.resolvedLanguage ?? "en"} onChange={(event) => { changeLanguage(event.currentTarget.value); }}>
          <option value="en">{t("app.english")}</option>
          <option value="zh-CN">{t("app.chinese")}</option>
        </select>
      </header>
      <main>
        <Outlet />
      </main>
    </>
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
      <nav aria-label="Account">
        <Link to="/">{t("nav.home")}</Link>{" "}
        <Link to="/account/sessions">{t("nav.sessions")}</Link>{" "}
        <button type="button" disabled={mutation.isPending} onClick={() => { mutation.mutate(); }}>{t("nav.logout")}</button>
      </nav>
      {mutation.error === null ? null : <p role="alert">{requestErrorMessage(mutation.error, t)}</p>}
      <Outlet />
    </>
  );
}
