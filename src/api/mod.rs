use std::sync::Arc;

use axum::{AddExtensionLayer, Router};
use tower::ServiceBuilder;
use tower_http::trace::TraceLayer;

use crate::config::Config;

mod routes;

fn api_router() -> Router {
    routes::calculate::router()
}

pub async fn serve(config: Config) -> anyhow::Result<()> {
    let server_host = config.api_host.to_owned();
    let server_port = config.api_port.unwrap();

    let app = api_router().layer(
        ServiceBuilder::new()
            .layer(TraceLayer::new_for_http())
            .layer(AddExtensionLayer::new(Arc::new(config))),
    );

    log::info!("serving on {}", server_port);
    axum::Server::bind(&format!("{}:{}", server_host, server_port).parse()?)
        .serve(app.into_make_service())
        .await?;

    Ok(())
}
