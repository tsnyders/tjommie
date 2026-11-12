//! `server.ts` routes (M6): extra HTTP endpoints such as `/mcp` and the desktop host routes. Only programs on this
//! machine reach them: a local Host and no Origin header at all (browsers always send one).

mod host_support;

use std::sync::Arc;

use axum::response::IntoResponse;
use daemon::server::{Handlers, Routes, Server, ServerOptions};
use host_support::{get, http};

async fn server(defer: bool) -> Server {
    Server::start(ServerOptions {
        port: 0,
        token: "synthetic-token".into(),
        handlers: Handlers::default(),
        allowed_origins: vec!["app://tjommie".into()],
        defer_requests: defer,
    })
    .await
    .unwrap()
}

fn routes() -> Routes {
    Routes::default().route(
        "/probe",
        Arc::new(|request| {
            Box::pin(
                async move { format!("{} {}", request.method(), request.uri()).into_response() },
            )
        }),
    )
}

#[tokio::test]
async fn serves_a_route_only_to_local_programs_without_an_origin() {
    let server = server(false).await;
    server
        .activate_with("synthetic-token", Handlers::default(), routes())
        .unwrap();
    let port = server.port();
    let reply = get(port, "/probe?control=1", &[]).await;
    assert_eq!(
        (reply.status, String::from_utf8_lossy(&reply.body).as_ref()),
        (200, "GET /probe?control=1")
    );
    assert_eq!(http(port, "POST", "/probe", &[], b"{}").await.status, 200);
    // Even an Origin the WebSocket would accept is refused: routes are for programs, never pages.
    assert_eq!(
        get(port, "/probe", &[("Origin", "app://tjommie")])
            .await
            .status,
        403
    );
    assert_eq!(get(port, "/probe", &[("Origin", "null")]).await.status, 403);
    assert_eq!(
        get(port, "/probe", &[("Host", "evil.example")])
            .await
            .status,
        403
    );
    assert_eq!(
        get(port, "/probe", &[("Host", &format!("localhost:{port}"))])
            .await
            .status,
        200
    );
    assert_eq!(get(port, "/elsewhere", &[]).await.status, 404);
    assert_eq!(get(port, "/health", &[]).await.status, 200);
    server.close().await;
}

#[tokio::test]
async fn answers_503_until_activated_and_drops_routes_on_plain_activate() {
    let server = server(true).await;
    let port = server.port();
    assert_eq!(get(port, "/probe", &[]).await.status, 503);
    server
        .activate_with("synthetic-token", Handlers::default(), routes())
        .unwrap();
    assert_eq!(get(port, "/probe", &[]).await.status, 200);
    server
        .activate("synthetic-token", Handlers::default())
        .unwrap();
    assert_eq!(get(port, "/probe", &[]).await.status, 404);
    server.close().await;
}
