use axum::{
    Router,
    extract::Path,
    http::HeaderMap,
    response::{Html, IntoResponse, Response},
    routing::get,
};

use super::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/{code}", get(document))
}

struct Documentation {
    language: &'static str,
    html: String,
}

fn localized_document(code: &str, accept_language: &str) -> Option<Documentation> {
    let chinese = preferred_chinese(accept_language);
    let (title, detail) = match (chinese, code) {
        (true, "mail-delivery-unavailable") => (
            "邮件投递暂不可用",
            "系统暂时无法投递必需的邮件。请稍后重试。",
        ),
        (false, "mail-delivery-unavailable") => (
            "Mail delivery unavailable",
            "The service cannot currently deliver required email. Please try again later.",
        ),
        _ => return None,
    };
    Some(Documentation {
        language: if chinese { "zh-CN" } else { "en" },
        html: format!(
            "<!doctype html><html lang=\"{}\"><head><meta charset=\"utf-8\"><title>{}</title></head><body><h1>{}</h1><p>{}</p><p><code>{}</code></p></body></html>",
            if chinese { "zh-CN" } else { "en" },
            escape(title),
            escape(title),
            escape(detail),
            escape(&code)
        ),
    })
}

async fn document(Path(code): Path<String>, headers: HeaderMap) -> Response {
    let accept = headers
        .get("accept-language")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    let Some(document) = localized_document(&code, accept) else {
        return axum::http::StatusCode::NOT_FOUND.into_response();
    };
    let mut response = Html(document.html).into_response();
    response.headers_mut().insert(
        "Vary",
        axum::http::HeaderValue::from_static("Accept-Language"),
    );
    response.headers_mut().insert(
        "Content-Language",
        axum::http::HeaderValue::from_static(document.language),
    );
    response
}

fn preferred_chinese(header: &str) -> bool {
    header
        .split(',')
        .filter_map(|item| {
            let mut pieces = item.trim().split(';');
            let lang = pieces.next()?.trim().to_ascii_lowercase();
            let q = pieces
                .find_map(|part| part.trim().strip_prefix("q=")?.parse::<f32>().ok())
                .unwrap_or(1.0);
            Some((lang, q))
        })
        .filter(|(_, q)| *q > 0.0)
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .is_some_and(|(lang, _)| lang == "zh" || lang.starts_with("zh-"))
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use super::localized_document;

    #[test]
    fn documents_the_kebab_case_type_code_and_negotiates_chinese_by_quality() {
        let document = localized_document("mail-delivery-unavailable", "en;q=0.5, zh-CN;q=0.9")
            .expect("known stable code");
        assert!(document.html.contains("邮件投递暂不可用"));
        assert_eq!(document.language, "zh-CN");
    }
}
