use axum::{
    routing::{get, post},
    Router,
};
use proxima_centauri::{process_command, root};
use std::net::SocketAddr;
use tracing::Level;

#[tokio::main]
async fn main() {
    // initialize tracing
    let subscriber = tracing_subscriber::FmtSubscriber::builder()
        .with_max_level(Level::TRACE)
        .finish();

    tracing::subscriber::set_global_default(subscriber).unwrap();

    // build our application with a route
    let app = Router::new()
        // `GET /` goes to `root`
        .route("/", get(root))
        // `POST /command` goes to `process_command`
        .route("/command", post(process_command));

    // run our app with hyper
    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    tracing::debug!("listening  on {}", addr);
    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await
        .unwrap();
}
