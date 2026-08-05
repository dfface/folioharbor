#![forbid(unsafe_code)]

// Keep startup fallible so later adapters can propagate initialization errors.
#[allow(clippy::unnecessary_wraps)]
fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    tracing::info!("folioharbor-api");
    Ok(())
}
