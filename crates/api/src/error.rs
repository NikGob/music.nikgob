//! Errors as the API answers them, `{"error", "message", "details"}`, and
//! JSON answers and bodies with the API's own error on a bad one.

use axum::body::Bytes;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde::de::DeserializeOwned;
use utoipa::ToSchema;

/// The content type of every JSON answer.
pub const JSON: &str = "application/json; charset=utf-8";

/// An error answer.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct ErrorBody {
    /// `bad_request`, `unauthorized`, `forbidden`, `not_found`,
    /// `nothing_to_play`, `rate_limited`, `no_audio`, `player_timeout`,
    /// `trash_off`, `confirm_unknown`, `confirm_expired`,
    /// `confirm_mismatch`, `no_services`, `lookup_failed`,
    /// `lookup_timeout`, `plan_changed`, `not_done`, `too_many`,
    /// `outside_library`, `offline`, `nothing_playable`,
    /// `transfer_failed`, `transfer_timeout`, `internal`.
    pub error: String,
    /// What went wrong, in Russian.
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<ErrorDetails>,
    /// `plan_changed`: the plan as it is now, with a new `confirm_token`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<Object>)]
    pub plan: Option<serde_json::Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct ErrorDetails {
    /// The right the token lacks (403): `read`, `player`, `queue`,
    /// `volume`, `duck`, `output`, `media`, `library`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub needs: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
    pub needs: Option<&'static str>,
    /// The new plan of a `plan_changed`.
    pub plan: Option<serde_json::Value>,
}

impl ApiError {
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self { status, code, message: message.into(), needs: None, plan: None }
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "bad_request", message)
    }

    pub fn unauthorized() -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "unauthorized", "A live token is required")
    }

    pub fn forbidden(needs: &'static str) -> Self {
        Self { needs: Some(needs), ..Self::new(StatusCode::FORBIDDEN, "forbidden", "The token lacks the right") }
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", message)
    }

    pub fn nothing_to_play() -> Self {
        Self::new(StatusCode::CONFLICT, "nothing_to_play", "The queue is empty")
    }

    pub fn rate_limited() -> Self {
        Self::new(StatusCode::TOO_MANY_REQUESTS, "rate_limited", "Too many requests")
    }

    pub fn no_audio() -> Self {
        Self::new(StatusCode::SERVICE_UNAVAILABLE, "no_audio", "The output device is unavailable")
    }

    /// The player did not get to the request in time: it may run later, or
    /// not at all.
    pub fn timeout() -> Self {
        Self::new(StatusCode::GATEWAY_TIMEOUT, "player_timeout", "The player did not answer in time")
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", message)
    }

    /// An error of its own code and status (`trash_off` 409,
    /// `confirm_expired` 409, `lookup_timeout` 504…).
    pub fn of(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self::new(status, code, message)
    }

    /// `409 plan_changed`: what was confirmed is not what would be done
    /// now; `plan` is the new one, with its own token.
    pub fn plan_changed(plan: serde_json::Value) -> Self {
        Self {
            plan: Some(plan),
            ..Self::new(StatusCode::CONFLICT, "plan_changed", "The plan changed since it was shown: here is the new one, confirm it")
        }
    }

    pub fn body(&self) -> ErrorBody {
        ErrorBody {
            error: self.code.to_owned(),
            message: self.message.clone(),
            details: self.needs.map(|n| ErrorDetails { needs: Some(n.to_owned()) }),
            plan: self.plan.clone(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.status;
        let mut r = Json(self.body()).into_response();
        *r.status_mut() = status;
        r
    }
}

/// A JSON answer (`application/json; charset=utf-8`).
pub struct Json<T>(pub T);

impl<T: Serialize> IntoResponse for Json<T> {
    fn into_response(self) -> Response {
        match serde_json::to_vec(&self.0) {
            Ok(bytes) => {
                let mut r = Response::new(axum::body::Body::from(bytes));
                r.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static(JSON));
                r
            }
            Err(e) => {
                log::error!("api: an answer could not be written: {e}");
                let mut r = Response::new(axum::body::Body::from(
                    r#"{"error":"internal","message":"Failed to build the response"}"#,
                ));
                *r.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
                r.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static(JSON));
                r
            }
        }
    }
}

/// A request body as `T`; an empty body is `T::default()`. Whatever the
/// content type (curl sends a form type without `-H`).
pub fn body<T: DeserializeOwned + Default>(bytes: &Bytes) -> Result<T, ApiError> {
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(T::default());
    }
    serde_json::from_slice(bytes).map_err(|e| ApiError::bad_request(format!("Invalid request body: {e}")))
}

/// The query string's `key=value` pairs.
pub fn query_pairs(raw: Option<&str>) -> Vec<(String, String)> {
    raw.map(|q| form_urlencoded::parse(q.as_bytes()).map(|(k, v)| (k.into_owned(), v.into_owned())).collect()).unwrap_or_default()
}

/// One value of the query string.
pub fn query_value(raw: Option<&str>, key: &str) -> Option<String> {
    query_pairs(raw).into_iter().find(|(k, _)| k == key).map(|(_, v)| v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_read_as_the_contract_says() {
        let e = serde_json::to_value(ApiError::forbidden("duck").body()).unwrap();
        assert_eq!(e, serde_json::json!({"error": "forbidden", "message": "The token lacks the right", "details": {"needs": "duck"}}));
        let e = serde_json::to_value(ApiError::nothing_to_play().body()).unwrap();
        assert_eq!(e["error"], "nothing_to_play");
        assert!(e.get("details").is_none());
        assert_eq!(ApiError::nothing_to_play().status, StatusCode::CONFLICT);
        assert_eq!(ApiError::rate_limited().status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(ApiError::no_audio().status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!((ApiError::timeout().status, ApiError::timeout().code), (StatusCode::GATEWAY_TIMEOUT, "player_timeout"));
    }

    #[derive(Debug, Default, serde::Deserialize, PartialEq)]
    struct B {
        a: Option<u32>,
    }

    #[test]
    fn bodies() {
        assert_eq!(body::<B>(&Bytes::from_static(b"")).unwrap(), B::default());
        assert_eq!(body::<B>(&Bytes::from_static(b"  \n")).unwrap(), B::default());
        assert_eq!(body::<B>(&Bytes::from_static(b"{\"a\":3}")).unwrap(), B { a: Some(3) });
        assert_eq!(body::<B>(&Bytes::from_static(b"{nope")).unwrap_err().code, "bad_request");
        assert_eq!(query_value(Some("fade_ms=450&x=1"), "fade_ms").as_deref(), Some("450"));
        assert_eq!(query_value(Some("q=%D0%BA%D0%B8%D0%BD%D0%BE+x"), "q").as_deref(), Some("кино x"));
        assert_eq!(query_value(None, "q"), None);
    }
}
