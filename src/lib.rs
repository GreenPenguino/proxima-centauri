use axum::{http::StatusCode, Json};
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, SocketAddr};
use tokio::io::{self, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use uuid::Uuid;

#[derive(Deserialize, Serialize, Debug)]
pub struct ProxyCommand {
    command: Command,
    signature: [u8; 32],
}

#[derive(Deserialize, Serialize, Debug)]
enum Command {
    New {
        incoming_port: u16,
        destination_port: u16,
        destination_ip: IpAddr,
        id: Uuid,
    },
    Modify {
        destionation_ip: IpAddr,
        id: Uuid,
    },
    Delete {
        id: Uuid,
    },
}

#[derive(Serialize)]
pub struct ProxyResponse {
    message: String,
}

pub async fn root() -> &'static str {
    "Hello, World!"
}

pub async fn process_command(
    Json(payload): Json<ProxyCommand>,
) -> (StatusCode, Json<ProxyResponse>) {
    tracing::error!("Received payload: {:?}", payload);
    // TODO: verify signature
    match payload.command {
        Command::New {
            incoming_port,
            destination_port,
            destination_ip,
            id,
        } => {
            // TODO: add id to global proxy map
            add_proxy(
                incoming_port,
                SocketAddr::new(destination_ip, destination_port),
            )
            .await
            .unwrap(); // TODO: error propagation??
        }
        Command::Modify {
            destionation_ip,
            id,
        } => todo!(),
        Command::Delete { id } => todo!(),
    }
    (
        StatusCode::CREATED,
        Json(ProxyResponse {
            message: "Success".to_string(),
        }),
    )
}

async fn add_proxy(in_port: u16, destination: SocketAddr) -> anyhow::Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", in_port)).await?;

    tracing::info!("proxying port {in_port} to {destination}");

    tokio::spawn(proxy(listener, destination));
    Ok(())
}

async fn proxy(listener: TcpListener, destination: SocketAddr) {
    while let Ok((inbound, _)) = listener.accept().await {
        let transfer = transfer(inbound, destination);

        tokio::spawn(transfer);
    }
}

async fn transfer(mut inbound: TcpStream, destination: SocketAddr) -> anyhow::Result<()> {
    let mut outbound = TcpStream::connect(destination).await?;

    let (mut ri, mut wi) = inbound.split();
    let (mut ro, mut wo) = outbound.split();

    let client_to_server = async {
        io::copy(&mut ri, &mut wo).await?;
        wo.shutdown().await
    };

    let server_to_client = async {
        io::copy(&mut ro, &mut wi).await?;
        wi.shutdown().await
    };

    tokio::try_join!(client_to_server, server_to_client)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use crate::{Command, ProxyCommand};
    use serde::{Deserialize, Serialize};
    use uuid::uuid;

    #[test]
    fn serialize_proxy_command_new() {
        let proxy_command = ProxyCommand {
            command: Command::New {
                incoming_port: 5555,
                destination_port: 6666,
                destination_ip: std::net::IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
                id: uuid!("67e55044-10b1-426f-9247-bb680e5fe0c8"),
            },
            signature: [0u8; 32],
        };
        let expected = "{\"command\":{\"New\":{\"incoming_port\":5555,\"destination_port\":6666,\"\
                        destination_ip\":\"127.0.0.1\",\"id\":\"67e55044-10b1-426f-9247-bb680e5fe0c8\"}},\
                        \"signature\":[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0]}";
        assert_eq!(serde_json::to_string(&proxy_command).unwrap(), expected);
    }

    #[test]
    fn serialize_proxy_command_delete() {
        let proxy_command = ProxyCommand {
            command: Command::Delete {
                id: uuid!("67e55044-10b1-426f-9247-bb680e5fe0c8"),
            },
            signature: [0u8; 32],
        };
        let expected = "{\"command\":{\"Delete\":{\"id\":\"67e55044-10b1-426f-9247-bb680e5fe0c8\"}},\
                        \"signature\":[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0]}";
        assert_eq!(serde_json::to_string(&proxy_command).unwrap(), expected);
    }
}
