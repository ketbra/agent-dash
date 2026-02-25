use crate::client_listener::ClientMessage;
use axum::http::{header, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use tokio::sync::mpsc;

const INDEX_HTML: &str = include_str!("../web/index.html");
const APP_JS: &str = include_str!("../web/app.js");
const STYLE_CSS: &str = include_str!("../web/style.css");

pub async fn run(port: u16, _client_tx: mpsc::Sender<ClientMessage>) {
    if port == 0 {
        return;
    }

    let app = Router::new()
        .route("/", get(index_handler))
        .route("/app.js", get(js_handler))
        .route("/style.css", get(css_handler));

    let addr = format!("127.0.0.1:{port}");
    eprintln!("  web interface: http://{addr}");

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .expect("failed to bind web server");
    axum::serve(listener, app).await.expect("web server error");
}

async fn index_handler() -> Html<&'static str> {
    Html(INDEX_HTML)
}

async fn js_handler() -> Response {
    (StatusCode::OK, [(header::CONTENT_TYPE, "application/javascript")], APP_JS).into_response()
}

async fn css_handler() -> Response {
    (StatusCode::OK, [(header::CONTENT_TYPE, "text/css")], STYLE_CSS).into_response()
}
