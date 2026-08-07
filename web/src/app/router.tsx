import { useTranslation } from "react-i18next";
import { Navigate, Outlet, Route, Routes } from "react-router-dom";

import { ForgotPasswordPage } from "../features/auth/ForgotPasswordPage";
import { requestErrorMessage } from "../features/auth/form";
import { LoginPage } from "../features/auth/LoginPage";
import { RegisterPage } from "../features/auth/RegisterPage";
import { ResetPasswordPage } from "../features/auth/ResetPasswordPage";
import { SessionsPage } from "../features/auth/SessionsPage";
import { useSession } from "../features/auth/session";
import { VerifyEmailPage } from "../features/auth/VerifyEmailPage";
import { AppLayout, AuthenticatedLayout } from "./layout";

function SessionLoading() {
  const { t } = useTranslation();
  return <p role="status" aria-live="polite">{t("app.loading")}</p>;
}

function SessionError({ error }: { error: unknown }) {
  const { t } = useTranslation();
  return <p role="alert">{requestErrorMessage(error, t)}</p>;
}

function RequireAnonymous() {
  const session = useSession();
  if (session.status === "loading") {
    return <SessionLoading />;
  }
  if (session.status === "error") {
    return <SessionError error={session.error} />;
  }
  return session.status === "authenticated" ? <Navigate to="/" replace /> : <Outlet />;
}

function RequireAuthentication() {
  const session = useSession();
  if (session.status === "loading") {
    return <SessionLoading />;
  }
  if (session.status === "error") {
    return <SessionError error={session.error} />;
  }
  return session.status === "anonymous" ? <Navigate to="/login" replace /> : <Outlet />;
}

function HomePage() {
  const { t } = useTranslation();
  return <h2>{t("app.welcome")}</h2>;
}

export function AppRouter() {
  return (
    <Routes>
      <Route element={<AppLayout />}>
        <Route element={<RequireAnonymous />}>
          <Route path="/login" element={<LoginPage />} />
          <Route path="/register" element={<RegisterPage />} />
          <Route path="/verify-email" element={<VerifyEmailPage />} />
          <Route path="/forgot-password" element={<ForgotPasswordPage />} />
          <Route path="/reset-password" element={<ResetPasswordPage />} />
        </Route>
        <Route element={<RequireAuthentication />}>
          <Route element={<AuthenticatedLayout />}>
            <Route index element={<HomePage />} />
            <Route path="/account/sessions" element={<SessionsPage />} />
          </Route>
        </Route>
        <Route path="*" element={<Navigate to="/" replace />} />
      </Route>
    </Routes>
  );
}
