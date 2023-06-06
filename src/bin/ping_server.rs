use clap::Parser;
use std::error::Error;
use std::net::SocketAddrV4;
use std::str::FromStr;
use tokio::io::AsyncWriteExt;
use tokio::{io, net::TcpListener};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();

    let addr = SocketAddrV4::from_str(&args.address).unwrap();

    let listener = TcpListener::bind(&addr).await?;
    println!("Listening on: {addr}");

    loop {
        let (mut socket, _) = listener.accept().await?;

        tokio::spawn(async move {
            let (mut si, mut so) = socket.split();
            io::copy(&mut si, &mut so).await?;
            so.shutdown().await
        });
    }
}

#[derive(Parser, Debug)]
struct Args {
    /// Socket address to listen on
    #[arg(long)]
    address: String,
}
