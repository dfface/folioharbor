#![forbid(unsafe_code)]

use folioharbor_cli::{admin, check_storage, commands, migrate};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().json().init();
    let command = commands::parse(std::env::args().skip(1))?;
    match command {
        commands::Command::Migrate => migrate::run().await,
        commands::Command::CreateAdmin { email } => admin::run(email).await,
        commands::Command::CheckStorage => check_storage::run().await,
    }
}
