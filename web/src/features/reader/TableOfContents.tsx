import { useEffect, useRef, type RefObject } from "react";
import { useTranslation } from "react-i18next";

import type { PublicationLink } from "./api";

interface TableOfContentsProps {
  links: PublicationLink[];
  onClose: () => void;
  onNavigate: (link: PublicationLink) => void;
  returnFocusRef: RefObject<HTMLButtonElement | null>;
}

export function TableOfContents({ links, onClose, onNavigate, returnFocusRef }: TableOfContentsProps) {
  const { t } = useTranslation();
  const closeRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    closeRef.current?.focus();
  }, []);

  function close() {
    onClose();
    returnFocusRef.current?.focus();
  }

  return (
    <div
      aria-label={t("reader.toc")}
      aria-modal="true"
      role="dialog"
      onKeyDown={(event) => {
        if (event.key === "Escape") {
          event.preventDefault();
          close();
        }
      }}
    >
      <button ref={closeRef} type="button" onClick={close}>{t("reader.closeToc")}</button>
      <ol>
        {links.map((link) => (
          <li key={`${link.href}:${link.title ?? ""}`}>
            <button type="button" onClick={() => { onNavigate(link); }}>{link.title ?? t("reader.untitledSection")}</button>
          </li>
        ))}
      </ol>
    </div>
  );
}
