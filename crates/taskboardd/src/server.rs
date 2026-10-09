//! The HTTP server: the JSON API under `/tasks/api` (Taskboard.app and `tb` are its clients) and
//! attachment files at `/tasks/files/:id`.

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::{Body, Bytes};
use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::Router;
use serde_json::{json, Value};

use crate::api;
use crate::app::App;

const MAX_BODY: usize = 2 * 1024 * 1024;

pub fn router(app: Arc<App>) -> Router {
    Router::new().fallback(handle).with_state(app)
}

fn json_response(status: StatusCode, v: &Value) -> Response {
    (status, [(header::CONTENT_TYPE, "application/json"), (header::CACHE_CONTROL, "no-store")], v.to_string()).into_response()
}

fn error(status: u16, message: &str) -> Response {
    json_response(StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR), &json!({"error": message}))
}




fn serve_file(app: &App, id: &str) -> Response {
    match api::attachment_file(app, id) {
        Ok(p) => match std::fs::read(&p) {
            Ok(bytes) => {
                let mime = mime_guess::from_path(&p).first_or_octet_stream();
                let mime = if mime.type_() == "text" || mime.essence_str() == "application/javascript" {
                    "text/plain; charset=utf-8".to_string()
                } else {
                    mime.to_string()
                };
                (
                    StatusCode::OK,
                    [
                        (header::CONTENT_TYPE, mime),
                        (header::CACHE_CONTROL, "no-cache".to_string()),
                        (header::CONTENT_SECURITY_POLICY, "sandbox".to_string()),
                        (header::HeaderName::from_static("x-content-type-options"), "nosniff".to_string()),
                    ],
                    bytes,
                )
                    .into_response()
            }
            Err(_) => error(404, "That file can't be read."),
        },
        Err(e) => error(e.status, &e.message),
    }
}

async fn handle(
    State(app): State<Arc<App>>,
    method: Method,
    uri: Uri,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let path = uri.path().to_string();
    if path == "/" || path == "/tasks" || path == "/tasks/" {
        return json_response(StatusCode::OK, &json!({"board": "taskboardd", "version": env!("CARGO_PKG_VERSION"),
            "api": "/tasks/api", "app": "Open Taskboard.app to see the board."}));
    }
    let Some(rest) = path.strip_prefix("/tasks/") else { return error(404, "There's nothing at that address.") };
    if method == Method::OPTIONS {
        return error(403, "That isn't allowed here.");
    }
    if let Some(api_path) = rest.strip_prefix("api/").or(if rest == "api" { Some("") } else { None }) {
        if method != Method::GET && method != Method::POST {
            return error(405, "That isn't allowed here.");
        }
        if method == Method::POST && headers.get("x-task-board").and_then(|v| v.to_str().ok()) != Some("1") {
            return error(403, "Requests to the board need the X-Task-Board header.");
        }
        if body.len() > MAX_BODY {
            return error(413, "That request is too big.");
        }
        let body: Value = if body.is_empty() {
            json!({})
        } else {
            match serde_json::from_slice(&body) {
                Ok(v) => v,
                Err(_) => return error(400, "The request body isn't valid JSON."),
            }
        };
        // Only the app's header with this launch's app token says a request is the owner's own click
        // (`api::FROM`, `apptoken`); the header alone is anyone's say-so.
        let mut query = query;
        query.remove(api::FROM);
        if headers.get("x-task-board-from").and_then(|v| v.to_str().ok()) == Some("app") {
            if !crate::apptoken::matches(headers.get(crate::apptoken::HEADER).and_then(|v| v.to_str().ok()), &app.app_token) {
                return error(403, "That request says it's from the app, but it doesn't carry the app's token.");
            }
            query.insert(api::FROM.to_string(), "app".to_string());
        }
        let m = method.as_str().to_string();
        let p = api_path.to_string();
        let a = app.clone();
        let started = std::time::Instant::now();
        let out = tokio::task::spawn_blocking(move || api::dispatch(&a, &m, &p, &query, &body)).await;
        let ms = started.elapsed().as_millis();
        if ms > 1000 {
            app.info(format!("slow request: {} {} took {ms} ms", method, path));
        }
        return match out {
            Ok(Ok(v)) => json_response(StatusCode::OK, &v),
            Ok(Err(e)) => {
                if e.status >= 500 {
                    app.info(format!("{} {} failed: {}", method, path, e.message));
                }
                error(e.status, &e.message)
            }
            Err(e) => {
                app.info(format!("{} {} crashed: {e}", method, path));
                error(500, "The board hit an error. It's in the board's log.")
            }
        };
    }
    if method != Method::GET {
        return error(405, "That isn't allowed here.");
    }
    if let Some(id) = rest.strip_prefix("files/") {
        return serve_file(&app, id);
    }
    error(404, "There's nothing at that address.")
}

pub fn not_found() -> Response {
    (StatusCode::NOT_FOUND, Body::from("")).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::http::Request;
    use tower::ServiceExt;

    use crate::apptoken;

    fn board() -> (Router, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let app = App::for_tests(crate::config::Config::for_tests(dir.path()));
        (router(app), dir)
    }

    async fn call(r: Router, req: Request<Body>) -> (StatusCode, Value) {
        let resp = r.oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    #[tokio::test]
    async fn root_says_where_the_board_is() {
        let (r, _d) = board();
        for p in ["/", "/tasks", "/tasks/"] {
            let (s, v) = call(r.clone(), Request::get(p).body(Body::empty()).unwrap()).await;
            assert_eq!(s, StatusCode::OK, "{p}");
            assert_eq!(v["api"], "/tasks/api");
        }
    }

    #[tokio::test]
    async fn the_old_web_page_is_gone() {
        let (r, _d) = board();
        for p in ["/tasks/static/app.js", "/tasks/sessions", "/favicon.svg"] {
            let (s, _) = call(r.clone(), Request::get(p).body(Body::empty()).unwrap()).await;
            assert_eq!(s, StatusCode::NOT_FOUND, "{p}");
        }
    }

    #[tokio::test]
    async fn posts_still_need_the_header() {
        let (r, _d) = board();
        let req = Request::post("/tasks/api/tasks").header("content-type", "application/json").body(Body::from("{}")).unwrap();
        let (s, v) = call(r.clone(), req).await;
        assert_eq!(s, StatusCode::FORBIDDEN);
        assert!(v["error"].as_str().unwrap().contains("X-Task-Board"));
        let (s, v) = call(r, Request::get("/tasks/api/state").body(Body::empty()).unwrap()).await;
        assert_eq!(s, StatusCode::OK);
        assert!(v["columns"].is_object());
    }

    /// A board with one task waiting for Start: (router, app, T<id>).
    fn board_with_task() -> (Router, Arc<App>, i64, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let app = App::for_tests(crate::config::Config::for_tests(dir.path()));
        let repo = dir.path().join("webapp");
        std::fs::create_dir_all(&repo).unwrap();
        app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string())).unwrap();
        let t = api::dispatch(&app, "POST", "/tasks", &HashMap::new(), &json!({"title": "Add login", "project": "webapp"})).unwrap();
        (router(app.clone()), app, t["id"].as_i64().unwrap(), dir)
    }

    fn post(path: &str, headers: &[(&str, &str)], body: Value) -> Request<Body> {
        let mut req = Request::post(path).header("content-type", "application/json").header("x-task-board", "1");
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        req.body(Body::from(body.to_string())).unwrap()
    }

    fn log(app: &App, id: i64) -> Vec<String> {
        use crate::util::RowExt;
        app.db.q("SELECT text FROM events WHERE task_id = ? ORDER BY id", crate::p![id]).unwrap().iter().map(|r| r.st("text")).collect()
    }

    #[tokio::test]
    async fn the_app_header_without_the_apps_token_is_refused() {
        let (r, app, id, _d) = board_with_task();
        let start = format!("/tasks/api/tasks/T{id}/start");
        for headers in [
            vec![("x-task-board-from", "app")],
            vec![("x-task-board-from", "app"), (apptoken::HEADER, "")],
            vec![("x-task-board-from", "app"), (apptoken::HEADER, "not-the-token")],
            vec![("x-task-board-from", "app"), (apptoken::HEADER, &app.app_token[..10])],
        ] {
            let (s, v) = call(r.clone(), post(&start, &headers, json!({"mode": "queue"}))).await;
            assert_eq!(s, StatusCode::FORBIDDEN, "{headers:?}");
            assert!(v["error"].as_str().unwrap().contains("app's token"), "{v}");
        }
        // The query can't say it either.
        let (s, v) = call(r.clone(), post(&format!("{start}?_from=app"), &[], json!({"mode": "queue"}))).await;
        assert_eq!(s, StatusCode::FORBIDDEN);
        assert!(v["error"].as_str().unwrap().contains("Only a human can start"), "{v}");
        // Nor can the token with no app header.
        let (s, _) = call(r.clone(), post(&start, &[(apptoken::HEADER, &app.app_token)], json!({"mode": "queue"}))).await;
        assert_eq!(s, StatusCode::FORBIDDEN);
        assert!(!log(&app, id).iter().any(|l| l.starts_with("Started")), "{:?}", log(&app, id));

        // Every other app-only signal goes through the same door (a wave's review stop, here).
        let (s, v) = call(r, post("/tasks/api/goals/G1/waves/1", &[("x-task-board-from", "app")], json!({"stop_after": true}))).await;
        assert_eq!(s, StatusCode::FORBIDDEN);
        assert!(v["error"].as_str().unwrap().contains("app's token"), "{v}");
    }

    #[tokio::test]
    async fn the_apps_own_start_carries_its_token() {
        let (r, app, id, _d) = board_with_task();
        let token = app.app_token.clone();
        let req = post(&format!("/tasks/api/tasks/T{id}/start"), &[("x-task-board-from", "app"), (apptoken::HEADER, &token)], json!({"mode": "queue"}));
        let (s, v) = call(r, req).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert!(log(&app, id).contains(&"Started in the UI".to_string()), "{:?}", log(&app, id));
    }

    #[test]
    fn each_board_has_its_own_token() {
        let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (a, b) = (App::for_tests(crate::config::Config::for_tests(a.path())), App::for_tests(crate::config::Config::for_tests(b.path())));
        assert_ne!(a.app_token, b.app_token);
        assert_eq!(a.app_token.len(), 64);
    }
}
