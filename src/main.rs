mod cli;
mod infra;

#[tokio::main]
async fn main() {
    cli::run().await;
}
