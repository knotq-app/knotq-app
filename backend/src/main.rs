use std::net::SocketAddr;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let addr: SocketAddr = std::env::var("KNOTQ_BACKEND_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:7878".to_string())
        .parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    eprintln!("knotq-backend listening on http://{addr}");
    axum::serve(listener, knotq_backend::app()).await?;
    Ok(())
}
