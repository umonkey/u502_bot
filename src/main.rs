mod cli;
mod domains;
mod infra;
mod services;
mod shared;

#[tokio::main]
async fn main() -> std::process::ExitCode {
    cli::run().await
}
