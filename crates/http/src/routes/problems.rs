use axum::{
    Router,
    extract::Path,
    http::HeaderMap,
    response::{Html, IntoResponse},
    routing::get,
};

use super::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/{code}", get(document))
}

async fn document(Path(code): Path<String>, headers: HeaderMap) -> impl IntoResponse {
    let chinese = headers
        .get("accept-language")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.to_ascii_lowercase().starts_with("zh"));
    let (title, detail) = match (chinese, code.as_str()) {
        (true, "mail_delivery_unavailable") => (
            "邮件投递暂不可用",
            "系统暂时无法投递必需的邮件。请稍后重试。",
        ),
        (false, "mail_delivery_unavailable") => (
            "Mail delivery unavailable",
            "The service cannot currently deliver required email. Please try again later.",
        ),
        (true, _) => (
            "请求问题说明",
            "此页面说明稳定的问题代码；响应 JSON 中的 code 仍是客户端应使用的机器可读标识。",
        ),
        (false, _) => (
            "Problem documentation",
            "This page documents a stable problem code; clients should continue to use the machine-readable code from the JSON response.",
        ),
    };
    Html(format!(
        "<!doctype html><html lang=\"{}\"><head><meta charset=\"utf-8\"><title>{}</title></head><body><h1>{}</h1><p>{}</p><p><code>{}</code></p></body></html>",
        if chinese { "zh-CN" } else { "en" },
        escape(title),
        escape(title),
        escape(detail),
        escape(&code)
    ))
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
