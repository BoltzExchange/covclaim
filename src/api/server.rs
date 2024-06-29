use std::io::Error;
use std::sync::Arc;

use axum::routing::post;
use axum::{Extension, Router};
use elements::AddressParams;
use tower_http::cors::CorsLayer;

use crate::api;
use crate::api::types::RouterState;
use crate::db::Pool;

pub async fn start_server(
    db: Pool,
    address_params: &'static AddressParams,
    host: &str,
    port: u32,
) -> Result<Result<(), Error>, Error> {
    let shared_state = Arc::new(RouterState { db, address_params });

    let app = Router::new()
        .route("/covenant", post(api::routes::post_covenant_claim))
        .layer(CorsLayer::permissive())
        .layer(Extension(shared_state));

    let listener = tokio::net::TcpListener::bind(format!("{}:{}", host, port)).await;
    if listener.is_err() {
        return Err(listener.err().unwrap());
    }

    Ok(axum::serve(listener.unwrap(), app.clone()).await)
}
