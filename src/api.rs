use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use utoipa::{OpenApi, ToSchema};

use crate::client::{self, DepError};

pub const SERVICE: &str = "srvcs-inverselerp";
pub const CONCERN: &str = "range: inverse linear interpolation (where value lies between a and b)";
pub const DEPENDS_ON: &[&str] = &["srvcs-floatsubtract", "srvcs-floatdivide"];

/// Dependency endpoints, injected as router state so tests can point them at
/// mock services.
#[derive(Clone)]
pub struct Deps {
    pub floatsubtract_url: String,
    pub floatdivide_url: String,
}

#[derive(Serialize, ToSchema)]
pub struct Info {
    pub service: &'static str,
    pub concern: &'static str,
    pub depends_on: Vec<&'static str>,
}

/// `GET /` — service identity (srvcs service standard).
#[utoipa::path(get, path = "/", responses((status = 200, body = Info)))]
pub async fn index() -> Json<Info> {
    Json(Info {
        service: SERVICE,
        concern: CONCERN,
        depends_on: DEPENDS_ON.to_vec(),
    })
}

#[derive(Deserialize, ToSchema)]
pub struct EvalRequest {
    #[schema(value_type = Object)]
    pub a: Value,
    #[schema(value_type = Object)]
    pub b: Value,
    #[schema(value_type = Object)]
    pub value: Value,
}

#[derive(Serialize, ToSchema)]
pub struct ResultResponse {
    #[schema(value_type = Object)]
    pub a: Value,
    #[schema(value_type = Object)]
    pub b: Value,
    #[schema(value_type = Object)]
    pub value: Value,
    /// Where `value` lies between `a` and `b`, as a fraction (`f64`).
    pub result: f64,
}

fn ok(a: Value, b: Value, value: Value, result: f64) -> Response {
    (
        StatusCode::OK,
        Json(json!({ "a": a, "b": b, "value": value, "result": result })),
    )
        .into_response()
}

fn degraded(dependency: &str) -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({ "error": "dependency unavailable", "dependency": dependency })),
    )
        .into_response()
}

fn forward(status: u16, body: Value) -> Response {
    let code = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
    (code, Json(body)).into_response()
}

/// A reachable dependency answered `200` but its body lacked a float `result`.
/// That is a contract violation we cannot recover from, so surface a `500`
/// rather than guessing.
fn malformed(dependency: &str) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(
            json!({ "error": "dependency returned a malformed result", "dependency": dependency }),
        ),
    )
        .into_response()
}

/// Call one dependency at `url` with `body`, mapping its outcome to either the
/// parsed response body (on `200`) or an early-return `Response` the caller
/// should surface verbatim:
///
/// - unreachable / non-`200`/`422` -> `503` degraded
/// - `422` -> forwarded `422` (the dependency rejected the input)
async fn ask(url: &str, body: &Value, dependency: &str) -> Result<Value, Response> {
    match client::call(url, body).await {
        Err(DepError::Unreachable) => Err(degraded(dependency)),
        Ok((200, body)) => Ok(body),
        Ok((422, body)) => Err(forward(422, body)),
        Ok(_) => Err(degraded(dependency)),
    }
}

/// `POST /` — compute the inverse linear interpolation of `value` between `a`
/// and `b` by composing two float primitives.
///
/// This service owns the *control flow* but delegates every arithmetic step to
/// its dependencies, exactly as specified:
///
/// 1. ask `srvcs-floatsubtract` for `num = value - a`;
/// 2. ask `srvcs-floatsubtract` for `den = b - a`;
/// 3. ask `srvcs-floatdivide` for `result = num / den`.
///
/// `inverselerp(0, 10, 5) == 0.5`.
///
/// If a dependency is unreachable it reports itself degraded (`503`); if a
/// dependency rejects the input it forwards the `422`.
#[utoipa::path(
    post,
    path = "/",
    request_body = EvalRequest,
    responses(
        (status = 200, body = ResultResponse),
        (status = 422, description = "a dependency rejected the input (forwarded)"),
        (status = 500, description = "a dependency returned a malformed result"),
        (status = 503, description = "a dependency is unavailable")
    )
)]
pub async fn evaluate(State(deps): State<Deps>, Json(req): Json<EvalRequest>) -> Response {
    // 1. num = value - a
    let num_body = match ask(
        &deps.floatsubtract_url,
        &json!({ "a": req.value, "b": req.a }),
        "srvcs-floatsubtract",
    )
    .await
    {
        Ok(body) => body,
        Err(resp) => return resp,
    };
    let num = match num_body.get("result").and_then(Value::as_f64) {
        Some(n) => n,
        None => return malformed("srvcs-floatsubtract"),
    };

    // 2. den = b - a
    let den_body = match ask(
        &deps.floatsubtract_url,
        &json!({ "a": req.b, "b": req.a }),
        "srvcs-floatsubtract",
    )
    .await
    {
        Ok(body) => body,
        Err(resp) => return resp,
    };
    let den = match den_body.get("result").and_then(Value::as_f64) {
        Some(d) => d,
        None => return malformed("srvcs-floatsubtract"),
    };

    // 3. result = num / den
    let div_body = match ask(
        &deps.floatdivide_url,
        &json!({ "a": num, "b": den }),
        "srvcs-floatdivide",
    )
    .await
    {
        Ok(body) => body,
        Err(resp) => return resp,
    };
    let result = match div_body.get("result").and_then(Value::as_f64) {
        Some(r) => r,
        None => return malformed("srvcs-floatdivide"),
    };

    ok(req.a, req.b, req.value, result)
}

#[derive(OpenApi)]
#[openapi(
    paths(index, evaluate),
    components(schemas(Info, EvalRequest, ResultResponse))
)]
pub struct ApiDoc;

/// Serve OpenAPI document
pub async fn openapi_json() -> Json<utoipa::openapi::OpenApi> {
    Json(ApiDoc::openapi())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openapi_documents_routes() {
        let doc = ApiDoc::openapi();
        let root = doc.paths.paths.get("/").expect("path / present");
        assert!(root.get.is_some());
        assert!(root.post.is_some());
    }

    #[tokio::test]
    async fn index_reports_all_dependencies() {
        let Json(info) = index().await;
        assert_eq!(info.service, "srvcs-inverselerp");
        assert_eq!(
            info.concern,
            "range: inverse linear interpolation (where value lies between a and b)"
        );
        assert_eq!(
            info.depends_on,
            vec!["srvcs-floatsubtract", "srvcs-floatdivide"]
        );
    }
}
