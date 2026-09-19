#[tokio::main]
async fn main() -> Result<(), triptych::BoxError> {
    let _tracing_guard = triptych::logging::init_tracing();
    triptych::run().await
}
