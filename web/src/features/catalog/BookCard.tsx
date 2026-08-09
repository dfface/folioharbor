import { useTranslation } from "react-i18next";
import { ArrowUpRight, BookOpen } from "lucide-react";
import { Link } from "react-router";

import { Card, CardContent, CardHeader, CardTitle } from "../../components/ui/card";
import type { BookSummary } from "./api";

export function BookCard({ book, libraryId, compact = false }: { book: BookSummary; libraryId: string; compact?: boolean }) {
  const { t } = useTranslation();
  return (
    <Card className={`book-card ${compact ? "book-card--compact" : ""}`} size="sm" data-od-id={`book-card-${book.item_id}`}>
      <CardHeader>
        <div className="book-card-icon" aria-hidden="true"><BookOpen size={18} /></div>
        <CardTitle><h4><Link to={`/libraries/${encodeURIComponent(libraryId)}/items/${encodeURIComponent(book.item_id)}`}>{book.primary_title}</Link></h4></CardTitle>
      </CardHeader>
      <CardContent>
        <dl className="book-meta">
          <div><dt>{t("catalog.work")}</dt><dd>{book.authors.length > 0 ? book.authors.join(", ") : t("catalog.unknownAuthor")}</dd></div>
          <div><dt>{t("catalog.edition")}</dt><dd>{t("catalog.epub")}</dd></div>
        </dl>
        <div className="book-card-footer">
          <span className="book-availability">{book.can_read ? t("catalog.readyToRead") : t("catalog.available")}</span>
          <Link className="book-open-link" to={`/libraries/${encodeURIComponent(libraryId)}/items/${encodeURIComponent(book.item_id)}`} aria-label={t("catalog.openBook", { title: book.primary_title })}><ArrowUpRight size={17} /></Link>
        </div>
        {!book.can_download && book.can_read ? <p className="book-note">{t("catalog.onlineOnly")}</p> : null}
      </CardContent>
    </Card>
  );
}
