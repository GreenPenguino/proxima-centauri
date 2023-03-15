use axum::extract::State;
use axum::{http::StatusCode, Json};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::{Arc, Mutex};
use tokio::io::{self, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch::{self, Receiver, Sender};
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
        destination_ip: Ipv4Addr,
        id: Uuid,
    },
    Modify {
        destination_port: u16,
        destination_ip: Ipv4Addr,
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

#[derive(Debug)]
pub struct GlobalState {
    proxies: Mutex<HashMap<Uuid, ProxyState>>,
}

impl GlobalState {
    pub fn new() -> Self {
        Self {
            proxies: Mutex::new(HashMap::new()),
        }
    }
}

#[derive(Debug)]
struct ProxyState {
    destination: SocketAddrV4,
    control: Sender<ProxyControlMessage>,
}

pub async fn root() -> &'static str {
    "Hello, World!"
}

pub async fn process_command(
    State(state): State<Arc<GlobalState>>,
    Json(payload): Json<ProxyCommand>,
) -> (StatusCode, Json<ProxyResponse>) {
    tracing::info!("Received payload: {:?}", payload);
    // TODO: verify signature
    match payload.command {
        Command::New {
            incoming_port,
            destination_port,
            destination_ip,
            id,
        } => {
            let addr = SocketAddrV4::new(destination_ip, destination_port);
            let (tx, rx) = watch::channel(ProxyControlMessage::Open { destination: addr });
            state.proxies.lock().unwrap().insert(
                id,
                ProxyState {
                    destination: addr,
                    control: tx,
                },
            );
            add_proxy(incoming_port, rx).await.unwrap(); // TODO: error propagation??
        }
        Command::Modify {
            destination_port,
            destination_ip,
            id,
        } => {
            if let Some(proxy) = state.proxies.lock().unwrap().get_mut(&id) {
                proxy.destination.set_port(destination_port);
                proxy.destination.set_ip(destination_ip);
                proxy
                    .control
                    .send(ProxyControlMessage::Open {
                        destination: proxy.destination,
                    })
                    .unwrap();
            }
        }
        Command::Delete { id } => {
            if let Some(proxy) = state.proxies.lock().unwrap().get_mut(&id) {
                proxy.control.send(ProxyControlMessage::Close).unwrap();
            }
        }
    }
    (
        StatusCode::CREATED,
        Json(ProxyResponse {
            message: "Success".to_string(),
        }),
    )
}

#[derive(Debug)]
enum ProxyControlMessage {
    Open { destination: SocketAddrV4 }, // Reroute { new: SocketAddr },
    Close,
}

async fn add_proxy(in_port: u16, control: Receiver<ProxyControlMessage>) -> anyhow::Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", in_port)).await?;

    tracing::info!("proxying port {in_port} to {:?}", *control.borrow());

    tokio::spawn(proxy(listener, control));
    Ok(())
}

async fn proxy(listener: TcpListener, mut control: Receiver<ProxyControlMessage>) {
    loop {
        tokio::select! {
            l = listener.accept()=> {
                if let Ok((inbound, _)) = l {
                    let transfer = transfer(inbound, control.clone());

                    tokio::spawn(transfer);
                }
            }
            _ = control.changed() => {
                match *control.borrow() {
                    ProxyControlMessage::Open { destination } => {
                        tracing::info!("destination for proxy port {} changed to {}", listener.local_addr().unwrap(), destination);
                    },
                    ProxyControlMessage::Close => {
                        tracing::info!("destination for proxy port {} closed", listener.local_addr().unwrap());
                        return;
                    },
                }
            }
        }
    }
}

async fn transfer(
    mut inbound: TcpStream,
    mut control: Receiver<ProxyControlMessage>,
) -> anyhow::Result<()> {
    loop {
        let current_destination =
            if let ProxyControlMessage::Open { destination } = *control.borrow() {
                Some(destination)
            } else {
                break Ok(());
            };
        let mut outbound = TcpStream::connect(current_destination.unwrap()).await?;

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

        // Select between the copy tasks and watch channel
        tokio::select! {
            // Join the two copy streams and wait for the connection to clone
            result = async move { tokio::join!(client_to_server, server_to_client) } => {
                match result {
                    (Ok(_), Ok(_)) => {
                        break Ok(());
                    }
                    (r1, r2) => {
                        if r1.is_err() {
                            tracing::error!("error closing client->server of {:?}: {:?}", inbound, &r1);
                        }
                        if r2.is_err() {
                            tracing::error!("error closing server->client of {:?}: {:?}", inbound, &r2);
                        }
                        r1?;
                        r2?;
                    },
                }
            }
            _ = control.changed() => {
                match *control.borrow() {
                    ProxyControlMessage::Open { destination } => {
                        eprintln!("Switching to new destination: {destination}");
                        // Disconnect the current outbound connection and restart the loop
                        drop(outbound);
                        continue;
                    },
                    ProxyControlMessage::Close => {
                        break Ok(());
                    },
                }

            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use crate::{Command, ProxyCommand};
    use uuid::uuid;

    #[test]
    fn serialize_proxy_command_new() {
        let proxy_command = ProxyCommand {
            command: Command::New {
                incoming_port: 5555,
                destination_port: 6666,
                destination_ip: Ipv4Addr::new(127, 0, 0, 1),
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
