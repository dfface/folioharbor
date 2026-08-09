import { Grid2X2, List, Search } from "lucide-react";
import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";

import { Button } from "../../components/ui/button";
import { requestErrorMessage } from "../auth/form";
import { useCurrentLibrary } from "../libraries/LibraryLayout";
import { BookCard } from "./BookCard";
import { useBooks } from "./queries";

export function BooksPage() {
  const { t } = useTranslation();
  const library = useCurrentLibrary();
  const books = useBooks(library.library_id);
  const [query, setQuery] = useState("");
  const [view, setView] = useState<"grid" | "list">("grid");

  const items = useMemo(() => books.data?.pages.flatMap((page) => page.items) ?? [], [books.data]);
  const filteredItems = useMemo(() => {
    const normalized = query.trim().toLocaleLowerCase();
    if (normalized === "") return items;
    return items.filter((book) => `${book.primary_title} ${book.authors.join(" ")}`.toLocaleLowerCase().includes(normalized));
  }, [items, query]);
  return (
    <section className="page-section" aria-labelledby="books-title">
      <div className="catalogue-header" data-od-id="catalogue-header">
        <div>
          <p className="page-kicker">{t("catalog.collection")}</p>
          <h3 id="books-title">{t("catalog.allBooks")}</h3>
          <p className="catalogue-description">{t("catalog.description")}</p>
        </div>
        {books.isSuccess ? <p className="catalogue-count">{t("catalog.bookCount", { count: items.length })}</p> : null}
      </div>
      <div className="catalogue-tools" data-od-id="catalogue-tools">
        <label className="catalogue-search">
          <span className="visually-hidden">{t("catalog.search")}</span>
          <Search size={18} aria-hidden="true" />
          <input value={query} onChange={(event) => { setQuery(event.currentTarget.value); }} placeholder={t("catalog.searchPlaceholder")} type="search" />
        </label>
        <div className="view-toggle" aria-label={t("catalog.view")}>
          <Button type="button" variant={view === "grid" ? "secondary" : "ghost"} size="icon" aria-label={t("catalog.gridView")} aria-pressed={view === "grid"} onClick={() => { setView("grid"); }}><Grid2X2 size={17} /></Button>
          <Button type="button" variant={view === "list" ? "secondary" : "ghost"} size="icon" aria-label={t("catalog.listView")} aria-pressed={view === "list"} onClick={() => { setView("list"); }}><List size={18} /></Button>
        </div>
      </div>
      {books.isPending ? <p role="status">{t("catalog.loading")}</p> : null}
      {books.isError ? <p role="alert">{requestErrorMessage(books.error, t)}</p> : null}
      {books.isSuccess && items.length === 0 ? <p>{t("catalog.empty")}</p> : null}
      {books.isSuccess && items.length > 0 ? (
        <ul className={`book-grid book-grid--${view}`} aria-label={t("catalog.booksList")}>
          {filteredItems.map((book) => <li key={book.item_id}><BookCard book={book} libraryId={library.library_id} compact={view === "list"} /></li>)}
        </ul>
      ) : null}
      {books.isSuccess && items.length > 0 && filteredItems.length === 0 ? <p className="catalogue-empty">{t("catalog.noResults")}</p> : null}
      {books.isSuccess && books.hasNextPage ? (
        <Button
          type="button"
          disabled={books.isFetchingNextPage}
          onClick={() => { void books.fetchNextPage(); }}
        >
          {books.isFetchingNextPage ? t("catalog.loadingMore") : t("catalog.loadMore")}
        </Button>
      ) : null}
    </section>
  );
}
