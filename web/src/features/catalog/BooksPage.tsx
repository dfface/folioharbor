import { useTranslation } from "react-i18next";

import { requestErrorMessage } from "../auth/form";
import { useCurrentLibrary } from "../libraries/LibraryLayout";
import { BookCard } from "./BookCard";
import { useBooks } from "./queries";

export function BooksPage() {
  const { t } = useTranslation();
  const library = useCurrentLibrary();
  const books = useBooks(library.library_id);

  if (books.isPending) {
    return <section><h3>{t("catalog.allBooks")}</h3><p role="status">{t("catalog.loading")}</p></section>;
  }
  if (books.isError) {
    return <section><h3>{t("catalog.allBooks")}</h3><p role="alert">{requestErrorMessage(books.error, t)}</p></section>;
  }
  const items = books.data.pages.flatMap((page) => page.items);
  return (
    <section aria-labelledby="books-title">
      <h3 id="books-title">{t("catalog.allBooks")}</h3>
      {items.length === 0 ? <p>{t("catalog.empty")}</p> : (
        <ul aria-label={t("catalog.booksList")}>
          {items.map((book) => <li key={book.item_id}><BookCard book={book} libraryId={library.library_id} /></li>)}
        </ul>
      )}
      {books.hasNextPage ? (
        <button
          type="button"
          disabled={books.isFetchingNextPage}
          onClick={() => { void books.fetchNextPage(); }}
        >
          {books.isFetchingNextPage ? t("catalog.loadingMore") : t("catalog.loadMore")}
        </button>
      ) : null}
    </section>
  );
}
