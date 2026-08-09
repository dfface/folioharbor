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

  const items = books.data?.pages.flatMap((page) => page.items) ?? [];
  return (
    <section className="page-section" aria-labelledby="books-title">
      <div className="catalogue-header">
        <h3 id="books-title">{t("catalog.allBooks")}</h3>
        {books.isSuccess ? <p className="catalogue-count">{items.length}</p> : null}
      </div>
      {books.isPending ? <p role="status">{t("catalog.loading")}</p> : null}
      {books.isError ? <p role="alert">{requestErrorMessage(books.error, t)}</p> : null}
      {books.isSuccess && items.length === 0 ? <p>{t("catalog.empty")}</p> : null}
      {books.isSuccess && items.length > 0 ? (
        <ul className="book-grid" aria-label={t("catalog.booksList")}>
          {items.map((book) => <li key={book.item_id}><BookCard book={book} libraryId={library.library_id} /></li>)}
        </ul>
      ) : null}
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
