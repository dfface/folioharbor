import { useTranslation } from "react-i18next";
import { Link } from "react-router";

import type { BookSummary } from "./api";

export function BookCard({ book, libraryId }: { book: BookSummary; libraryId: string }) {
  const { t } = useTranslation();
  return (
    <article>
      <h4><Link to={`/libraries/${encodeURIComponent(libraryId)}/items/${encodeURIComponent(book.item_id)}`}>{book.primary_title}</Link></h4>
      <dl>
        <div><dt>{t("catalog.work")}</dt><dd>{book.authors.length > 0 ? book.authors.join(", ") : t("catalog.unknownAuthor")}</dd></div>
        <div><dt>{t("catalog.edition")}</dt><dd>{t("catalog.epub")}</dd></div>
        <div><dt>{t("catalog.copy")}</dt><dd>{t("catalog.available")}</dd></div>
      </dl>
      {!book.can_download && book.can_read ? <p>{t("catalog.onlineOnly")}</p> : null}
    </article>
  );
}
