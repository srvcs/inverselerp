use axum::body::Body;
use axum::extract::Json as AxumJson;
use axum::http::{Request, StatusCode};
use axum::routing::post;
use axum::{Json, Router as AxumRouter};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use srvcs_inverselerp::{api::Deps, health, router, telemetry};
use tower::ServiceExt;

const DEAD_URL: &str = "http://127.0.0.1:1";

/// Spawn a *computing* mock `srvcs-floatsubtract`: reads `{"a": x, "b": y}` and
/// returns `{"result": x - y}` — the real float difference. The orchestration is
/// genuinely driven by this answer rather than a canned value.
async fn spawn_floatsubtract() -> String {
    let app = AxumRouter::new().route(
        "/",
        post(|AxumJson(body): AxumJson<Value>| async move {
            let a = body.get("a").and_then(Value::as_f64).unwrap_or(0.0);
            let b = body.get("b").and_then(Value::as_f64).unwrap_or(0.0);
            Json(json!({ "result": a - b }))
        }),
    );
    serve(app).await
}

/// Spawn a *computing* mock `srvcs-floatdivide`: reads `{"a": x, "b": y}` and
/// returns `{"result": x / y}` — the real float quotient, or `422` on a zero
/// divisor.
async fn spawn_floatdivide() -> String {
    let app = AxumRouter::new().route(
        "/",
        post(|AxumJson(body): AxumJson<Value>| async move {
            let a = body.get("a").and_then(Value::as_f64).unwrap_or(0.0);
            let b = body.get("b").and_then(Value::as_f64).unwrap_or(1.0);
            if b == 0.0 {
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Json(json!({ "error": "division by zero" })),
                );
            }
            (StatusCode::OK, Json(json!({ "result": a / b })))
        }),
    );
    serve(app).await
}

/// Spawn a mock returning a fixed status + body (used for error-path tests).
async fn spawn_fixed(status: StatusCode, body: Value) -> String {
    let app = AxumRouter::new().route(
        "/",
        post(move || {
            let body = body.clone();
            async move { (status, Json(body)) }
        }),
    );
    serve(app).await
}

async fn serve(app: AxumRouter) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

fn app(floatsubtract_url: &str, floatdivide_url: &str) -> axum::Router {
    router(
        telemetry::metrics_handle_for_tests(),
        Deps {
            floatsubtract_url: floatsubtract_url.to_string(),
            floatdivide_url: floatdivide_url.to_string(),
        },
    )
}

async fn inverselerp(
    floatsubtract_url: &str,
    floatdivide_url: &str,
    a: f64,
    b: f64,
    value: f64,
) -> (StatusCode, Value) {
    let res = app(floatsubtract_url, floatdivide_url)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "a": a, "b": b, "value": value }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn status_of(uri: &str) -> StatusCode {
    app(DEAD_URL, DEAD_URL)
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap()
        .status()
}

fn approx(got: &Value, expected: f64) -> bool {
    got.as_f64().map(|g| (g - expected).abs() < 1e-9) == Some(true)
}

// --- Standard endpoints. ---

#[tokio::test]
async fn healthz_ok() {
    assert_eq!(status_of("/healthz").await, StatusCode::OK);
}

#[tokio::test]
async fn readyz_reflects_state() {
    health::set_ready(true);
    assert_eq!(status_of("/readyz").await, StatusCode::OK);
}

#[tokio::test]
async fn metrics_ok() {
    assert_eq!(status_of("/metrics").await, StatusCode::OK);
}

#[tokio::test]
async fn openapi_ok() {
    assert_eq!(status_of("/openapi.json").await, StatusCode::OK);
}

#[tokio::test]
async fn generates_request_id_when_absent() {
    let res = app(DEAD_URL, DEAD_URL)
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        res.headers().contains_key("x-request-id"),
        "response must carry a generated x-request-id"
    );
}

#[tokio::test]
async fn index_reports_identity() {
    let res = app(DEAD_URL, DEAD_URL)
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["service"], "srvcs-inverselerp");
    assert_eq!(
        body["concern"],
        "range: inverse linear interpolation (where value lies between a and b)"
    );
    assert_eq!(
        body["depends_on"],
        json!(["srvcs-floatsubtract", "srvcs-floatdivide"])
    );
}

// --- Correctness cases, against the computing mocks. ---

#[tokio::test]
async fn inverselerp_0_10_5_is_half() {
    let (s, d) = (spawn_floatsubtract().await, spawn_floatdivide().await);
    let (status, body) = inverselerp(&s, &d, 0.0, 10.0, 5.0).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["a"], 0.0);
    assert_eq!(body["b"], 10.0);
    assert_eq!(body["value"], 5.0);
    // (5 - 0) / (10 - 0) = 0.5
    assert!(approx(&body["result"], 0.5));
}

#[tokio::test]
async fn inverselerp_at_a_is_zero() {
    let (s, d) = (spawn_floatsubtract().await, spawn_floatdivide().await);
    let (status, body) = inverselerp(&s, &d, 2.0, 8.0, 2.0).await;
    assert_eq!(status, StatusCode::OK);
    // (2 - 2) / (8 - 2) = 0
    assert!(approx(&body["result"], 0.0));
}

#[tokio::test]
async fn inverselerp_at_b_is_one() {
    let (s, d) = (spawn_floatsubtract().await, spawn_floatdivide().await);
    let (status, body) = inverselerp(&s, &d, 2.0, 8.0, 8.0).await;
    assert_eq!(status, StatusCode::OK);
    // (8 - 2) / (8 - 2) = 1
    assert!(approx(&body["result"], 1.0));
}

#[tokio::test]
async fn inverselerp_quarter() {
    let (s, d) = (spawn_floatsubtract().await, spawn_floatdivide().await);
    let (status, body) = inverselerp(&s, &d, 0.0, 100.0, 25.0).await;
    assert_eq!(status, StatusCode::OK);
    // (25 - 0) / (100 - 0) = 0.25
    assert!(approx(&body["result"], 0.25));
}

#[tokio::test]
async fn inverselerp_extrapolates_below() {
    let (s, d) = (spawn_floatsubtract().await, spawn_floatdivide().await);
    let (status, body) = inverselerp(&s, &d, 10.0, 20.0, 5.0).await;
    assert_eq!(status, StatusCode::OK);
    // (5 - 10) / (20 - 10) = -0.5
    assert!(approx(&body["result"], -0.5));
}

#[tokio::test]
async fn inverselerp_fractional_inputs() {
    let (s, d) = (spawn_floatsubtract().await, spawn_floatdivide().await);
    let (status, body) = inverselerp(&s, &d, 1.5, 3.5, 2.0).await;
    assert_eq!(status, StatusCode::OK);
    // (2.0 - 1.5) / (3.5 - 1.5) = 0.5 / 2.0 = 0.25
    assert!(approx(&body["result"], 0.25));
}

// --- Error / degraded paths. ---

#[tokio::test]
async fn degrades_when_floatsubtract_unreachable() {
    let d = spawn_floatdivide().await;
    let (status, body) = inverselerp(DEAD_URL, &d, 0.0, 10.0, 5.0).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["dependency"], "srvcs-floatsubtract");
}

#[tokio::test]
async fn degrades_when_floatdivide_unreachable() {
    // floatsubtract is reachable, so the pipeline reaches the divide call.
    let s = spawn_floatsubtract().await;
    let (status, body) = inverselerp(&s, DEAD_URL, 0.0, 10.0, 5.0).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["dependency"], "srvcs-floatdivide");
}

#[tokio::test]
async fn forwards_422_from_floatsubtract() {
    let d = spawn_floatdivide().await;
    let s = spawn_fixed(
        StatusCode::UNPROCESSABLE_ENTITY,
        json!({ "error": "value is not a number" }),
    )
    .await;
    let (status, _) = inverselerp(&s, &d, 0.0, 10.0, 5.0).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn forwards_422_from_floatdivide() {
    // floatsubtract computes real results, so the pipeline reaches divide,
    // which rejects (degenerate range a == b -> den 0) -> forward 422.
    let s = spawn_floatsubtract().await;
    let d = spawn_floatdivide().await;
    let (status, _) = inverselerp(&s, &d, 5.0, 5.0, 5.0).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn malformed_floatsubtract_result_is_500() {
    // floatsubtract answers 200 but with no float result -> contract violation.
    let d = spawn_floatdivide().await;
    let s = spawn_fixed(StatusCode::OK, json!({ "result": "not-a-number" })).await;
    let (status, body) = inverselerp(&s, &d, 0.0, 10.0, 5.0).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(body["dependency"], "srvcs-floatsubtract");
}

#[tokio::test]
async fn malformed_floatdivide_result_is_500() {
    let s = spawn_floatsubtract().await;
    let d = spawn_fixed(StatusCode::OK, json!({ "result": "not-a-number" })).await;
    let (status, body) = inverselerp(&s, &d, 0.0, 10.0, 5.0).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(body["dependency"], "srvcs-floatdivide");
}
