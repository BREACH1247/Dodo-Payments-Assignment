#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dodo_assignment::api::run().await
}
