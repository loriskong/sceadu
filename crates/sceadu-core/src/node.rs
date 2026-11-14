//! libp2p transport and session layer for Sceadu.
//!
//! Every node publishes a signed Biography to its configured bootstrap peers,
//! stores verified biographies locally, and exchanges messages directly.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddrV6;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::agent::{AgentMessage, FileRef, Inbox};
use crate::biography::BiographyStore;
use crate::identity::Biography;
use crate::session::file::{self, FileStore, FILE_PROTOCOL};
use crate::session::{RpcRequest, RpcResponse, PROTOCOL};
use crate::transport::tor::TorTransport;
use crate::transport::{
    classify_multiaddr, endpoint_record, ensure_dial_allowed, ensure_ipv6_listen_addr,
    ensure_ipv6_peer_addr, ensure_operation_allowed, ensure_peer_id, is_onion, ConnectivityPolicy,
    EndpointRecord, NetworkStatus, Reachability, TransportKind, TransportStatus,
};
use crate::{AgentId, Error, Identity, Result};
use futures::StreamExt;
use libp2p::core::upgrade::Version;
use libp2p::core::Transport;
use libp2p::identity::Keypair;
use libp2p::multiaddr::Protocol;
use libp2p::request_response::{
    self, Message, OutboundRequestId, ProtocolSupport, ResponseChannel,
};
use libp2p::swarm::behaviour::toggle::Toggle;
use libp2p::swarm::{NetworkBehaviour, StreamProtocol, Swarm, SwarmEvent};
use libp2p::SwarmBuilder;
pub use libp2p::{Multiaddr, PeerId};
use libp2p_stream::Control;
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::task::JoinHandle;

/// Default bootstrap multiaddr. Empty means none configured.
pub const DEFAULT_BOOTSTRAP: &str = "/ip6/2402:4e00:c011:4300:3056:455a:18e2:0/tcp/29788/p2p/12D3KooWKfNgZEDi7nhJXtBEXAEnT5zuPaNjgothqKDoFJBmRVuv";
const BIOGRAPHY_REFRESH_SECONDS: u64 = 300;
const BIOGRAPHY_QUERY_LIMIT: u32 = 1000;

#[derive(NetworkBehaviour)]
struct Behaviour {
    rpc: request_response::json::Behaviour<RpcRequest, RpcResponse>,
    relay_client: libp2p::relay::client::Behaviour,
    relay_server: Toggle<libp2p::relay::Behaviour>,
    file: libp2p_stream::Behaviour,
}

/// L1 transport configuration.
#[derive(Debug, Clone)]
pub struct TransportConfig {
    pub connectivity_policy: ConnectivityPolicy,
    pub relay: Option<Multiaddr>,
    pub relay_server: bool,
    pub tor_proxy: Option<SocketAddrV6>,
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

/// Biography configuration.
#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub description: String,
    pub tags: Vec<String>,
    pub refresh_interval: Duration,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            description: "Sceadu agent".to_owned(),
            tags: vec!["tennis".to_owned()],
            refresh_interval: Duration::from_secs(BIOGRAPHY_REFRESH_SECONDS),
        }
    }
}

/// Composition-root configuration.
#[derive(Debug, Clone, Default)]
pub struct NodeConfig {
    pub transport: TransportConfig,
    pub session: SessionConfig,
    pub agent: AgentConfig,
    pub inbox_path: Option<PathBuf>,
    pub file_store_path: Option<PathBuf>,
    pub biography_store_path: Option<PathBuf>,
    /// Bootstrap peers. Empty means no bootstrap.
    pub bootstrap: Vec<Multiaddr>,
}

struct NodeState {
    sources: HashMap<PeerId, SourceStats>,
    address_book: AddressBook,
    pending_dial_endpoints: HashMap<PeerId, String>,
    connectivity_policy: ConnectivityPolicy,
    relay_enabled: bool,
    relay_server: bool,
    tor_enabled: bool,
    relay_reservation: Option<PeerId>,
    pending_dials: HashMap<PeerId, oneshot::Sender<Result<PeerId>>>,
    pending_biographies: HashMap<OutboundRequestId, oneshot::Sender<Result<Vec<Biography>>>>,
    pending_peer_biographies:
        HashMap<OutboundRequestId, (PeerId, oneshot::Sender<Result<Biography>>)>,
    pending_messages: HashMap<OutboundRequestId, oneshot::Sender<Result<()>>>,
    agent_id: AgentId,
    inbox: Inbox,
    biographies: BiographyStore,
    connected_peers: HashSet<PeerId>,
}

#[derive(Default)]
struct SourceStats {
    failures: u32,
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

impl Default for NodeState {
    fn default() -> Self {
        Self {
            sources: HashMap::new(),
            address_book: AddressBook::default(),
            pending_dial_endpoints: HashMap::new(),
            connectivity_policy: ConnectivityPolicy::Auto,
            relay_enabled: false,
            relay_server: false,
            tor_enabled: false,
            relay_reservation: None,
            pending_dials: HashMap::new(),
            pending_biographies: HashMap::new(),
            pending_peer_biographies: HashMap::new(),
            pending_messages: HashMap::new(),
            agent_id: Identity::generate().agent_id(),
            inbox: Inbox::open(None).expect("in-memory inbox"),
            biographies: BiographyStore::open(None).expect("in-memory biography store"),
            connected_peers: HashSet::new(),
        }
    }
}

enum Command {
    Dial {
        addr: Multiaddr,
        reply: oneshot::Sender<Result<PeerId>>,
    },
    SendMessage {
        peer: PeerId,
        message: Box<AgentMessage>,
        reply: oneshot::Sender<Result<()>>,
    },
    PublishBiography,
    BiographyUpdate {
        description: Option<String>,
        tags: Option<Vec<String>>,
        reply: oneshot::Sender<Result<Biography>>,
    },
    RefreshBiographies {
        reply: oneshot::Sender<Result<Vec<Biography>>>,
    },
    RequestBiography {
        peer: PeerId,
        reply: oneshot::Sender<Result<Biography>>,
    },
    AddAddress {
        peer: PeerId,
        addr: Multiaddr,
        reply: oneshot::Sender<Result<()>>,
    },
    DialPeer {
        peer: PeerId,
        reply: oneshot::Sender<Result<PeerId>>,
    },
    Status {
        reply: oneshot::Sender<NetworkStatus>,
    },
    Peers {
        reply: oneshot::Sender<Vec<PeerId>>,
    },
    Shutdown,
}

/// A running Sceadu node. Commands are serialized through the task that owns
/// the libp2p swarm.
pub struct NodeHandle {
    peer_id: PeerId,
    listen_addr: Option<Multiaddr>,
    agent_id: AgentId,
    inbox: Inbox,
    biographies: BiographyStore,
    file_control: Arc<Mutex<Control>>,
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

    pub async fn send_message(&self, peer: PeerId, message: AgentMessage) -> Result<()> {
        let (reply, receiver) = oneshot::channel();
        self.commands
            .send(Command::SendMessage {
                peer,
                message: Box::new(message),
                reply,
            })
            .await
            .map_err(|_| Error::Closed)?;
        receiver.await.map_err(|_| Error::Closed)?
    }

    pub async fn update_biography(
        &self,
        description: Option<String>,
        tags: Option<Vec<String>>,
    ) -> Result<Biography> {
        let (reply, receiver) = oneshot::channel();
        self.commands
            .send(Command::BiographyUpdate {
                description,
                tags,
                reply,
            })
            .await
            .map_err(|_| Error::Closed)?;
        receiver.await.map_err(|_| Error::Closed)?
    }

    pub async fn publish_biography(&self) -> Result<()> {
        self.commands
            .send(Command::PublishBiography)
            .await
            .map_err(|_| Error::Closed)
    }

    pub async fn request_biography(&self, peer: PeerId) -> Result<Biography> {
        let (reply, receiver) = oneshot::channel();
        self.commands
            .send(Command::RequestBiography { peer, reply })
            .await
            .map_err(|_| Error::Closed)?;
        receiver.await.map_err(|_| Error::Closed)?
    }

    pub async fn refresh_biographies(&self) -> Result<Vec<Biography>> {
        let (reply, receiver) = oneshot::channel();
        self.commands
            .send(Command::RefreshBiographies { reply })
            .await
            .map_err(|_| Error::Closed)?;
        receiver.await.map_err(|_| Error::Closed)?
    }

    pub fn biographies(&self) -> Result<Vec<Biography>> {
        self.biographies.all()
    }

    pub fn biography(&self, agent_id: &AgentId) -> Result<Option<Biography>> {
        self.biographies.get(agent_id)
    }

    pub async fn add_address(&self, peer: PeerId, addr: Multiaddr) -> Result<()> {
        let (reply, receiver) = oneshot::channel();
        self.commands
            .send(Command::AddAddress { peer, addr, reply })
            .await
            .map_err(|_| Error::Closed)?;
        receiver.await.map_err(|_| Error::Closed)?
    }

    pub async fn peers(&self) -> Result<Vec<PeerId>> {
        let (reply, receiver) = oneshot::channel();
        self.commands
            .send(Command::Peers { reply })
            .await
            .map_err(|_| Error::Closed)?;
        receiver.await.map_err(|_| Error::Closed)
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

    pub fn inbox(&self) -> Inbox {
        self.inbox.clone()
    }

    pub async fn send_file(
        &self,
        peer: PeerId,
        path: impl AsRef<std::path::Path>,
    ) -> Result<FileRef> {
        let mut control = self.file_control.lock().await;
        let stream = control
            .open_stream(peer, StreamProtocol::new(FILE_PROTOCOL))
            .await
            .map_err(|error| Error::Unsupported(format!("open file stream: {error}")))?;
        file::send(stream, path.as_ref()).await
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
    for bootstrap in &config.bootstrap {
        ensure_ipv6_peer_addr(bootstrap)?;
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
        .map_err(|error| Error::IdentityMessage(error.to_string()))?;
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
    let swarm = SwarmBuilder::with_existing_identity(keypair)
        .with_tokio()
        .with_tcp(
            libp2p::tcp::Config::default(),
            libp2p::noise::Config::new,
            libp2p::yamux::Config::default,
        )
        .map_err(|error| Error::Transport(crate::transport::Error::Other(error.to_string())))?
        .with_other_transport(move |key| {
            let noise =
                libp2p::noise::Config::new(key).expect("generated ed25519 key is valid for Noise");
            TorTransport::new(tor_proxy)
                .upgrade(Version::V1Lazy)
                .authenticate(noise)
                .multiplex(libp2p::yamux::Config::default())
                .boxed()
        })
        .map_err(|error| Error::Transport(crate::transport::Error::Other(error.to_string())))?
        .with_relay_client(libp2p::noise::Config::new, libp2p::yamux::Config::default)
        .map_err(|error| Error::Transport(crate::transport::Error::Other(error.to_string())))?
        .with_behaviour(move |_key, relay_client| Behaviour {
            rpc: rpc_behaviour(),
            relay_client,
            relay_server: Toggle::from(relay_server),
            file: libp2p_stream::Behaviour::new(),
        })
        .map_err(|error| Error::Behaviour(error.to_string()))?;
    let mut swarm = swarm
        .with_swarm_config(|swarm_config| swarm_config.with_idle_connection_timeout(idle_timeout))
        .build();
    let mut file_control = swarm.behaviour_mut().file.new_control();
    let mut incoming_files = file_control
        .accept(StreamProtocol::new(FILE_PROTOCOL))
        .map_err(|error| Error::Unsupported(format!("file protocol: {error}")))?;
    let file_store_path = config
        .file_store_path
        .clone()
        .unwrap_or_else(|| std::env::temp_dir().join(format!("sceadu-files-{}", peer_id)));
    let file_store = FileStore::open(file_store_path)?;
    tokio::spawn(async move {
        while let Some((_peer, stream)) = incoming_files.next().await {
            let store = file_store.clone();
            tokio::spawn(async move {
                let _ = file::receive(store, stream).await;
            });
        }
    });

    for address in &config.bootstrap {
        if let Some(peer) = peer_from_multiaddr(address) {
            swarm.add_peer_address(peer, address.clone());
        }
        let _ = swarm.dial(address.clone());
    }

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

    let mut endpoints = Vec::new();
    if let Some(onion) = &onion {
        endpoints.push(onion.to_string());
    }
    if let Some(address) = &listen_addr {
        if !endpoints
            .iter()
            .any(|endpoint| endpoint == &address.to_string())
        {
            endpoints.push(address.to_string());
        }
    }
    let biography_store_path = config
        .biography_store_path
        .clone()
        .unwrap_or_else(|| std::env::temp_dir().join(format!("sceadu-biographies-{}", peer_id)));
    let biographies = BiographyStore::open(Some(&biography_store_path))?;
    let previous = biographies.get(&identity.agent_id())?;
    let description = previous
        .as_ref()
        .map(|biography| biography.description.clone())
        .unwrap_or_else(|| config.agent.description.clone());
    let tags = previous
        .as_ref()
        .map(|biography| biography.tags.clone())
        .unwrap_or_else(|| config.agent.tags.clone());
    let local_biography = Biography::new(identity, endpoints, description, tags)?;
    biographies.upsert(&local_biography)?;
    let inbox = Inbox::open(config.inbox_path.as_deref()).map_err(Error::Agent)?;
    let (commands, receiver) = mpsc::channel(32);
    let task = tokio::spawn(run_node(
        swarm,
        local_biography.clone(),
        identity.clone(),
        receiver,
        config,
        inbox.clone(),
        biographies.clone(),
    ));
    let handle = NodeHandle {
        peer_id,
        listen_addr: listen_addr.clone(),
        agent_id: local_biography.agent_id.clone(),
        inbox,
        biographies,
        file_control: Arc::new(Mutex::new(file_control)),
        commands,
        task,
    };
    handle.publish_biography().await?;

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
    mut local_biography: Biography,
    identity: Identity,
    mut commands: mpsc::Receiver<Command>,
    config: NodeConfig,
    inbox: Inbox,
    biographies: BiographyStore,
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
        agent_id: local_biography.agent_id.clone(),
        inbox,
        biographies,
        ..Default::default()
    };
    let mut refresh = tokio::time::interval(config.agent.refresh_interval);

    loop {
        tokio::select! {
            _ = refresh.tick() => {
                publish_biography(&mut swarm, &local_biography, &state, &config);
            }
            event = swarm.select_next_some() => {
                handle_event(
                    &mut swarm,
                    event,
                    &mut local_biography,
                    &identity,
                    &mut state,
                    &config,
                );
            }
            command = commands.recv() => {
                match command {
                    Some(command) => {
                        if !handle_command(&mut swarm, command, &mut local_biography, &identity, &mut state, &config) {
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
    local_biography: &mut Biography,
    identity: &Identity,
    state: &mut NodeState,
    config: &NodeConfig,
) {
    match event {
        SwarmEvent::ConnectionEstablished { peer_id, .. } => {
            state.connected_peers.insert(peer_id);
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
            state.connected_peers.remove(&peer_id);
        }
        SwarmEvent::NewListenAddr { address, .. } => {
            let address = ensure_peer_id(address, *swarm.local_peer_id());
            state
                .address_book
                .upsert(endpoint_record(&address, "listen", false));
            add_biography_endpoint(swarm, local_biography, identity, state, config, &address);
        }
        SwarmEvent::ExternalAddrConfirmed { address } => {
            state
                .address_book
                .upsert(endpoint_record(&address, "external", false));
            add_biography_endpoint(swarm, local_biography, identity, state, config, &address);
        }
        SwarmEvent::Behaviour(event) => match event {
            BehaviourEvent::Rpc(event) => {
                handle_rpc_event(swarm, event, local_biography, state, config)
            }
            BehaviourEvent::RelayClient(event) => {
                if let libp2p::relay::client::Event::ReservationReqAccepted {
                    relay_peer_id, ..
                } = event
                {
                    state.relay_reservation = Some(relay_peer_id);
                }
            }
            BehaviourEvent::RelayServer(_) | BehaviourEvent::File(_) => {}
        },
        _ => {}
    }
}

fn handle_rpc_event(
    swarm: &mut Swarm<Behaviour>,
    event: request_response::Event<RpcRequest, RpcResponse>,
    local_biography: &Biography,
    state: &mut NodeState,
    config: &NodeConfig,
) {
    match event {
        request_response::Event::Message { peer, message, .. } => match message {
            Message::Request {
                request, channel, ..
            } => respond_to_request(
                swarm,
                peer,
                request,
                channel,
                local_biography,
                state,
                config,
            ),
            Message::Response {
                request_id,
                response,
            } => handle_response(peer, request_id, response, state),
        },
        request_response::Event::OutboundFailure {
            peer,
            request_id,
            error,
            ..
        } => {
            let message = format!("request failed: {error}");
            if let Some(reply) = state.pending_messages.remove(&request_id) {
                let _ = reply.send(Err(Error::Request(message.clone())));
            }
            if let Some(reply) = state.pending_biographies.remove(&request_id) {
                let _ = reply.send(Err(Error::Request(message.clone())));
            }
            if let Some((_, reply)) = state.pending_peer_biographies.remove(&request_id) {
                let _ = reply.send(Err(Error::Request(message)));
            }
            record_source_failure(state, &peer);
        }
        _ => {}
    }
}

fn handle_incoming_message(_peer: PeerId, message: AgentMessage, state: &NodeState) -> RpcResponse {
    let fail = |message: String| RpcResponse::Error { message };
    if let Err(error) = message.validate() {
        return fail(error.to_string());
    }
    if message.to != state.agent_id.to_string() {
        return fail("message is not addressed to this agent".to_owned());
    }
    if message.from.parse::<AgentId>().is_err() {
        return fail("invalid sender agent id".to_owned());
    }
    match state.inbox.ingest(&message) {
        Ok(_) => RpcResponse::MessageAck {
            message_id: message.message_id,
        },
        Err(error) => RpcResponse::Error {
            message: error.to_string(),
        },
    }
}

fn respond_to_request(
    swarm: &mut Swarm<Behaviour>,
    peer: PeerId,
    request: RpcRequest,
    channel: ResponseChannel<RpcResponse>,
    local_biography: &Biography,
    state: &mut NodeState,
    config: &NodeConfig,
) {
    let response = match request {
        RpcRequest::Biography(biography) => {
            match store_biography_for_peer(&biography, &peer, state) {
                Ok(()) => RpcResponse::BiographyAck,
                Err(error) => RpcResponse::Error {
                    message: error.to_string(),
                },
            }
        }
        RpcRequest::Biographies { limit } => {
            let biographies = state
                .biographies
                .all()
                .unwrap_or_default()
                .into_iter()
                .filter(|biography| {
                    biography.agent_id != local_biography.agent_id
                        || config.bootstrap.is_empty()
                        || peer != *swarm.local_peer_id()
                })
                .take(limit.clamp(1, BIOGRAPHY_QUERY_LIMIT) as usize)
                .collect();
            RpcResponse::Biographies { biographies }
        }
        RpcRequest::Message(message) => handle_incoming_message(peer, *message, state),
    };

    let _ = swarm.behaviour_mut().rpc.send_response(channel, response);
}

fn store_biography_for_peer(
    biography: &Biography,
    peer: &PeerId,
    state: &mut NodeState,
) -> Result<()> {
    biography.verify()?;
    let expected_peer =
        biography_peer_id(biography).or_else(|| peer_id_for_agent_id(&biography.agent_id).ok());
    if expected_peer != Some(*peer) {
        return Err(Error::Request(
            "biography does not match the authenticated peer".to_owned(),
        ));
    }
    state.biographies.upsert(biography)?;
    record_source_success(state, peer);
    Ok(())
}

fn handle_response(
    peer: PeerId,
    request_id: OutboundRequestId,
    response: RpcResponse,
    state: &mut NodeState,
) {
    if let Some(reply) = state.pending_messages.remove(&request_id) {
        let result = match response {
            RpcResponse::MessageAck { .. } => Ok(()),
            RpcResponse::Error { message } => Err(Error::Request(message)),
            _ => Err(Error::Request("unexpected message response".to_owned())),
        };
        let _ = reply.send(result);
        return;
    }

    if let Some((peer_id, reply)) = state.pending_peer_biographies.remove(&request_id) {
        let result = match response {
            RpcResponse::Biographies { biographies } => biographies
                .into_iter()
                .find(|biography| biography_peer_id(biography) == Some(peer_id))
                .ok_or_else(|| Error::Request("peer biography not returned".to_owned())),
            RpcResponse::Error { message } => Err(Error::Request(message)),
            _ => Err(Error::Request("unexpected biography response".to_owned())),
        };
        let _ = reply.send(result);
        return;
    }

    if let Some(reply) = state.pending_biographies.remove(&request_id) {
        let result = match response {
            RpcResponse::Biographies { biographies } => {
                let mut valid = Vec::new();
                for biography in biographies {
                    if biography.verify().is_ok() {
                        let _ = state.biographies.upsert(&biography);
                        valid.push(biography);
                    }
                }
                record_source_success(state, &peer);
                Ok(valid)
            }
            RpcResponse::Error { message } => Err(Error::Request(message)),
            _ => Err(Error::Request("unexpected biography response".to_owned())),
        };
        let _ = reply.send(result);
    }
}

fn publish_biography(
    swarm: &mut Swarm<Behaviour>,
    local_biography: &Biography,
    state: &NodeState,
    config: &NodeConfig,
) {
    let local_peer = *swarm.local_peer_id();
    let mut seen = HashSet::new();
    let mut targets: Vec<Multiaddr> = Vec::new();

    // Publish to every known neighbor, not just the bootstrap seed.
    for biography in state.biographies.all().unwrap_or_default() {
        if biography.agent_id == local_biography.agent_id {
            continue;
        }
        for endpoint in biography.endpoints {
            let Ok(address) = endpoint.parse::<Multiaddr>() else {
                continue;
            };
            let Some(peer) = peer_from_multiaddr(&address) else {
                continue;
            };
            if peer == local_peer || !seen.insert(peer) {
                continue;
            }
            targets.push(address);
        }
    }
    for address in &config.bootstrap {
        let Some(peer) = peer_from_multiaddr(address) else {
            continue;
        };
        if peer == local_peer || !seen.insert(peer) {
            continue;
        }
        targets.push(address.clone());
    }

    for address in targets {
        let Some(peer) = peer_from_multiaddr(&address) else {
            continue;
        };
        if !swarm.is_connected(&peer) {
            let _ = swarm.dial(address);
        }
        let _ = swarm.behaviour_mut().rpc.send_request(
            &peer,
            RpcRequest::Biography(Box::new(local_biography.clone())),
        );
    }
}

fn handle_command(
    swarm: &mut Swarm<Behaviour>,
    command: Command,
    local_biography: &mut Biography,
    identity: &Identity,
    state: &mut NodeState,
    config: &NodeConfig,
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
                let _ = reply.send(Err(Error::from(error)));
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
        Command::SendMessage {
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
                let _ = reply.send(Err(Error::from(error)));
                return true;
            }
            if let Err(error) = message.validate() {
                let _ = reply.send(Err(Error::Agent(error)));
                return true;
            }
            let request_id = swarm
                .behaviour_mut()
                .rpc
                .send_request(&peer, RpcRequest::Message(message));
            state.pending_messages.insert(request_id, reply);
            true
        }
        Command::PublishBiography => {
            publish_biography(swarm, local_biography, state, config);
            true
        }
        Command::BiographyUpdate {
            description,
            tags,
            reply,
        } => {
            let description = description.unwrap_or_else(|| local_biography.description.clone());
            let tags = tags.unwrap_or_else(|| local_biography.tags.clone());
            match Biography::new(
                identity,
                local_biography.endpoints.clone(),
                description,
                tags,
            ) {
                Ok(next) => {
                    if let Err(error) = state.biographies.upsert(&next) {
                        let _ = reply.send(Err(error));
                        return true;
                    }
                    *local_biography = next.clone();
                    publish_biography(swarm, local_biography, state, config);
                    let _ = reply.send(Ok(next));
                }
                Err(error) => {
                    let _ = reply.send(Err(Error::Identity(error)));
                }
            }
            true
        }
        Command::RequestBiography { peer, reply } => {
            let request_id = swarm.behaviour_mut().rpc.send_request(
                &peer,
                RpcRequest::Biographies {
                    limit: BIOGRAPHY_QUERY_LIMIT,
                },
            );
            state
                .pending_peer_biographies
                .insert(request_id, (peer, reply));
            true
        }
        Command::RefreshBiographies { reply } => {
            let Some(address) = config.bootstrap.first() else {
                let _ = reply.send(Ok(state.biographies.all().unwrap_or_default()));
                return true;
            };
            let Some(peer) = peer_from_multiaddr(address) else {
                let _ = reply.send(Err(Error::Dial("invalid bootstrap address".to_owned())));
                return true;
            };
            if !swarm.is_connected(&peer) {
                let _ = swarm.dial(address.clone());
            }
            let request_id = swarm.behaviour_mut().rpc.send_request(
                &peer,
                RpcRequest::Biographies {
                    limit: BIOGRAPHY_QUERY_LIMIT,
                },
            );
            state.pending_biographies.insert(request_id, reply);
            true
        }
        Command::AddAddress { peer, addr, reply } => {
            state
                .address_book
                .upsert(endpoint_record(&addr, "manual", false));
            swarm.add_peer_address(peer, addr);
            let _ = reply.send(Ok(()));
            true
        }
        Command::DialPeer { peer, reply } => {
            if let Err(error) = ensure_operation_allowed(
                state.connectivity_policy,
                state.relay_enabled,
                state.relay_server,
                state.tor_enabled,
            ) {
                let _ = reply.send(Err(Error::from(error)));
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
        Command::Status { reply } => {
            let status = build_network_status(swarm.local_peer_id(), local_biography, state);
            let _ = reply.send(status);
            true
        }
        Command::Peers { reply } => {
            let mut peers: Vec<PeerId> = state.connected_peers.iter().copied().collect();
            peers.sort_by_key(|peer| peer.to_string());
            let _ = reply.send(peers);
            true
        }
        Command::Shutdown => false,
    }
}

fn add_biography_endpoint(
    swarm: &mut Swarm<Behaviour>,
    local_biography: &mut Biography,
    identity: &Identity,
    state: &mut NodeState,
    config: &NodeConfig,
    address: &Multiaddr,
) {
    let value = address.to_string();
    if local_biography
        .endpoints
        .iter()
        .any(|endpoint| endpoint == &value)
    {
        return;
    }
    let mut endpoints = local_biography.endpoints.clone();
    endpoints.push(value);
    if let Ok(next) = Biography::new(
        identity,
        endpoints,
        local_biography.description.clone(),
        local_biography.tags.clone(),
    ) {
        if state.biographies.upsert(&next).is_ok() {
            *local_biography = next;
            publish_biography(swarm, local_biography, state, config);
        }
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
    local_biography: &Biography,
    state: &NodeState,
) -> NetworkStatus {
    let listen = local_biography.endpoints.first().cloned();
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
        agent_id: local_biography.agent_id.to_string(),
        peer_id: peer_id.to_string(),
        listen,
        policy: state.connectivity_policy,
        reachability,
        transports: transport_statuses(state),
        endpoints,
        relay_reservation: state.relay_reservation.map(|peer| peer.to_string()),
    }
}

fn record_source_success(state: &mut NodeState, peer: &PeerId) {
    let stats = state.sources.entry(*peer).or_default();
    stats.failures = 0;
}

fn record_source_failure(state: &mut NodeState, peer: &PeerId) {
    let stats = state.sources.entry(*peer).or_default();
    stats.failures = stats.failures.saturating_add(1);
}

fn peer_from_multiaddr(address: &Multiaddr) -> Option<PeerId> {
    address
        .iter()
        .filter_map(|protocol| match protocol {
            Protocol::P2p(peer_id) => Some(peer_id),
            _ => None,
        })
        .last()
}

fn biography_peer_id(biography: &Biography) -> Option<PeerId> {
    biography
        .endpoints
        .iter()
        .filter_map(|endpoint| endpoint.parse::<Multiaddr>().ok())
        .find_map(|address| peer_from_multiaddr(&address))
}

fn peer_id_for_agent_id(agent_id: &AgentId) -> Result<PeerId> {
    let mut bytes = agent_id.as_bytes().to_vec();
    let keypair = Keypair::ed25519_from_bytes(&mut bytes)
        .map_err(|error| Error::IdentityMessage(error.to_string()))?;
    Ok(keypair.public().to_peer_id())
}
