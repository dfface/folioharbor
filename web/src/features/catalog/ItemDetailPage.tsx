import { useTranslation } from "react-i18next";
import { Link, useParams } from "react-router";

import { requestErrorMessage } from "../auth/form";
import { useCurrentLibrary } from "../libraries/LibraryLayout";
import { useItem } from "./queries";

export function ItemDetailPage() {
  const { t } = useTranslation();
  const library = useCurrentLibrary();
  const { itemId = "" } = useParams();
  const item = useItem(library.library_id, itemId);
  if (item.isPending) {
    return <p role="status">{t("catalog.loadingItem")}</p>;
  }
  if (item.isError) {
    return <p role="alert">{requestErrorMessage(item.error, t)}</p>;
  }
  const detail = item.data;
  return (
    <article aria-labelledby="item-title">
      <h3 id="item-title">{detail.primary_title}</h3>
      <dl>
        <div><dt>{t("catalog.work")}</dt><dd>{detail.authors.length > 0 ? detail.authors.join(", ") : t("catalog.unknownAuthor")}</dd></div>
        <div><dt>{t("catalog.edition")}</dt><dd>{t("catalog.epub")}</dd></div>
        <div><dt>{t("catalog.copy")}</dt><dd>{t("catalog.available")}</dd></div>
      </dl>
      <div>
        {detail.can_read ? <Link to={`/libraries/${library.library_id}/items/${detail.item_id}/read`}>{t("catalog.readOnline")}</Link> : null}{" "}
        {detail.can_download ? <a href={`/api/v1/items/${encodeURIComponent(detail.item_id)}/download`}>{t("catalog.download")}</a> : null}
      </div>
      {detail.can_read && !detail.can_download ? <p>{t("catalog.onlineOnly")}</p> : null}
    </article>
  );
}
