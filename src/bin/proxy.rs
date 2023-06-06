use axum::{
    routing::{get, post},
    Router,
};
use proxima_centauri::{process_command, root, GlobalState};
use std::sync::Arc;
use tracing::Level;

#[tokio::main]
async fn main() {
    let addr: std::net::SocketAddr = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:14000".to_string())
        .parse()
        .unwrap();

    // initialize tracing
    let subscriber = tracing_subscriber::FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .finish();

    tracing::subscriber::set_global_default(subscriber).unwrap();

    let verifying_key = std::env::args().nth(1);

    let shared_state = Arc::new(GlobalState::new(verifying_key.as_ref()));
    // build our application with a route
    let app = Router::new()
        // `GET /` goes to `root`
        .route("/", get(root))
        // `POST /command` goes to `process_command`
        .route("/command", post(process_command))
        .with_state(shared_state);

    // run our app with hyper
    tracing::debug!("listening  on {}", addr);
    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await
        .unwrap();
}
