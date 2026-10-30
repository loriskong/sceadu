//! libp2p transport and session layer for Sceadu.
//!
//! M1c scope: one TCP + Noise connection, signed manifest exchange, and an
//! echo RPC over a libp2p request/response stream.

use std::collections::HashMap;
use std::time::Duration;

use futures::StreamExt;
use libp2p::identity::Keypair;
use libp2p::multiaddr::Protocol;
use libp2p::request_response::{
    self, Message, OutboundRequestId, ProtocolSupport, ResponseChannel,
};
use libp2p::swarm::{StreamProtocol, Swarm, SwarmEvent};
use libp2p::SwarmBuilder;
pub use libp2p::{Multiaddr, PeerId};
use sceadu_core::{AgentId, Capability, Identity, Manifest, ManifestBody};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

const PROTOCOL: &str = "/sceadu/rpc/1.0.0";
const MANIFEST_TTL_SECONDS: u64 = 3600;

type Behaviour = request_response::json::Behaviour<RpcRequest, RpcResponse>;
type BehaviourEvent = request_response::Event<RpcRequest, RpcResponse>;

/// Errors returned by the network layer.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("core error: {0}")]
    Core(#[from] sceadu_core::Error),
    #[error("identity error: {0}")]
    Identity(String),
    #[error("transport error: {0}")]
    Transport(String),
    #[error("behaviour error: {0}")]
    Behaviour(String),
    #[error("listen error: {0}")]
    Listen(String),
    #[error("dial error: {0}")]
    Dial(String),
    #[error("request error: {0}")]
    Request(String),
    #[error("peer mismatch: expected {expected}, got {actual}")]
    PeerMismatch { expected: String, actual: String },
    #[error("node task stopped")]
    Closed,
}

/// Result type used by the network layer.
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Serialize, Deserialize)]
enum RpcRequest {
    Manifest(Box<Manifest>),
    Echo { message: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum RpcResponse {
    Manifest(Box<Manifest>),
    Echo { message: String },
    Error { message: String },
}

#[derive(Default)]
struct NodeState {
    manifests: HashMap<PeerId, Manifest>,
    pending_dials: HashMap<PeerId, oneshot::Sender<Result<PeerId>>>,
    pending_manifests: HashMap<OutboundRequestId, oneshot::Sender<Result<Manifest>>>,
    pending_echoes: HashMap<OutboundRequestId, oneshot::Sender<Result<String>>>,
}

enum Command {
    Dial {
        addr: Multiaddr,
        reply: oneshot::Sender<Result<PeerId>>,
    },
    Manifest {
        peer: PeerId,
        reply: oneshot::Sender<Result<Manifest>>,
    },
    Echo {
        peer: PeerId,
        message: String,
        reply: oneshot::Sender<Result<String>>,
    },
    Shutdown,
}

/// A running Sceadu node. Commands are serialized through the task that owns
/// the libp2p swarm.
pub struct NodeHandle {
    peer_id: PeerId,
    listen_addr: Option<Multiaddr>,
    agent_id: AgentId,
    commands: mpsc::Sender<Command>,
    task: JoinHandle<()>,
}

impl NodeHandle {
    pub fn peer_id(&self) -> PeerId {
        self.peer_id
    }

    pub fn listen_addr(&self) -> Option<&Multiaddr> {
        self.listen_addr.as_ref()
    }

    pub fn agent_id(&self) -> &AgentId {
        &self.agent_id
    }

    pub async fn dial(&self, addr: Multiaddr) -> Result<PeerId> {
        let (reply, receiver) = oneshot::channel();
        self.commands
            .send(Command::Dial { addr, reply })
            .await
            .map_err(|_| Error::Closed)?;
        receiver.await.map_err(|_| Error::Closed)?
    }

    pub async fn request_manifest(&self, peer: PeerId) -> Result<Manifest> {
        let (reply, receiver) = oneshot::channel();
        self.commands
            .send(Command::Manifest { peer, reply })
            .await
            .map_err(|_| Error::Closed)?;
        receiver.await.map_err(|_| Error::Closed)?
    }

    pub async fn echo(&self, peer: PeerId, message: impl Into<String>) -> Result<String> {
        let (reply, receiver) = oneshot::channel();
        self.commands
            .send(Command::Echo {
                peer,
                message: message.into(),
                reply,
            })
            .await
            .map_err(|_| Error::Closed)?;
        receiver.await.map_err(|_| Error::Closed)?
    }

    pub async fn shutdown(self) {
        let _ = self.commands.send(Command::Shutdown).await;
        let _ = self.task.await;
    }
}

/// Start a node, optionally listening on `listen`.
///
/// Returns the handle and the actual listen address (with `/p2p/<peer-id>`).
pub async fn spawn(
    identity: &Identity,
    listen: Option<Multiaddr>,
) -> Result<(NodeHandle, Option<Multiaddr>)> {
    let mut seed = identity.to_seed();
    let keypair = Keypair::ed25519_from_bytes(&mut seed.expose()[..])
        .map_err(|error| Error::Identity(error.to_string()))?;
    let peer_id = keypair.public().to_peer_id();

    let mut swarm = SwarmBuilder::with_existing_identity(keypair)
        .with_tokio()
        .with_tcp(
            libp2p::tcp::Config::default(),
            libp2p::noise::Config::new,
            libp2p::yamux::Config::default,
        )
        .map_err(|error| Error::Transport(error.to_string()))?
        .with_behaviour(|_| behaviour())
        .map_err(|error| Error::Behaviour(error.to_string()))?
        .with_swarm_config(|config| config.with_idle_connection_timeout(Duration::from_secs(60)))
        .build();

    let listen_addr = if let Some(listen) = listen {
        swarm
            .listen_on(listen)
            .map_err(|error| Error::Listen(error.to_string()))?;
        let address = tokio::time::timeout(Duration::from_secs(5), wait_for_listen(&mut swarm))
            .await
            .map_err(|_| Error::Listen("timed out waiting for a listen address".to_owned()))??;
        Some(address.with(Protocol::P2p(peer_id)))
    } else {
        None
    };

    let mut body = ManifestBody::new(identity.agent_id(), 1, MANIFEST_TTL_SECONDS);
    body.capabilities.push(Capability::new("task.echo"));
    if let Some(address) = &listen_addr {
        body.endpoints.push(address.to_string());
    }
    body.protocols.push(PROTOCOL.to_owned());
    let local_manifest = Manifest::sign(body, identity)?;

    let (commands, receiver) = mpsc::channel(32);
    let task = tokio::spawn(run_node(swarm, local_manifest.clone(), receiver));

    let handle = NodeHandle {
        peer_id,
        listen_addr: listen_addr.clone(),
        agent_id: local_manifest.body.agent_id.clone(),
        commands,
        task,
    };

    Ok((handle, listen_addr))
}

fn behaviour() -> Behaviour {
    request_response::json::Behaviour::new(
        [(StreamProtocol::new(PROTOCOL), ProtocolSupport::Full)],
        request_response::Config::default(),
    )
}

async fn wait_for_listen(swarm: &mut Swarm<Behaviour>) -> Result<Multiaddr> {
    loop {
        if let SwarmEvent::NewListenAddr { address, .. } = swarm.select_next_some().await {
            return Ok(address);
        }
    }
}

async fn run_node(
    mut swarm: Swarm<Behaviour>,
    local_manifest: Manifest,
    mut commands: mpsc::Receiver<Command>,
) {
    let mut state = NodeState::default();

    loop {
        tokio::select! {
            event = swarm.select_next_some() => {
                handle_event(&mut swarm, event, &local_manifest, &mut state);
            }
            command = commands.recv() => {
                match command {
                    Some(command) => {
                        if !handle_command(&mut swarm, command, &local_manifest, &mut state) {
                            break;
                        }
                    }
                    None => break,
                }
            }
        }
    }
}

fn handle_event(
    swarm: &mut Swarm<Behaviour>,
    event: SwarmEvent<BehaviourEvent>,
    local_manifest: &Manifest,
    state: &mut NodeState,
) {
    match event {
        SwarmEvent::ConnectionEstablished { peer_id, .. } => {
            if let Some(reply) = state.pending_dials.remove(&peer_id) {
                let _ = reply.send(Ok(peer_id));
            }
        }
        SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
            let message = format!("dial failed: {error}");
            if let Some(peer) = peer_id {
                if let Some(reply) = state.pending_dials.remove(&peer) {
                    let _ = reply.send(Err(Error::Dial(message)));
                }
            } else {
                for (_, reply) in state.pending_dials.drain() {
                    let _ = reply.send(Err(Error::Dial(message.clone())));
                }
            }
        }
        SwarmEvent::ConnectionClosed { peer_id, .. } => {
            state.manifests.remove(&peer_id);
        }
        SwarmEvent::Behaviour(event) => {
            handle_behaviour_event(swarm, event, local_manifest, state);
        }
        _ => {}
    }
}

fn handle_behaviour_event(
    swarm: &mut Swarm<Behaviour>,
    event: BehaviourEvent,
    local_manifest: &Manifest,
    state: &mut NodeState,
) {
    match event {
        request_response::Event::Message { peer, message, .. } => match message {
            Message::Request {
                request, channel, ..
            } => respond_to_request(swarm, peer, request, channel, local_manifest, state),
            Message::Response {
                request_id,
                response,
            } => handle_response(peer, request_id, response, state),
        },
        request_response::Event::OutboundFailure {
            request_id, error, ..
        } => {
            let message = format!("request failed: {error}");
            if let Some(reply) = state.pending_manifests.remove(&request_id) {
                let _ = reply.send(Err(Error::Request(message.clone())));
            }
            if let Some(reply) = state.pending_echoes.remove(&request_id) {
                let _ = reply.send(Err(Error::Request(message)));
            }
        }
        _ => {}
    }
}

fn respond_to_request(
    swarm: &mut Swarm<Behaviour>,
    peer: PeerId,
    request: RpcRequest,
    channel: ResponseChannel<RpcResponse>,
    local_manifest: &Manifest,
    state: &mut NodeState,
) {
    let response = match request {
        RpcRequest::Manifest(manifest) => match verify_manifest_for_peer(&manifest, &peer) {
            Ok(()) => {
                state.manifests.insert(peer, *manifest);
                RpcResponse::Manifest(Box::new(local_manifest.clone()))
            }
            Err(error) => RpcResponse::Error {
                message: error.to_string(),
            },
        },
        RpcRequest::Echo { message } => {
            if state.manifests.contains_key(&peer) {
                RpcResponse::Echo { message }
            } else {
                RpcResponse::Error {
                    message: "manifest exchange required".to_owned(),
                }
            }
        }
    };

    let _ = swarm.behaviour_mut().send_response(channel, response);
}

fn handle_response(
    peer: PeerId,
    request_id: OutboundRequestId,
    response: RpcResponse,
    state: &mut NodeState,
) {
    if let Some(reply) = state.pending_manifests.remove(&request_id) {
        let result = match response {
            RpcResponse::Manifest(manifest) => {
                let manifest = *manifest;
                match verify_manifest_for_peer(&manifest, &peer) {
                    Ok(()) => {
                        state.manifests.insert(peer, manifest.clone());
                        Ok(manifest)
                    }
                    Err(error) => Err(error),
                }
            }
            RpcResponse::Error { message } => Err(Error::Request(message)),
            RpcResponse::Echo { .. } => Err(Error::Request("unexpected echo response".to_owned())),
        };
        let _ = reply.send(result);
        return;
    }

    if let Some(reply) = state.pending_echoes.remove(&request_id) {
        let result = match response {
            RpcResponse::Echo { message } => Ok(message),
            RpcResponse::Error { message } => Err(Error::Request(message)),
            RpcResponse::Manifest(_) => {
                Err(Error::Request("unexpected manifest response".to_owned()))
            }
        };
        let _ = reply.send(result);
    }
}

fn handle_command(
    swarm: &mut Swarm<Behaviour>,
    command: Command,
    local_manifest: &Manifest,
    state: &mut NodeState,
) -> bool {
    match command {
        Command::Dial { addr, reply } => {
            let peer = match peer_from_multiaddr(&addr) {
                Some(peer) => peer,
                None => {
                    let _ = reply.send(Err(Error::Dial(
                        "multiaddr must include /p2p/<peer-id>".to_owned(),
                    )));
                    return true;
                }
            };

            if swarm.is_connected(&peer) {
                let _ = reply.send(Ok(peer));
                return true;
            }

            match swarm.dial(addr) {
                Ok(()) => {
                    state.pending_dials.insert(peer, reply);
                }
                Err(error) => {
                    let _ = reply.send(Err(Error::Dial(error.to_string())));
                }
            }
            true
        }
        Command::Manifest { peer, reply } => {
            let request_id = swarm.behaviour_mut().send_request(
                &peer,
                RpcRequest::Manifest(Box::new(local_manifest.clone())),
            );
            state.pending_manifests.insert(request_id, reply);
            true
        }
        Command::Echo {
            peer,
            message,
            reply,
        } => {
            let request_id = swarm
                .behaviour_mut()
                .send_request(&peer, RpcRequest::Echo { message });
            state.pending_echoes.insert(request_id, reply);
            true
        }
        Command::Shutdown => false,
    }
}

fn verify_manifest_for_peer(manifest: &Manifest, peer: &PeerId) -> Result<()> {
    manifest.verify()?;
    let expected = peer_id_for_agent_id(&manifest.body.agent_id)?;
    if &expected != peer {
        return Err(Error::PeerMismatch {
            expected: expected.to_string(),
            actual: peer.to_string(),
        });
    }
    Ok(())
}

fn peer_id_for_agent_id(agent_id: &AgentId) -> Result<PeerId> {
    let public = libp2p::identity::ed25519::PublicKey::try_from_bytes(agent_id.as_bytes())
        .map_err(|error| Error::Identity(error.to_string()))?;
    Ok(PeerId::from_public_key(&libp2p::identity::PublicKey::from(
        public,
    )))
}

fn peer_from_multiaddr(address: &Multiaddr) -> Option<PeerId> {
    address.iter().find_map(|protocol| match protocol {
        Protocol::P2p(peer) => Some(peer),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_peer_id_matches_agent_id() {
        let identity = Identity::generate();
        let peer = peer_id_for_agent_id(&identity.agent_id()).unwrap();
        let keypair = {
            let mut seed = identity.to_seed();
            Keypair::ed25519_from_bytes(&mut seed.expose()[..]).unwrap()
        };
        assert_eq!(peer, keypair.public().to_peer_id());
    }

    #[test]
    fn rejects_manifest_from_a_different_peer() {
        let signer = Identity::generate();
        let other = Identity::generate();
        let mut body = ManifestBody::new(signer.agent_id(), 1, MANIFEST_TTL_SECONDS);
        body.capabilities.push(Capability::new("task.echo"));
        let manifest = Manifest::sign(body, &signer).unwrap();
        let peer = peer_id_for_agent_id(&other.agent_id()).unwrap();

        assert!(matches!(
            verify_manifest_for_peer(&manifest, &peer),
            Err(Error::PeerMismatch { .. })
        ));
    }
}
