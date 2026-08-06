use axum::{
    Router,
    extract::Path,
    http::HeaderMap,
    response::{Html, IntoResponse, Response},
    routing::get,
};

use super::AppState;
use crate::problem::{ProblemDocumentKind, problem_document};

pub fn router() -> Router<AppState> {
    Router::new().route("/{code}", get(document))
}

pub(crate) fn stateless_router() -> Router {
    Router::new().route("/{code}", get(document))
}

struct Documentation {
    language: &'static str,
    html: String,
}

fn localized_document(code: &str, accept_language: &str) -> Option<Documentation> {
    let entry = problem_document(code)?;
    let chinese = preferred_chinese(accept_language);
    let (title, detail) = copy(entry.kind, chinese);
    Some(Documentation {
        language: if chinese { "zh-CN" } else { "en" },
        html: format!(
            "<!doctype html><html lang=\"{}\"><head><meta charset=\"utf-8\"><title>{}</title></head><body><h1>{}</h1><p>{}</p><p><code>{}</code></p></body></html>",
            if chinese { "zh-CN" } else { "en" },
            escape(title),
            escape(title),
            escape(detail),
            escape(code)
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
    let ranges = header
        .split(',')
        .filter_map(|item| {
            let mut pieces = item.trim().split(';');
            let lang = pieces.next()?.trim().to_ascii_lowercase();
            let q = pieces
                .find_map(|part| part.trim().strip_prefix("q=")?.parse::<f32>().ok())
                .unwrap_or(1.0);
            (q.is_finite() && (0.0..=1.0).contains(&q)).then_some((lang, q))
        })
        .collect::<Vec<_>>();
    let en = supported_quality(&ranges, "en");
    let zh = supported_quality(&ranges, "zh-cn");
    zh > en && zh > 0.0
}

fn supported_quality(ranges: &[(String, f32)], supported: &str) -> f32 {
    let primary = supported.split('-').next().unwrap_or(supported);
    let mut selected: Option<(u8, f32)> = None;
    for (range, quality) in ranges {
        let range_primary = range.split('-').next().unwrap_or(range);
        let specificity = if range == supported {
            2
        } else if range != "*" && range_primary == primary {
            1
        } else if range == "*" {
            0
        } else {
            continue;
        };
        if selected.is_none_or(|(current, _)| specificity > current) {
            selected = Some((specificity, *quality));
        }
    }
    selected.map_or(0.0, |(_, quality)| quality)
}

const fn copy(kind: ProblemDocumentKind, chinese: bool) -> (&'static str, &'static str) {
    match (kind, chinese) {
        (ProblemDocumentKind::Authentication, false) => (
            "Authentication required",
            "Sign in with a valid session before retrying this request.",
        ),
        (ProblemDocumentKind::Authentication, true) => {
            ("需要身份验证", "请使用有效会话登录后重试此请求。")
        }
        (ProblemDocumentKind::Forbidden, false) => (
            "Action forbidden",
            "The authenticated account is not permitted to perform this action.",
        ),
        (ProblemDocumentKind::Forbidden, true) => ("操作被禁止", "当前已验证账户无权执行此操作。"),
        (ProblemDocumentKind::NotFound, false) => (
            "Resource not found",
            "The requested resource is absent or is not visible to this account.",
        ),
        (ProblemDocumentKind::NotFound, true) => {
            ("未找到资源", "请求的资源不存在或当前账户不可见。")
        }
        (ProblemDocumentKind::Conflict, false) => (
            "Request conflict",
            "The request conflicts with the current state. Refresh state before retrying.",
        ),
        (ProblemDocumentKind::Conflict, true) => {
            ("请求冲突", "请求与当前状态冲突。请刷新状态后重试。")
        }
        (ProblemDocumentKind::Invalid, false) => (
            "Invalid request",
            "One or more request values are malformed, unsupported, or expired.",
        ),
        (ProblemDocumentKind::Invalid, true) => {
            ("请求无效", "一个或多个请求值格式错误、不受支持或已过期。")
        }
        (ProblemDocumentKind::TooLarge, false) => (
            "Payload too large",
            "The submitted payload exceeds the configured upload limit.",
        ),
        (ProblemDocumentKind::TooLarge, true) => {
            ("请求体过大", "提交的请求体超过了配置的上传限制。")
        }
        (ProblemDocumentKind::RateLimited, false) => (
            "Too many requests",
            "Wait for the response Retry-After interval before retrying.",
        ),
        (ProblemDocumentKind::RateLimited, true) => {
            ("请求过于频繁", "请等待响应中的 Retry-After 时间后重试。")
        }
        (ProblemDocumentKind::Storage, false) => (
            "Storage exhausted",
            "The service currently lacks enough physical storage for this operation.",
        ),
        (ProblemDocumentKind::Storage, true) => {
            ("存储空间不足", "服务当前没有足够的物理存储空间完成此操作。")
        }
        (ProblemDocumentKind::MailUnavailable, false) => (
            "Mail delivery unavailable",
            "The service cannot currently deliver required email. Please try again later.",
        ),
        (ProblemDocumentKind::MailUnavailable, true) => (
            "邮件投递暂不可用",
            "系统暂时无法投递必需的邮件。请稍后重试。",
        ),
        (ProblemDocumentKind::Unavailable, false) => (
            "Service temporarily unavailable",
            "A required service is temporarily unavailable. Retry later.",
        ),
        (ProblemDocumentKind::Unavailable, true) => {
            ("服务暂不可用", "所需服务暂时不可用。请稍后重试。")
        }
        (ProblemDocumentKind::Internal, false) => (
            "Internal server error",
            "The service encountered an unexpected error. Use the request ID when reporting it.",
        ),
        (ProblemDocumentKind::Internal, true) => (
            "服务器内部错误",
            "服务遇到意外错误。报告问题时请提供请求 ID。",
        ),
    }
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
