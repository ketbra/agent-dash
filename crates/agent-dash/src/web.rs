use crate::client_listener::ClientMessage;
use axum::{response::Html, routing::get, Router};
use tokio::sync::mpsc;

/// Start the web server on the given port. Pass 0 to disable.
pub async fn run(port: u16, _client_tx: mpsc::Sender<ClientMessage>) {
    if port == 0 {
        return;
    }

    let app = Router::new()
        .route("/", get(index_handler));

    let addr = format!("127.0.0.1:{port}");
    eprintln!("  web interface: http://{addr}");

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .expect("failed to bind web server");
    axum::serve(listener, app).await.expect("web server error");
}

async fn index_handler() -> Html<&'static str> {
    Html("<h1>agent-dash</h1>")
}
