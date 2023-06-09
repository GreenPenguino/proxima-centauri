use axum::extract::State;
use axum::{http::StatusCode, Json};
use p384::ecdsa::signature::Verifier;
use p384::ecdsa::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::str::FromStr;
use std::sync::{Arc, Mutex, RwLock};
use std::time;
use tokio::io::{self, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch::{self, Receiver, Sender};
use uuid::Uuid;

#[derive(Deserialize, Serialize, Debug)]
pub struct ProxyCommand {
    #[serde(flatten)]
    command: Command,
    timestamp: Option<u64>,
    signature: Option<Signature>,
}

impl ProxyCommand {
    fn verify_signature(&self, verifying_key: &Option<VerifyingKey>) -> bool {
        match (verifying_key, &self.signature) {
            (Some(key), Some(signature)) => {
                let mut message = serde_json::to_string(&self.command).unwrap();

                let timestamp = if let Some(timestamp) = self.timestamp {
                    message.push_str(&timestamp.to_string());
                    time::Duration::from_secs(timestamp)
                } else {
                    tracing::debug!("timestamp missing while signature is present");
                    return false; // timestamp missing with signature present
                };

                if !key.verify(message.as_bytes(), signature).is_ok() {
                    tracing::debug!("signature does not match message");
                    return false; // signature doesn't match
                }

                let now = time::SystemTime::now()
                    .duration_since(time::UNIX_EPOCH)
                    .unwrap();
                if timestamp > (now + time::Duration::from_secs(30)) {
                    tracing::warn!("command is more than 30s from the future");
                    false
                } else if now - timestamp <= time::Duration::from_secs(60) {
                    // less than a minute old
                    true
                } else {
                    tracing::warn!("command is more than a minute old");
                    false
                }
            }
            (Some(_), None) => false,
            (None, _) => true,
        }
    }
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "snake_case")]
enum Command {
    Create {
        incoming_port: u16,
        destination_port: u16,
        destination_ip: IpAddr,
        id: Uuid,
    },
    Modify {
        destination_port: u16,
        destination_ip: IpAddr,
        id: Uuid,
    },
    Delete {
        id: Uuid,
    },
    Status,
}

#[derive(Serialize)]
pub enum ProxyResponse {
    Message(String),
    Status {
        tunnels: HashMap<Uuid, (u16, SocketAddr)>,
    },
}

#[derive(Debug)]
pub struct GlobalState {
    proxies: Mutex<HashMap<Uuid, ProxyState>>,
    ports: RwLock<HashSet<u16>>,
    verifying_key: Option<VerifyingKey>,
}

impl GlobalState {
    pub fn new<S: AsRef<str>>(verifying_key: Option<S>) -> Self {
        Self {
            proxies: Mutex::new(HashMap::new()),
            ports: RwLock::new(HashSet::new()),
            verifying_key: verifying_key.and_then(|key| VerifyingKey::from_str(key.as_ref()).ok()),
        }
    }
}

#[derive(Debug)]
struct ProxyState {
    incoming_port: u16,
    destination: SocketAddr,
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

    if !payload.verify_signature(&state.verifying_key) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(ProxyResponse::Message("Invalid signature".to_string())),
        );
    }
    match payload.command {
        Command::Create {
            incoming_port,
            destination_port,
            destination_ip,
            id,
        } => {
            // Check if ID or incoming_port already exists
            if state.proxies.lock().unwrap().get(&id).is_some() {
                return (
                    StatusCode::CONFLICT,
                    Json(ProxyResponse::Message(
                        "Id already exists. Use the modify command instead.".to_string(),
                    )),
                );
            }
            if !state.ports.write().unwrap().insert(incoming_port) {
                return (
                    StatusCode::CONFLICT,
                    Json(ProxyResponse::Message(format!(
                        "The `incoming_port` already in use: {incoming_port}"
                    ))),
                );
            }

            let addr = SocketAddr::new(destination_ip, destination_port);
            let (tx, rx) = watch::channel(ProxyControlMessage::Open { destination: addr });
            state.proxies.lock().unwrap().insert(
                id,
                ProxyState {
                    incoming_port,
                    destination: addr,
                    control: tx,
                },
            );
            add_proxy(incoming_port, rx).await.unwrap(); // TODO: error propagation??
            (
                StatusCode::ACCEPTED,
                Json(ProxyResponse ::
                    Message( format!(
                        "Created tunnel {id} on port {incoming_port} to use {destination_ip}:{destination_port}"
                    ),
                )),
            )
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
                (
                    StatusCode::ACCEPTED,
                    Json(ProxyResponse::Message(format!(
                        "Changed tunnel {id} to use {destination_ip}:{destination_port}"
                    ))),
                )
            } else {
                (
                    StatusCode::NOT_FOUND,
                    Json(ProxyResponse::Message(format!("Id not found: {id}"))),
                )
            }
        }
        Command::Delete { id } => {
            if let Some(proxy) = state.proxies.lock().unwrap().remove(&id) {
                proxy.control.send(ProxyControlMessage::Close).unwrap();
                state.ports.write().unwrap().remove(&proxy.incoming_port);
                (
                    StatusCode::ACCEPTED,
                    Json(ProxyResponse::Message(format!("Deleted tunnel: {id}"))),
                )
            } else {
                (
                    StatusCode::NOT_FOUND,
                    Json(ProxyResponse::Message(format!("Id not found: {id}"))),
                )
            }
        }
        Command::Status => (
            StatusCode::OK,
            Json(ProxyResponse::Status {
                tunnels: state
                    .proxies
                    .lock()
                    .unwrap()
                    .iter()
                    .map(|(key, value)| (*key, (value.incoming_port, value.destination)))
                    .collect(),
            }),
        ),
    }
}

#[derive(Debug)]
enum ProxyControlMessage {
    Open { destination: SocketAddr },
    Close,
}

async fn add_proxy(in_port: u16, control: Receiver<ProxyControlMessage>) -> anyhow::Result<()> {
    let listener = TcpListener::bind(("0.0.0.0", in_port)).await?;

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
    use std::{
        net::{IpAddr, Ipv4Addr},
        time,
    };

    use crate::{Command, ProxyCommand};
    use p384::{
        ecdsa::{signature::Signer, Signature, SigningKey, VerifyingKey},
        elliptic_curve::rand_core::OsRng,
    };
    use uuid::uuid;

    #[test]
    fn serialize_proxy_command_create() {
        let key = SigningKey::from_slice(&[1; 48]).unwrap();
        let signature = key.sign(&[]); // Not a valid signature
        let proxy_command = ProxyCommand {
            command: Command::Create {
                incoming_port: 5555,
                destination_port: 6666,
                destination_ip: IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
                id: uuid!("67e55044-10b1-426f-9247-bb680e5fe0c8"),
            },
            timestamp: Some(8888),
            signature: Some(signature),
        };
        let expected = "{\"create\":{\"incoming_port\":5555,\"destination_port\":6666,\"\
                        destination_ip\":\"127.0.0.1\",\"id\":\"67e55044-10b1-426f-9247-bb680e5fe0c8\"},\
                        \"timestamp\":8888,\
                        \"signature\":\"\
                            5C912C4B3BFF2ADB49885DCBDB53D6D3041D0632E498CDFF\
                            2114CD2DCAC936AB0901B47C411E5BB57FE77BEF96044940\
                            81680ADAD0775CD144E2D2678537F621ED587E13EB430126\
                            C7A757AEC99CE08A2D0F3A5C9FB45E9349F36408DFD7BA17\"}";

        assert_eq!(serde_json::to_string(&proxy_command).unwrap(), expected);
    }

    #[test]
    fn serialize_proxy_command_delete() {
        let key = SigningKey::from_slice(&[1; 48]).unwrap();
        let signature = key.sign(&[]); // Not a valid signature
        let proxy_command = ProxyCommand {
            command: Command::Delete {
                id: uuid!("67e55044-10b1-426f-9247-bb680e5fe0c8"),
            },
            timestamp: Some(987654),
            signature: Some(signature),
        };
        let expected = "{\"delete\":{\"id\":\"67e55044-10b1-426f-9247-bb680e5fe0c8\"},\
                        \"timestamp\":987654,\
                        \"signature\":\"\
                            5C912C4B3BFF2ADB49885DCBDB53D6D3041D0632E498CDFF\
                            2114CD2DCAC936AB0901B47C411E5BB57FE77BEF96044940\
                            81680ADAD0775CD144E2D2678537F621ED587E13EB430126\
                            C7A757AEC99CE08A2D0F3A5C9FB45E9349F36408DFD7BA17\"}";

        assert_eq!(serde_json::to_string(&proxy_command).unwrap(), expected);
    }

    #[test]
    fn verify_signature() {
        let subscriber = tracing_subscriber::FmtSubscriber::builder()
            .with_max_level(tracing::Level::TRACE)
            .finish();
        tracing::subscriber::set_global_default(subscriber).unwrap();

        let command = Command::Create {
            incoming_port: 4567,
            destination_port: 7654,
            destination_ip: IpAddr::V4(Ipv4Addr::new(123, 23, 76, 21)),
            id: uuid::Uuid::new_v4(),
        };

        // Create signed message
        let signing_key = SigningKey::random(&mut OsRng);
        let timestamp = time::SystemTime::now()
            .duration_since(time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let mut message = serde_json::to_string(&command).unwrap();
        message.push_str(&timestamp.to_string());
        let signature: Signature = signing_key.sign(message.as_bytes());
        let bytes = signature.to_bytes();
        assert_eq!(bytes.len(), 96);
        let proxy_command = ProxyCommand {
            command,
            timestamp: Some(timestamp),
            signature: Some(signature),
        };

        // Verify signed message
        let verifying_key = VerifyingKey::from(&signing_key);
        assert!(proxy_command.verify_signature(&Some(verifying_key)));
    }
}
