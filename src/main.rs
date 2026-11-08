#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    openfga_axum_lab::run().await
}
