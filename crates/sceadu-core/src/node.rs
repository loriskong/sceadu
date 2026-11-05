//! libp2p transport and session layer for Sceadu.
//!
//! M1c scope: one TCP + Noise connection, signed manifest exchange, and an
//! echo RPC over a libp2p request/response stream.

use std::collections::{HashMap, HashSet, VecDeque};
use std::net::SocketAddrV6;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::agent::TaskHandle;
use crate::discovery::{capability_key, manifest_key, validate_contacts, NeighborPolicy};
use crate::identity::ContactRecord;
use crate::session::{RpcRequest, RpcResponse, TaskEvent, TaskEventKind, PROTOCOL};
use crate::transport::tor::TorTransport;
use crate::transport::{
    classify_multiaddr, endpoint_record, ensure_dial_allowed, ensure_ipv6_listen_addr,
    ensure_ipv6_peer_addr, ensure_operation_allowed, ensure_peer_id, is_onion, ConnectivityPolicy,
    EndpointRecord, NetworkStatus, Reachability, TransportKind, TransportStatus,
};
use crate::{AgentId, Capability, Error, Identity, Manifest, ManifestBody, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use futures::StreamExt;
use libp2p::core::upgrade::Version;
use libp2p::core::Transport;
use libp2p::identity::Keypair;
use libp2p::kad::store::MemoryStore;
use libp2p::kad::{self, GetProvidersOk, GetRecordOk, QueryId, QueryResult, Quorum, Record};
use libp2p::multiaddr::Protocol;
use libp2p::request_response::{
    self, Message, OutboundRequestId, ProtocolSupport, ResponseChannel,
};
use libp2p::swarm::behaviour::toggle::Toggle;
use libp2p::swarm::{NetworkBehaviour, StreamProtocol, Swarm, SwarmEvent};
use libp2p::SwarmBuilder;
pub use libp2p::{Multiaddr, PeerId};
use rand_core::{OsRng, RngCore};
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

const MANIFEST_TTL_SECONDS: u64 = 3600;
const QUARANTINE_FAILURES: u32 = 3;
const QUARANTINE_DURATION: Duration = Duration::from_secs(60);
const MAX_TASK_INPUT_BYTES: usize = 64 * 1024;
const MAX_TASK_ID_LEN: usize = 128;

#[derive(NetworkBehaviour)]
struct Behaviour {
    rpc: request_response::json::Behaviour<RpcRequest, RpcResponse>,
    kad: libp2p::kad::Behaviour<MemoryStore>,
    relay_client: libp2p::relay::client::Behaviour,
    relay_server: Toggle<libp2p::relay::Behaviour>,
}

/// L1 transport configuration.
#[derive(Debug, Clone)]
pub struct TransportConfig {
    pub connectivity_policy: ConnectivityPolicy,
    /// Optional relay peer to reserve a circuit with.
    pub relay: Option<Multiaddr>,
    /// Whether this node also acts as a circuit relay server.
    pub relay_server: bool,
    /// SOCKS5 proxy used for Tor onion dialing.
    pub tor_proxy: Option<SocketAddrV6>,
    /// External onion endpoint advertised by this node.
    pub onion: Option<Multiaddr>,
}

impl Default for TransportConfig {
    fn default() -> Self {
        Self {
            connectivity_policy: ConnectivityPolicy::Auto,
            relay: None,
            relay_server: false,
            tor_proxy: None,
            onion: None,
        }
    }
}

/// L2 discovery configuration.
#[derive(Debug, Clone, Default)]
pub struct DiscoveryConfig {
    pub neighbor_policy: NeighborPolicy,
}

/// L3 session configuration.
#[derive(Debug, Clone)]
pub struct SessionConfig {
    pub idle_connection_timeout: Duration,
    pub dial_timeout: Duration,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            idle_connection_timeout: Duration::from_secs(60),
            dial_timeout: Duration::from_secs(5),
        }
    }
}

/// L4 agent protocol configuration.
#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub manifest_ttl: u64,
    pub default_capability: String,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            manifest_ttl: MANIFEST_TTL_SECONDS,
            default_capability: "task.echo".to_owned(),
        }
    }
}

/// Composition-root configuration.
#[derive(Debug, Clone, Default)]
pub struct NodeConfig {
    pub transport: TransportConfig,
    pub discovery: DiscoveryConfig,
    pub session: SessionConfig,
    pub agent: AgentConfig,
}

type PendingNeighbors = (
    u64,
    u32,
    Option<String>,
    oneshot::Sender<Result<Vec<ContactRecord>>>,
);

#[derive(Default)]
struct SourceStats {
    failures: u32,
    quarantined_until: Option<Instant>,
}

struct ActiveTask {
    events: mpsc::Sender<TaskEvent>,
    last_sequence: u64,
}

struct PendingAsk {
    task_id: String,
    events: mpsc::Sender<TaskEvent>,
    receiver: Option<mpsc::Receiver<TaskEvent>>,
    reply: oneshot::Sender<Result<TaskHandle>>,
}

struct IncomingTask {
    peer: PeerId,
    capability: String,
    input: Value,
    deadline_at: u64,
}

struct OutgoingTask {
    peer: PeerId,
    events: VecDeque<(u64, TaskEventKind)>,
    in_flight: Option<(OutboundRequestId, u64, TaskEventKind, u8)>,
}

#[derive(Default)]
struct AddressBook {
    endpoints: HashMap<String, EndpointRecord>,
}

impl AddressBook {
    fn upsert(&mut self, record: EndpointRecord) {
        self.endpoints.insert(record.addr.clone(), record);
    }

    fn records(&self) -> Vec<EndpointRecord> {
        let mut records: Vec<EndpointRecord> = self.endpoints.values().cloned().collect();
        records.sort_by(|left, right| left.addr.cmp(&right.addr));
        records
    }
}

#[derive(Default)]
struct NodeState {
    sources: HashMap<PeerId, SourceStats>,
    address_book: AddressBook,
    pending_dial_endpoints: HashMap<PeerId, String>,
    connectivity_policy: ConnectivityPolicy,
    relay_enabled: bool,
    relay_server: bool,
    tor_enabled: bool,
    relay_reservation: Option<PeerId>,
    pending_asks: HashMap<OutboundRequestId, PendingAsk>,
    active_tasks: HashMap<String, ActiveTask>,
    incoming_tasks: HashMap<String, IncomingTask>,
    outgoing_tasks: HashMap<String, OutgoingTask>,
    pending_event_sends: HashMap<OutboundRequestId, String>,
    manifests: HashMap<PeerId, Manifest>,
    pending_dials: HashMap<PeerId, oneshot::Sender<Result<PeerId>>>,
    pending_manifests: HashMap<OutboundRequestId, oneshot::Sender<Result<Manifest>>>,
    pending_echoes: HashMap<OutboundRequestId, oneshot::Sender<Result<String>>>,
    pending_neighbors: HashMap<OutboundRequestId, PendingNeighbors>,
    pending_dht: HashMap<QueryId, DhtPending>,
}

enum DhtPending {
    Publish {
        reply: oneshot::Sender<Result<()>>,
    },
    Lookup {
        agent_id: AgentId,
        found: Option<Box<Manifest>>,
        reply: oneshot::Sender<Result<Manifest>>,
    },
    Bootstrap {
        reply: oneshot::Sender<Result<()>>,
    },
    Provide {
        reply: oneshot::Sender<Result<()>>,
    },
    Providers {
        collected: HashSet<PeerId>,
        reply: oneshot::Sender<Result<Vec<PeerId>>>,
    },
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
    Neighbors {
        peer: PeerId,
        capability: Option<String>,
        max: u32,
        nonce: u64,
        reply: oneshot::Sender<Result<Vec<ContactRecord>>>,
    },
    Publish {
        reply: oneshot::Sender<Result<()>>,
    },
    Lookup {
        agent_id: AgentId,
        reply: oneshot::Sender<Result<Manifest>>,
    },
    AddAddress {
        peer: PeerId,
        addr: Multiaddr,
        reply: oneshot::Sender<Result<()>>,
    },
    Bootstrap {
        reply: oneshot::Sender<Result<()>>,
    },
    Provide {
        capability: String,
        reply: oneshot::Sender<Result<()>>,
    },
    Providers {
        capability: String,
        reply: oneshot::Sender<Result<Vec<PeerId>>>,
    },
    DialPeer {
        peer: PeerId,
        reply: oneshot::Sender<Result<PeerId>>,
    },
    Status {
        reply: oneshot::Sender<NetworkStatus>,
    },
    Ask {
        peer: PeerId,
        capability: String,
        input: Value,
        deadline_at: u64,
        reply: oneshot::Sender<Result<TaskHandle>>,
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

    pub async fn neighbors(
        &self,
        peer: PeerId,
        capability: Option<String>,
        max: u32,
    ) -> Result<Vec<ContactRecord>> {
        let nonce = now_unix();
        let (reply, receiver) = oneshot::channel();
        self.commands
            .send(Command::Neighbors {
                peer,
                capability,
                max,
                nonce,
                reply,
            })
            .await
            .map_err(|_| Error::Closed)?;
        receiver.await.map_err(|_| Error::Closed)?
    }

    /// Query multiple introducers and merge their validated contacts.
    ///
    /// A failed introducer is skipped rather than failing the whole discovery
    /// result. Records are de-duplicated by AgentID and capped globally.
    pub async fn discover(
        &self,
        introducers: &[PeerId],
        capability: Option<String>,
        per_introducer: u32,
        max_total: u32,
    ) -> Result<Vec<ContactRecord>> {
        let limit = max_total.clamp(1, 128) as usize;
        let mut seen = HashSet::new();
        let mut records = Vec::new();

        for introducer in introducers {
            if records.len() >= limit {
                break;
            }
            let Ok(batch) = self
                .neighbors(*introducer, capability.clone(), per_introducer)
                .await
            else {
                continue;
            };
            for record in batch {
                if records.len() >= limit {
                    break;
                }
                if seen.insert(record.manifest.body.agent_id.to_string()) {
                    records.push(record);
                }
            }
        }

        Ok(records)
    }

    pub async fn publish(&self) -> Result<()> {
        let (reply, receiver) = oneshot::channel();
        self.commands
            .send(Command::Publish { reply })
            .await
            .map_err(|_| Error::Closed)?;
        receiver.await.map_err(|_| Error::Closed)?
    }

    pub async fn lookup(&self, agent_id: AgentId) -> Result<Manifest> {
        let (reply, receiver) = oneshot::channel();
        self.commands
            .send(Command::Lookup { agent_id, reply })
            .await
            .map_err(|_| Error::Closed)?;
        receiver.await.map_err(|_| Error::Closed)?
    }

    pub async fn add_address(&self, peer: PeerId, addr: Multiaddr) -> Result<()> {
        let (reply, receiver) = oneshot::channel();
        self.commands
            .send(Command::AddAddress { peer, addr, reply })
            .await
            .map_err(|_| Error::Closed)?;
        receiver.await.map_err(|_| Error::Closed)?
    }

    pub async fn bootstrap(&self) -> Result<()> {
        let (reply, receiver) = oneshot::channel();
        self.commands
            .send(Command::Bootstrap { reply })
            .await
            .map_err(|_| Error::Closed)?;
        receiver.await.map_err(|_| Error::Closed)?
    }

    pub async fn provide(&self, capability: impl Into<String>) -> Result<()> {
        let (reply, receiver) = oneshot::channel();
        self.commands
            .send(Command::Provide {
                capability: capability.into(),
                reply,
            })
            .await
            .map_err(|_| Error::Closed)?;
        receiver.await.map_err(|_| Error::Closed)?
    }

    pub async fn providers(&self, capability: impl Into<String>) -> Result<Vec<PeerId>> {
        let (reply, receiver) = oneshot::channel();
        self.commands
            .send(Command::Providers {
                capability: capability.into(),
                reply,
            })
            .await
            .map_err(|_| Error::Closed)?;
        receiver.await.map_err(|_| Error::Closed)?
    }

    pub async fn network_status(&self) -> Result<NetworkStatus> {
        let (reply, receiver) = oneshot::channel();
        self.commands
            .send(Command::Status { reply })
            .await
            .map_err(|_| Error::Closed)?;
        receiver.await.map_err(|_| Error::Closed)
    }

    pub async fn dial_peer(&self, peer: PeerId) -> Result<PeerId> {
        let (reply, receiver) = oneshot::channel();
        self.commands
            .send(Command::DialPeer { peer, reply })
            .await
            .map_err(|_| Error::Closed)?;
        receiver.await.map_err(|_| Error::Closed)?
    }

    /// Start a task and return a handle that yields streaming events.
    pub async fn ask(
        &self,
        peer: PeerId,
        capability: impl Into<String>,
        input: Value,
        timeout: Duration,
    ) -> Result<TaskHandle> {
        let deadline_at = now_unix().saturating_add(timeout.as_secs().max(1));
        let (reply, receiver) = oneshot::channel();
        self.commands
            .send(Command::Ask {
                peer,
                capability: capability.into(),
                input,
                deadline_at,
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

/// Start a node with the default configuration.
pub async fn spawn(
    identity: &Identity,
    listen: Option<Multiaddr>,
) -> Result<(NodeHandle, Option<Multiaddr>)> {
    spawn_with_config(identity, listen, NodeConfig::default()).await
}

/// Start a node with an explicit configuration.
///
/// Returns the handle and the actual listen address (with `/p2p/<peer-id>`).
pub async fn spawn_with_config(
    identity: &Identity,
    listen: Option<Multiaddr>,
    config: NodeConfig,
) -> Result<(NodeHandle, Option<Multiaddr>)> {
    if let Some(listen) = &listen {
        ensure_ipv6_listen_addr(listen)?;
    }
    if let Some(relay) = &config.transport.relay {
        ensure_ipv6_peer_addr(relay)?;
    }
    match config.transport.connectivity_policy {
        ConnectivityPolicy::Privacy => {
            if config.transport.tor_proxy.is_none() {
                return Err(Error::TorUnavailable(
                    "privacy policy requires --tor-proxy; install/run the Tor daemon and provide its SOCKS5 proxy".to_owned(),
                ));
            }
            if let Some(onion) = &config.transport.onion {
                if !is_onion(onion) {
                    return Err(Error::Unsupported(
                        "--onion must be an /onion3/... address".to_owned(),
                    ));
                }
                if config.transport.tor_proxy.is_none() {
                    return Err(Error::TorUnavailable(
                        "--onion requires --tor-proxy (Tor plugin)".to_owned(),
                    ));
                }
            }
            if let Some(listen) = &listen {
                if classify_multiaddr(listen) != Reachability::Loopback {
                    return Err(Error::Unsupported(
                        "privacy listener must be loopback; publish a Tor onion service to it"
                            .to_owned(),
                    ));
                }
                if config.transport.onion.is_none() {
                    return Err(Error::Unsupported(
                        "privacy listener requires --onion".to_owned(),
                    ));
                }
            }
        }
        ConnectivityPolicy::Relay => {
            if config.transport.relay.is_none() && !config.transport.relay_server {
                return Err(Error::Unsupported(
                    "relay policy requires --relay or --relay-server".to_owned(),
                ));
            }
            if !config.transport.relay_server && listen.is_some() {
                return Err(Error::Unsupported(
                    "relay client policy must not open a direct listener".to_owned(),
                ));
            }
            if config.transport.relay_server && listen.is_none() {
                return Err(Error::Unsupported(
                    "relay server requires a direct listen address".to_owned(),
                ));
            }
        }
        ConnectivityPolicy::Direct => {
            if config.transport.relay.is_some() || config.transport.relay_server {
                return Err(Error::Unsupported(
                    "direct policy does not allow relay configuration".to_owned(),
                ));
            }
        }
        ConnectivityPolicy::Auto => {}
    }
    let mut seed = identity.to_seed();
    let keypair = Keypair::ed25519_from_bytes(&mut seed.expose()[..])
        .map_err(|error| Error::Identity(error.to_string()))?;
    let peer_id = keypair.public().to_peer_id();
    let onion = config
        .transport
        .onion
        .as_ref()
        .map(|address| ensure_peer_id(address.clone(), peer_id));
    let idle_timeout = config.session.idle_connection_timeout;
    let relay_server = if config.transport.relay_server {
        let mut behaviour =
            libp2p::relay::Behaviour::new(peer_id, libp2p::relay::Config::default());
        behaviour.set_status(Some(libp2p::relay::Status::Enable));
        Some(behaviour)
    } else {
        None
    };

    let tor_proxy = config.transport.tor_proxy;
    let mut swarm = SwarmBuilder::with_existing_identity(keypair)
        .with_tokio()
        .with_tcp(
            libp2p::tcp::Config::default(),
            libp2p::noise::Config::new,
            libp2p::yamux::Config::default,
        )
        .map_err(|error| Error::Transport(error.to_string()))?
        .with_other_transport(move |key| {
            let noise =
                libp2p::noise::Config::new(key).expect("generated ed25519 key is valid for Noise");
            TorTransport::new(tor_proxy)
                .upgrade(Version::V1Lazy)
                .authenticate(noise)
                .multiplex(libp2p::yamux::Config::default())
                .boxed()
        })
        .map_err(|error| Error::Transport(error.to_string()))?
        .with_relay_client(libp2p::noise::Config::new, libp2p::yamux::Config::default)
        .map_err(|error| Error::Transport(error.to_string()))?
        .with_behaviour(move |key, relay_client| {
            let peer_id = key.public().to_peer_id();
            let mut kad = libp2p::kad::Behaviour::new(peer_id, MemoryStore::new(peer_id));
            kad.set_mode(Some(libp2p::kad::Mode::Server));
            Behaviour {
                rpc: rpc_behaviour(),
                kad,
                relay_client,
                relay_server: Toggle::from(relay_server),
            }
        })
        .map_err(|error| Error::Behaviour(error.to_string()))?
        .with_swarm_config(|swarm_config| swarm_config.with_idle_connection_timeout(idle_timeout))
        .build();

    let relay_target = config.transport.relay.as_ref().map(|relay| {
        if relay
            .iter()
            .any(|protocol| protocol == Protocol::P2pCircuit)
        {
            relay.clone()
        } else {
            relay.clone().with(Protocol::P2pCircuit)
        }
    });
    if let Some(relay) = config.transport.relay.as_ref() {
        if let Some(relay_peer) = peer_from_multiaddr(relay) {
            if !swarm.is_connected(&relay_peer) {
                swarm
                    .dial(relay.clone())
                    .map_err(|error| Error::Dial(error.to_string()))?;
                tokio::time::timeout(
                    config.session.dial_timeout,
                    wait_for_connection(&mut swarm, relay_peer),
                )
                .await
                .map_err(|_| Error::Dial("timed out connecting to relay".to_owned()))??;
            }
        }
    }

    let mut listen_targets = Vec::new();
    if let Some(listen) = listen {
        listen_targets.push(listen);
    }
    if let Some(relay) = relay_target {
        listen_targets.push(relay);
    }
    for target in &listen_targets {
        swarm
            .listen_on(target.clone())
            .map_err(|error| Error::Listen(error.to_string()))?;
    }

    let listen_addr = if listen_targets.is_empty() {
        None
    } else {
        let address = tokio::time::timeout(Duration::from_secs(5), wait_for_listen(&mut swarm))
            .await
            .map_err(|_| Error::Listen("timed out waiting for a listen address".to_owned()))??;
        Some(ensure_peer_id(address, peer_id))
    };

    if config.transport.relay_server {
        if let Some(address) = &listen_addr {
            swarm.add_external_address(address.clone());
        }
    }
    if let Some(onion) = &onion {
        swarm.add_external_address(onion.clone());
    }

    let mut body = ManifestBody::new(identity.agent_id(), 1, MANIFEST_TTL_SECONDS);
    body.capabilities.push(Capability::new("task.echo"));
    if config.transport.connectivity_policy == ConnectivityPolicy::Privacy {
        if let Some(onion) = &onion {
            body.endpoints.push(onion.to_string());
        }
    } else if let Some(address) = &listen_addr {
        body.endpoints.push(address.to_string());
    }
    if let Some(onion) = &onion {
        if !body
            .endpoints
            .iter()
            .any(|endpoint| endpoint == &onion.to_string())
        {
            body.endpoints.push(onion.to_string());
        }
    }
    body.protocols.push(PROTOCOL.to_owned());
    let local_manifest = Manifest::sign(body, identity)?;

    let advertised_addr = if config.transport.connectivity_policy == ConnectivityPolicy::Privacy {
        onion.clone()
    } else {
        listen_addr.clone()
    };

    let (commands, receiver) = mpsc::channel(32);
    let task = tokio::spawn(run_node(
        swarm,
        local_manifest.clone(),
        identity.clone(),
        receiver,
        config,
    ));
    let handle = NodeHandle {
        peer_id,
        listen_addr: advertised_addr,
        agent_id: local_manifest.body.agent_id.clone(),
        commands,
        task,
    };

    Ok((handle, listen_addr))
}

fn rpc_behaviour() -> request_response::json::Behaviour<RpcRequest, RpcResponse> {
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

async fn wait_for_connection(swarm: &mut Swarm<Behaviour>, peer: PeerId) -> Result<()> {
    loop {
        match swarm.select_next_some().await {
            SwarmEvent::ConnectionEstablished { peer_id, .. } if peer_id == peer => return Ok(()),
            SwarmEvent::OutgoingConnectionError {
                peer_id: Some(failed),
                error,
                ..
            } if failed == peer => return Err(Error::Dial(error.to_string())),
            _ => {}
        }
    }
}

async fn run_node(
    mut swarm: Swarm<Behaviour>,
    mut local_manifest: Manifest,
    identity: Identity,
    mut commands: mpsc::Receiver<Command>,
    config: NodeConfig,
) {
    let mut state = NodeState {
        connectivity_policy: config.transport.connectivity_policy,
        relay_enabled: config.transport.relay.is_some(),
        relay_server: config.transport.relay_server,
        tor_enabled: config.transport.tor_proxy.is_some(),
        relay_reservation: config
            .transport
            .relay
            .as_ref()
            .and_then(peer_from_multiaddr),
        ..Default::default()
    };

    loop {
        tokio::select! {
            event = swarm.select_next_some() => {
                handle_event(
                    &mut swarm,
                    event,
                    &mut local_manifest,
                    &identity,
                    &mut state,
                    &config,
                );
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
    local_manifest: &mut Manifest,
    identity: &Identity,
    state: &mut NodeState,
    config: &NodeConfig,
) {
    match event {
        SwarmEvent::ConnectionEstablished { peer_id, .. } => {
            if let Some(addr) = state.pending_dial_endpoints.remove(&peer_id) {
                if let Some(mut record) = state.address_book.endpoints.get(&addr).cloned() {
                    record.verified = true;
                    if record.reachability == Reachability::PublicUnverified {
                        record.reachability = Reachability::PublicVerified;
                    }
                    state.address_book.upsert(record);
                }
            }
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
        SwarmEvent::NewListenAddr { address, .. } => {
            let address = ensure_peer_id(address, *swarm.local_peer_id());
            state
                .address_book
                .upsert(endpoint_record(&address, "listen", false));
            if state.connectivity_policy != ConnectivityPolicy::Privacy || is_onion(&address) {
                add_manifest_endpoint(local_manifest, identity, &address);
            }
        }
        SwarmEvent::ExternalAddrConfirmed { address } => {
            state
                .address_book
                .upsert(endpoint_record(&address, "external", false));
            if state.connectivity_policy != ConnectivityPolicy::Privacy || is_onion(&address) {
                add_manifest_endpoint(local_manifest, identity, &address);
            }
        }
        SwarmEvent::Behaviour(event) => match event {
            BehaviourEvent::Rpc(event) => {
                handle_rpc_event(swarm, event, local_manifest, state, config)
            }
            BehaviourEvent::Kad(event) => handle_kad_event(event, state),
            BehaviourEvent::RelayClient(event) => {
                if let libp2p::relay::client::Event::ReservationReqAccepted {
                    relay_peer_id, ..
                } = event
                {
                    state.relay_reservation = Some(relay_peer_id);
                }
            }
            BehaviourEvent::RelayServer(_) => {}
        },
        _ => {}
    }
}

fn add_manifest_endpoint(manifest: &mut Manifest, identity: &Identity, address: &Multiaddr) {
    let value = address.to_string();
    if manifest
        .body
        .endpoints
        .iter()
        .any(|endpoint| endpoint == &value)
    {
        return;
    }

    let mut body = manifest.body.clone();
    body.endpoints.push(value);
    body.seq = body.seq.saturating_add(1);
    body.issued_at = now_unix();
    body.expires_at = body.issued_at.saturating_add(MANIFEST_TTL_SECONDS);
    if let Ok(signed) = Manifest::sign(body, identity) {
        *manifest = signed;
    }
}

fn handle_rpc_event(
    swarm: &mut Swarm<Behaviour>,
    event: request_response::Event<RpcRequest, RpcResponse>,
    local_manifest: &Manifest,
    state: &mut NodeState,
    config: &NodeConfig,
) {
    match event {
        request_response::Event::Message { peer, message, .. } => match message {
            Message::Request {
                request, channel, ..
            } => respond_to_request(swarm, peer, request, channel, local_manifest, state, config),
            Message::Response {
                request_id,
                response,
            } => handle_response(swarm, peer, request_id, response, state),
        },
        request_response::Event::OutboundFailure {
            peer,
            request_id,
            error,
            ..
        } => {
            let message = format!("request failed: {error}");
            if let Some(reply) = state.pending_manifests.remove(&request_id) {
                let _ = reply.send(Err(Error::Request(message.clone())));
            }
            if let Some(reply) = state.pending_echoes.remove(&request_id) {
                let _ = reply.send(Err(Error::Request(message.clone())));
            }
            if let Some((_, _, _, reply)) = state.pending_neighbors.remove(&request_id) {
                record_source_failure(state, &peer);
                let _ = reply.send(Err(Error::Request(message)));
            }
            if let Some(task_id) = state.pending_event_sends.remove(&request_id) {
                retry_or_advance_task_event(swarm, state, &task_id, true);
            }
        }
        _ => {}
    }
}

fn task_error(message: impl Into<String>) -> RpcResponse {
    RpcResponse::Error {
        message: message.into(),
    }
}

fn handle_task_start(
    peer: PeerId,
    task_id: String,
    capability: String,
    input: Value,
    deadline_at: u64,
    local_manifest: &Manifest,
    state: &mut NodeState,
) -> RpcResponse {
    if let Err(error) = ensure_operation_allowed(
        state.connectivity_policy,
        state.relay_enabled,
        state.relay_server,
        state.tor_enabled,
    ) {
        return task_error(error.to_string());
    }
    if !state.manifests.contains_key(&peer) {
        return task_error("manifest exchange required");
    }
    if task_id.is_empty() || task_id.len() > MAX_TASK_ID_LEN {
        return task_error("invalid task id");
    }
    if state.incoming_tasks.contains_key(&task_id) {
        return task_error("task id already active");
    }
    if deadline_at <= now_unix() {
        return task_error("task deadline has passed");
    }
    if !local_manifest
        .body
        .capabilities
        .iter()
        .any(|advertised| advertised.id == capability)
    {
        return task_error("capability not advertised");
    }
    let input_size = serde_json::to_vec(&input)
        .map(|bytes| bytes.len())
        .unwrap_or(usize::MAX);
    if input_size > MAX_TASK_INPUT_BYTES {
        return task_error("task input too large");
    }

    state.incoming_tasks.insert(
        task_id.clone(),
        IncomingTask {
            peer,
            capability,
            input,
            deadline_at,
        },
    );
    RpcResponse::TaskAccepted { task_id }
}

fn handle_task_ready(
    peer: PeerId,
    task_id: String,
    state: &mut NodeState,
) -> (RpcResponse, Vec<(u64, TaskEventKind)>) {
    let Some(task) = state.incoming_tasks.remove(&task_id) else {
        return (task_error("unknown task"), Vec::new());
    };
    if task.peer != peer {
        return (task_error("task peer mismatch"), Vec::new());
    }
    if task.deadline_at <= now_unix() {
        return (
            RpcResponse::TaskReadyAck { task_id },
            vec![
                (
                    1,
                    TaskEventKind::Failed {
                        message: "task deadline expired".to_owned(),
                    },
                ),
                (2, TaskEventKind::Done),
            ],
        );
    }

    let events = match task.capability.as_str() {
        "task.echo" => vec![
            (
                1,
                TaskEventKind::Started {
                    capability: task.capability.clone(),
                },
            ),
            (
                2,
                TaskEventKind::Progress {
                    message: "processing".to_owned(),
                    fraction: Some(0.5),
                },
            ),
            (
                3,
                TaskEventKind::Result {
                    output: serde_json::json!({ "echo": task.input }),
                },
            ),
            (4, TaskEventKind::Done),
        ],
        _ => vec![
            (
                1,
                TaskEventKind::Failed {
                    message: "no local handler for capability".to_owned(),
                },
            ),
            (2, TaskEventKind::Done),
        ],
    };

    (RpcResponse::TaskReadyAck { task_id }, events)
}

fn handle_task_event(
    state: &mut NodeState,
    task_id: String,
    sequence: u64,
    event: TaskEventKind,
) -> RpcResponse {
    if !state.active_tasks.contains_key(&task_id) {
        return task_error("unknown task");
    }

    let terminal = matches!(&event, TaskEventKind::Done);
    if let Some(active) = state.active_tasks.get_mut(&task_id) {
        if sequence <= active.last_sequence {
            return RpcResponse::TaskEventAck { task_id, sequence };
        }
        active.last_sequence = sequence;
        let _ = active.events.try_send(TaskEvent {
            task_id: task_id.clone(),
            sequence,
            event,
        });
    }
    if terminal {
        state.active_tasks.remove(&task_id);
    }

    RpcResponse::TaskEventAck { task_id, sequence }
}

fn send_next_task_event(swarm: &mut Swarm<Behaviour>, state: &mut NodeState, task_id: &str) {
    let (peer, sequence, event) = {
        let Some(task) = state.outgoing_tasks.get_mut(task_id) else {
            return;
        };
        if task.in_flight.is_some() {
            return;
        }
        let Some((sequence, event)) = task.events.pop_front() else {
            state.outgoing_tasks.remove(task_id);
            return;
        };
        (task.peer, sequence, event)
    };

    let request_id = swarm.behaviour_mut().rpc.send_request(
        &peer,
        RpcRequest::TaskEventPush {
            task_id: task_id.to_owned(),
            sequence,
            event: event.clone(),
        },
    );
    state
        .pending_event_sends
        .insert(request_id, task_id.to_owned());
    if let Some(task) = state.outgoing_tasks.get_mut(task_id) {
        task.in_flight = Some((request_id, sequence, event, 0));
    }
}

fn retry_or_advance_task_event(
    swarm: &mut Swarm<Behaviour>,
    state: &mut NodeState,
    task_id: &str,
    failed: bool,
) {
    let retry = {
        let Some(task) = state.outgoing_tasks.get_mut(task_id) else {
            return;
        };
        let Some((_, sequence, event, attempts)) = task.in_flight.take() else {
            return;
        };
        if failed && attempts < 3 {
            Some((task.peer, sequence, event, attempts + 1))
        } else {
            None
        }
    };

    if let Some((peer, sequence, event, attempts)) = retry {
        let request_id = swarm.behaviour_mut().rpc.send_request(
            &peer,
            RpcRequest::TaskEventPush {
                task_id: task_id.to_owned(),
                sequence,
                event: event.clone(),
            },
        );
        state
            .pending_event_sends
            .insert(request_id, task_id.to_owned());
        if let Some(task) = state.outgoing_tasks.get_mut(task_id) {
            task.in_flight = Some((request_id, sequence, event, attempts));
        }
        return;
    }

    let empty = state
        .outgoing_tasks
        .get(task_id)
        .map(|task| task.events.is_empty())
        .unwrap_or(true);
    if empty {
        state.outgoing_tasks.remove(task_id);
    } else {
        send_next_task_event(swarm, state, task_id);
    }
}

fn new_task_id() -> String {
    let mut bytes = [0u8; 16];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn respond_to_request(
    swarm: &mut Swarm<Behaviour>,
    peer: PeerId,
    request: RpcRequest,
    channel: ResponseChannel<RpcResponse>,
    local_manifest: &Manifest,
    state: &mut NodeState,
    config: &NodeConfig,
) {
    let mut followup: Vec<(PeerId, String, u64, TaskEventKind)> = Vec::new();
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
        RpcRequest::Neighbors {
            capability,
            max,
            nonce,
        } => {
            let records = answer_neighbors(
                state,
                &peer,
                capability.as_deref(),
                max,
                config.discovery.neighbor_policy,
            );
            RpcResponse::Neighbors {
                records,
                nonce,
                policy: config.discovery.neighbor_policy.as_str().to_owned(),
            }
        }
        RpcRequest::TaskStart {
            task_id,
            capability,
            input,
            deadline_at,
        } => handle_task_start(
            peer,
            task_id,
            capability,
            input,
            deadline_at,
            local_manifest,
            state,
        ),
        RpcRequest::TaskReady { task_id } => {
            let events_task_id = task_id.clone();
            let (response, events) = handle_task_ready(peer, task_id, state);
            for (sequence, event) in events {
                followup.push((peer, events_task_id.clone(), sequence, event));
            }
            response
        }
        RpcRequest::TaskEventPush {
            task_id,
            sequence,
            event,
        } => handle_task_event(state, task_id, sequence, event),
    };

    let _ = swarm.behaviour_mut().rpc.send_response(channel, response);
    if let Some((peer, task_id, _, _)) = followup.first() {
        let peer = *peer;
        let task_id = task_id.clone();
        let events = followup
            .into_iter()
            .map(|(_, _, sequence, event)| (sequence, event))
            .collect();
        state.outgoing_tasks.insert(
            task_id.clone(),
            OutgoingTask {
                peer,
                events,
                in_flight: None,
            },
        );
        send_next_task_event(swarm, state, &task_id);
    }
}

fn handle_response(
    swarm: &mut Swarm<Behaviour>,
    peer: PeerId,
    request_id: OutboundRequestId,
    response: RpcResponse,
    state: &mut NodeState,
) {
    if let Some(task_id) = state.pending_event_sends.remove(&request_id) {
        retry_or_advance_task_event(swarm, state, &task_id, false);
        return;
    }

    if let Some(pending) = state.pending_asks.remove(&request_id) {
        let PendingAsk {
            task_id: expected_task_id,
            events,
            receiver,
            reply,
        } = pending;

        let result = match response {
            RpcResponse::TaskAccepted { task_id } if task_id == expected_task_id => {
                let Some(receiver) = receiver else {
                    let _ = reply.send(Err(Error::Request(
                        "task handle receiver already consumed".to_owned(),
                    )));
                    return;
                };
                state.active_tasks.insert(
                    task_id.clone(),
                    ActiveTask {
                        events,
                        last_sequence: 0,
                    },
                );
                let _ = swarm.behaviour_mut().rpc.send_request(
                    &peer,
                    RpcRequest::TaskReady {
                        task_id: task_id.clone(),
                    },
                );
                Ok(TaskHandle::from_receiver(task_id, receiver))
            }
            RpcResponse::Error { message } => Err(Error::Request(message)),
            _ => Err(Error::Request("unexpected task response".to_owned())),
        };
        let _ = reply.send(result);
        return;
    }

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
            RpcResponse::Echo { .. }
            | RpcResponse::Neighbors { .. }
            | RpcResponse::TaskAccepted { .. }
            | RpcResponse::TaskReadyAck { .. }
            | RpcResponse::TaskEventAck { .. } => {
                Err(Error::Request("unexpected manifest response".to_owned()))
            }
        };
        let _ = reply.send(result);
        return;
    }

    if let Some(reply) = state.pending_echoes.remove(&request_id) {
        let result = match response {
            RpcResponse::Echo { message } => Ok(message),
            RpcResponse::Error { message } => Err(Error::Request(message)),
            RpcResponse::Manifest(_)
            | RpcResponse::Neighbors { .. }
            | RpcResponse::TaskAccepted { .. }
            | RpcResponse::TaskReadyAck { .. }
            | RpcResponse::TaskEventAck { .. } => {
                Err(Error::Request("unexpected echo response".to_owned()))
            }
        };
        let _ = reply.send(result);
        return;
    }

    if let Some((expected_nonce, max, capability, reply)) =
        state.pending_neighbors.remove(&request_id)
    {
        let result = match response {
            RpcResponse::Neighbors { records, nonce, .. } if nonce == expected_nonce => {
                let total = records.len();
                let valid = validate_contacts(records, capability.as_deref(), max);
                if valid.len() == total {
                    record_source_success(state, &peer);
                } else {
                    record_source_failure(state, &peer);
                }
                Ok(valid)
            }
            RpcResponse::Neighbors { .. } => {
                record_source_failure(state, &peer);
                Err(Error::Request(
                    "neighbor response nonce mismatch".to_owned(),
                ))
            }
            RpcResponse::Error { message } => {
                record_source_failure(state, &peer);
                Err(Error::Request(message))
            }
            _ => {
                record_source_failure(state, &peer);
                Err(Error::Request("unexpected neighbor response".to_owned()))
            }
        };
        let _ = reply.send(result);
    }
}

fn transport_statuses(state: &NodeState) -> Vec<TransportStatus> {
    vec![
        TransportStatus {
            transport: TransportKind::Tcp,
            available: true,
            note: "IPv6 TCP + Noise".to_owned(),
        },
        TransportStatus {
            transport: TransportKind::Relay,
            available: state.relay_enabled || state.relay_server,
            note: if state.relay_server {
                "circuit relay v2 server + client".to_owned()
            } else if state.relay_enabled {
                "circuit relay v2 client".to_owned()
            } else {
                "pass --relay or --relay-server to enable".to_owned()
            },
        },
        TransportStatus {
            transport: TransportKind::Tor,
            available: state.tor_enabled,
            note: if state.tor_enabled {
                "SOCKS5 onion dialing".to_owned()
            } else {
                "pass --tor-proxy to enable".to_owned()
            },
        },
    ]
}

fn build_network_status(
    peer_id: &PeerId,
    local_manifest: &Manifest,
    state: &NodeState,
) -> NetworkStatus {
    let listen = local_manifest.body.endpoints.first().cloned();
    let reachability = listen
        .as_deref()
        .and_then(|value| value.parse::<Multiaddr>().ok())
        .map(|addr| classify_multiaddr(&addr))
        .unwrap_or(Reachability::Unknown);

    let mut endpoints = state.address_book.records();
    if let Some(addr) = &listen {
        if !endpoints.iter().any(|record| &record.addr == addr) {
            if let Ok(parsed) = addr.parse::<Multiaddr>() {
                endpoints.push(endpoint_record(&parsed, "listen", false));
            }
        }
    }

    NetworkStatus {
        agent_id: local_manifest.body.agent_id.to_string(),
        peer_id: peer_id.to_string(),
        listen,
        policy: state.connectivity_policy,
        reachability,
        transports: transport_statuses(state),
        endpoints,
        relay_reservation: state.relay_reservation.map(|peer| peer.to_string()),
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
            if let Err(error) = ensure_dial_allowed(
                state.connectivity_policy,
                state.relay_enabled,
                state.relay_server,
                state.tor_enabled,
                &addr,
            ) {
                let _ = reply.send(Err(error));
                return true;
            }
            let peer = match peer_from_multiaddr(&addr) {
                Some(peer) => peer,
                None => {
                    let _ = reply.send(Err(Error::Dial(
                        "multiaddr must include /p2p/<peer-id>".to_owned(),
                    )));
                    return true;
                }
            };

            let endpoint = endpoint_record(&addr, "dial", false);
            if swarm.is_connected(&peer) {
                let mut verified = endpoint;
                verified.verified = true;
                state.address_book.upsert(verified);
                let _ = reply.send(Ok(peer));
                return true;
            }

            match swarm.dial(addr) {
                Ok(()) => {
                    state
                        .pending_dial_endpoints
                        .insert(peer, endpoint.addr.clone());
                    state.address_book.upsert(endpoint);
                    state.pending_dials.insert(peer, reply);
                }
                Err(error) => {
                    let _ = reply.send(Err(Error::Dial(error.to_string())));
                }
            }
            true
        }
        Command::Manifest { peer, reply } => {
            if let Err(error) = ensure_operation_allowed(
                state.connectivity_policy,
                state.relay_enabled,
                state.relay_server,
                state.tor_enabled,
            ) {
                let _ = reply.send(Err(error));
                return true;
            }
            let request_id = swarm.behaviour_mut().rpc.send_request(
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
            if let Err(error) = ensure_operation_allowed(
                state.connectivity_policy,
                state.relay_enabled,
                state.relay_server,
                state.tor_enabled,
            ) {
                let _ = reply.send(Err(error));
                return true;
            }
            let request_id = swarm
                .behaviour_mut()
                .rpc
                .send_request(&peer, RpcRequest::Echo { message });
            state.pending_echoes.insert(request_id, reply);
            true
        }
        Command::Neighbors {
            peer,
            capability,
            max,
            nonce,
            reply,
        } => {
            if let Err(error) = ensure_operation_allowed(
                state.connectivity_policy,
                state.relay_enabled,
                state.relay_server,
                state.tor_enabled,
            ) {
                let _ = reply.send(Err(error));
                return true;
            }
            if source_is_quarantined(state, &peer) {
                let _ = reply.send(Err(Error::Request("introducer is quarantined".to_owned())));
                return true;
            }
            let request_id = swarm.behaviour_mut().rpc.send_request(
                &peer,
                RpcRequest::Neighbors {
                    capability: capability.clone(),
                    max,
                    nonce,
                },
            );
            state
                .pending_neighbors
                .insert(request_id, (nonce, max, capability, reply));
            true
        }
        Command::Publish { reply } => {
            let bytes = match serde_json::to_vec(local_manifest) {
                Ok(bytes) => bytes,
                Err(error) => {
                    let _ = reply.send(Err(Error::Serialization(error.to_string())));
                    return true;
                }
            };
            let mut record = Record::new(manifest_key(&local_manifest.body.agent_id), bytes);
            record.expires = Some(Instant::now() + Duration::from_secs(MANIFEST_TTL_SECONDS));
            let query = match swarm.behaviour_mut().kad.put_record(record, Quorum::One) {
                Ok(query) => query,
                Err(error) => {
                    let _ = reply.send(Err(Error::Dht(error.to_string())));
                    return true;
                }
            };
            state
                .pending_dht
                .insert(query, DhtPending::Publish { reply });
            true
        }
        Command::Lookup { agent_id, reply } => {
            let query = swarm
                .behaviour_mut()
                .kad
                .get_record(manifest_key(&agent_id));
            state.pending_dht.insert(
                query,
                DhtPending::Lookup {
                    agent_id,
                    found: None,
                    reply,
                },
            );
            true
        }
        Command::AddAddress { peer, addr, reply } => {
            state
                .address_book
                .upsert(endpoint_record(&addr, "manual", false));
            let _ = swarm.behaviour_mut().kad.add_address(&peer, addr);
            let _ = reply.send(Ok(()));
            true
        }
        Command::Bootstrap { reply } => {
            let query = match swarm.behaviour_mut().kad.bootstrap() {
                Ok(query) => query,
                Err(error) => {
                    let _ = reply.send(Err(Error::Dht(error.to_string())));
                    return true;
                }
            };
            state
                .pending_dht
                .insert(query, DhtPending::Bootstrap { reply });
            true
        }
        Command::Provide { capability, reply } => {
            let query = match swarm
                .behaviour_mut()
                .kad
                .start_providing(capability_key(&capability))
            {
                Ok(query) => query,
                Err(error) => {
                    let _ = reply.send(Err(Error::Dht(error.to_string())));
                    return true;
                }
            };
            state
                .pending_dht
                .insert(query, DhtPending::Provide { reply });
            true
        }
        Command::Providers { capability, reply } => {
            let query = swarm
                .behaviour_mut()
                .kad
                .get_providers(capability_key(&capability));
            state.pending_dht.insert(
                query,
                DhtPending::Providers {
                    collected: HashSet::new(),
                    reply,
                },
            );
            true
        }
        Command::DialPeer { peer, reply } => {
            if let Err(error) = ensure_operation_allowed(
                state.connectivity_policy,
                state.relay_enabled,
                state.relay_server,
                state.tor_enabled,
            ) {
                let _ = reply.send(Err(error));
                return true;
            }
            if swarm.is_connected(&peer) {
                let _ = reply.send(Ok(peer));
                return true;
            }
            match swarm.dial(peer) {
                Ok(()) => {
                    state.pending_dials.insert(peer, reply);
                }
                Err(error) => {
                    let _ = reply.send(Err(Error::Dial(error.to_string())));
                }
            }
            true
        }
        Command::Ask {
            peer,
            capability,
            input,
            deadline_at,
            reply,
        } => {
            if let Err(error) = ensure_operation_allowed(
                state.connectivity_policy,
                state.relay_enabled,
                state.relay_server,
                state.tor_enabled,
            ) {
                let _ = reply.send(Err(error));
                return true;
            }
            let (events, receiver) = mpsc::channel(32);
            let task_id = new_task_id();
            let request_id = swarm.behaviour_mut().rpc.send_request(
                &peer,
                RpcRequest::TaskStart {
                    task_id: task_id.clone(),
                    capability,
                    input,
                    deadline_at,
                },
            );
            state.pending_asks.insert(
                request_id,
                PendingAsk {
                    task_id,
                    events,
                    receiver: Some(receiver),
                    reply,
                },
            );
            true
        }
        Command::Status { reply } => {
            let status = build_network_status(swarm.local_peer_id(), local_manifest, state);
            let _ = reply.send(status);
            true
        }
        Command::Shutdown => false,
    }
}

fn handle_kad_event(event: kad::Event, state: &mut NodeState) {
    let kad::Event::OutboundQueryProgressed {
        id, result, step, ..
    } = event
    else {
        return;
    };

    if let Some(DhtPending::Lookup {
        agent_id, found, ..
    }) = state.pending_dht.get_mut(&id)
    {
        update_found(found, agent_id, &result);
    }

    if let Some(DhtPending::Providers { collected, .. }) = state.pending_dht.get_mut(&id) {
        if let QueryResult::GetProviders(Ok(GetProvidersOk::FoundProviders { providers, .. })) =
            &result
        {
            collected.extend(providers.iter().cloned());
        }
    }

    if !step.last {
        return;
    }

    let Some(pending) = state.pending_dht.remove(&id) else {
        return;
    };

    match pending {
        DhtPending::Publish { reply } => {
            let outcome = match result {
                QueryResult::PutRecord(Ok(_)) => Ok(()),
                QueryResult::PutRecord(Err(error)) => Err(Error::Dht(error.to_string())),
                other => Err(Error::Dht(format!("unexpected DHT result: {other:?}"))),
            };
            let _ = reply.send(outcome);
        }
        DhtPending::Lookup { found, reply, .. } => {
            let outcome = match found {
                Some(manifest) => Ok(*manifest),
                None => Err(Error::Dht("manifest not found in DHT".to_owned())),
            };
            let _ = reply.send(outcome);
        }
        DhtPending::Bootstrap { reply } => {
            let outcome = match result {
                QueryResult::Bootstrap(Ok(_)) => Ok(()),
                QueryResult::Bootstrap(Err(error)) => Err(Error::Dht(error.to_string())),
                other => Err(Error::Dht(format!("unexpected DHT result: {other:?}"))),
            };
            let _ = reply.send(outcome);
        }
        DhtPending::Provide { reply } => {
            let outcome = match result {
                QueryResult::StartProviding(Ok(_)) => Ok(()),
                QueryResult::StartProviding(Err(error)) => Err(Error::Dht(error.to_string())),
                other => Err(Error::Dht(format!("unexpected DHT result: {other:?}"))),
            };
            let _ = reply.send(outcome);
        }
        DhtPending::Providers { collected, reply } => {
            let mut peers: Vec<PeerId> = collected.into_iter().collect();
            peers.sort_by_key(|peer| peer.to_string());
            let _ = reply.send(Ok(peers));
        }
    }
}

fn update_found(found: &mut Option<Box<Manifest>>, agent_id: &AgentId, result: &QueryResult) {
    if found.is_some() {
        return;
    }
    let QueryResult::GetRecord(Ok(GetRecordOk::FoundRecord(peer_record))) = result else {
        return;
    };
    let Ok(manifest) = serde_json::from_slice::<Manifest>(&peer_record.record.value) else {
        return;
    };
    if &manifest.body.agent_id == agent_id && manifest.verify().is_ok() {
        *found = Some(Box::new(manifest));
    }
}

fn record_source_success(state: &mut NodeState, peer: &PeerId) {
    let stats = state.sources.entry(*peer).or_default();
    stats.failures = 0;
    stats.quarantined_until = None;
}

fn record_source_failure(state: &mut NodeState, peer: &PeerId) {
    let stats = state.sources.entry(*peer).or_default();
    stats.failures = stats.failures.saturating_add(1);
    if stats.failures >= QUARANTINE_FAILURES {
        stats.quarantined_until = Some(Instant::now() + QUARANTINE_DURATION);
    }
}

fn source_is_quarantined(state: &NodeState, peer: &PeerId) -> bool {
    state
        .sources
        .get(peer)
        .and_then(|stats| stats.quarantined_until)
        .is_some_and(|until| until > Instant::now())
}

fn answer_neighbors(
    state: &NodeState,
    requester: &PeerId,
    capability: Option<&str>,
    max: u32,
    policy: NeighborPolicy,
) -> Vec<ContactRecord> {
    if policy == NeighborPolicy::Stealth {
        return Vec::new();
    }

    let limit = max.clamp(1, 32) as usize;
    let observed_at = now_unix();
    state
        .manifests
        .iter()
        .filter(|(peer, _)| *peer != requester)
        .filter(|(_, manifest)| {
            capability.is_none_or(|cap| {
                manifest
                    .body
                    .capabilities
                    .iter()
                    .any(|capability| capability.id == cap)
            })
        })
        .take(limit)
        .map(|(_, manifest)| ContactRecord {
            manifest: manifest.clone(),
            observed_at,
        })
        .collect()
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn verify_manifest_for_peer(manifest: &Manifest, peer: &PeerId) -> Result<()> {
    manifest.verify()?;
    for endpoint in &manifest.body.endpoints {
        let address = endpoint
            .parse::<Multiaddr>()
            .map_err(|error| Error::Unsupported(format!("invalid manifest endpoint: {error}")))?;
        ensure_ipv6_peer_addr(&address)?;
    }
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
    address
        .iter()
        .filter_map(|protocol| match protocol {
            Protocol::P2p(peer) => Some(peer),
            _ => None,
        })
        .last()
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

    fn signed_manifest(identity: &Identity, capability: &str) -> Manifest {
        let mut body = ManifestBody::new(identity.agent_id(), 1, MANIFEST_TTL_SECONDS);
        body.capabilities.push(Capability::new(capability));
        Manifest::sign(body, identity).unwrap()
    }

    fn contact(manifest: Manifest) -> ContactRecord {
        ContactRecord {
            manifest,
            observed_at: 0,
        }
    }

    #[test]
    fn filters_forged_expired_and_wrong_capability() {
        let identity = Identity::generate();
        let valid = contact(signed_manifest(&identity, "task.echo"));

        let mut forged_manifest = signed_manifest(&identity, "task.echo");
        forged_manifest.body.capabilities[0].id = "task.other".to_owned();
        let forged = contact(forged_manifest);

        let expired_identity = Identity::generate();
        let mut expired_body =
            ManifestBody::new(expired_identity.agent_id(), 1, MANIFEST_TTL_SECONDS);
        expired_body.issued_at = 1;
        expired_body.expires_at = 2;
        expired_body.capabilities.push(Capability::new("task.echo"));
        let expired = contact(Manifest::sign(expired_body, &expired_identity).unwrap());

        let wrong_identity = Identity::generate();
        let wrong_capability = contact(signed_manifest(&wrong_identity, "task.other"));

        let filtered = validate_contacts(
            vec![valid, forged, expired, wrong_capability],
            Some("task.echo"),
            8,
        );

        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].manifest.body.agent_id, identity.agent_id());
    }

    #[test]
    fn deduplicates_contacts() {
        let identity = Identity::generate();
        let duplicate = contact(signed_manifest(&identity, "task.echo"));

        let filtered = validate_contacts(vec![duplicate.clone(), duplicate], Some("task.echo"), 8);

        assert_eq!(filtered.len(), 1);
    }

    #[test]
    fn classifies_ipv6_endpoints() {
        assert_eq!(
            classify_multiaddr(&"/ip6/::1/tcp/4001".parse().unwrap()),
            Reachability::Loopback
        );
        assert_eq!(
            classify_multiaddr(&"/ip6/fc00::1/tcp/4001".parse().unwrap()),
            Reachability::Private
        );
        assert_eq!(
            classify_multiaddr(&"/ip6/fe80::1/tcp/4001".parse().unwrap()),
            Reachability::LinkLocal
        );
        assert_eq!(
            classify_multiaddr(&"/ip6/2001:db8::1/tcp/4001".parse().unwrap()),
            Reachability::Unknown
        );
        assert_eq!(
            classify_multiaddr(&"/ip6/2606:4700::1/tcp/4001".parse().unwrap()),
            Reachability::PublicUnverified
        );
    }

    #[test]
    fn rejects_ipv4_and_unspecified_listen_addresses() {
        assert!(ensure_ipv6_listen_addr(&"/ip4/127.0.0.1/tcp/4001".parse().unwrap()).is_err());
        assert!(ensure_ipv6_listen_addr(&"/ip6/::/tcp/4001".parse().unwrap()).is_err());
        assert!(ensure_ipv6_peer_addr(&"/ip4/127.0.0.1/tcp/4001".parse().unwrap()).is_err());
        assert!(ensure_ipv6_peer_addr(&"/ip6/::/tcp/4001".parse().unwrap()).is_err());
        assert!(ensure_ipv6_listen_addr(&"/ip6/::1/tcp/4001".parse().unwrap()).is_ok());
        assert!(ensure_ipv6_peer_addr(&"/ip6/2606:4700::1/tcp/4001".parse().unwrap()).is_ok());
    }

    #[test]
    fn privacy_and_relay_policies_fail_closed() {
        assert!(ensure_operation_allowed(ConnectivityPolicy::Auto, false, false, false).is_ok());
        assert!(ensure_operation_allowed(ConnectivityPolicy::Direct, false, false, false).is_ok());
        assert!(ensure_operation_allowed(ConnectivityPolicy::Relay, false, false, false).is_err());
        assert!(ensure_operation_allowed(ConnectivityPolicy::Relay, true, false, false).is_ok());
        assert!(ensure_operation_allowed(ConnectivityPolicy::Privacy, true, true, false).is_err());
        assert!(ensure_operation_allowed(ConnectivityPolicy::Privacy, false, false, true).is_ok());
    }

    #[test]
    fn quarantines_repeated_source_failures() {
        let mut state = NodeState::default();
        let peer = peer_id_for_agent_id(&Identity::generate().agent_id()).unwrap();

        for _ in 0..QUARANTINE_FAILURES {
            record_source_failure(&mut state, &peer);
        }
        assert!(source_is_quarantined(&state, &peer));

        record_source_success(&mut state, &peer);
        assert!(!source_is_quarantined(&state, &peer));
    }

    #[test]
    fn respects_the_requested_record_limit() {
        let mut records = Vec::new();
        for _ in 0..3 {
            records.push(contact(signed_manifest(&Identity::generate(), "task.echo")));
        }

        assert_eq!(validate_contacts(records, Some("task.echo"), 2).len(), 2);
    }
}
