# Connectivity Policy

M4b implements the first real reachability paths: circuit relay v2, AutoNAT,
DCUtR, and UPnP/NAT-PMP/PCP port mapping. Tor/I2P and QUIC remain future work.

## Policies

```text
auto      use direct TCP or a configured relay path
direct    use only direct TCP; reject /p2p-circuit addresses
relay     require a relay path; requires --relay or --relay-server
privacy   require Tor or I2P; unavailable until M4c
```

A `privacy` node must not create a cleartext listener and all operations are
rejected with `Unsupported`. A relay client must not create a direct listener;
a relay server must. `direct` rejects `/p2p-circuit` addresses. There is no
silent downgrade.

`auto` currently tries direct TCP and a configured relay path; it does not
automatically select a relay when a direct path fails.

## Reachability

Each endpoint is classified locally:

```text
loopback          127.0.0.0/8 or ::1
private           RFC 1918, IPv6 unique-local, or link-local
public_unverified a public address that has not been confirmed
public_verified   a public address confirmed by a connection or feeler
relay             reached through a relay reservation (future)
tor               .onion endpoint (future)
i2p               I2P destination (future)
unknown           no usable IP or not yet classified
```

This is per-endpoint, not a single node-wide boolean. An agent can have a
loopback listener and a future relay reservation at the same time.

## Address book

M4a keeps an in-memory address book:

```text
EndpointRecord {
  addr          multiaddr string
  transport     tcp | relay | tor | i2p
  reachability  see above
  source        listen | manual | dial | ...
  verified      whether a connection confirmed the address
}
```

Dialed endpoints are added when the dial starts and marked verified when the
connection is established. Manual addresses are added unverified. The local
listen endpoint is included in status with source `listen`.

When a new listen address or external address is confirmed, the node re-signs
its manifest with the additional endpoint (sequence increments, expiry
refreshes). This is how relayed and port-mapped endpoints become visible to
peers. DHT records are not automatically republished after an endpoint change;
callers should publish again if they need the DHT copy refreshed.

Bitcoin-style tried/new tables, feeler probes, persistence, and address gossip
are future work.

## `net status`

`sceadu net status` returns one JSON object:

```json
{
  "agent_id": "aan:...",
  "peer_id": "12D3Koo...",
  "listen": "/ip4/127.0.0.1/tcp/4001/p2p/12D3Koo...",
  "policy": "auto",
  "reachability": "loopback",
  "transports": [
    {"transport": "tcp", "available": true, "note": "direct TCP + Noise"},
    {"transport": "relay", "available": false, "note": "pass --relay or --relay-server to enable"},
    {"transport": "port_mapping", "available": false, "note": "pass --port-mapping to enable"},
    {"transport": "tor", "available": false, "note": "planned for M4c"},
    {"transport": "i2p", "available": false, "note": "planned for M4c"}
  ],
  "endpoints": [],
  "external_endpoint": null,
  "port_mapping": false,
  "relay_reservation": null
}
```

The CLI reports planned transports as unavailable. Agents must read the status
instead of inferring availability from the policy name.

## Transport mapping

Native libp2p equivalents are the chosen implementation:

```text
STUN-like public address discovery   AutoNAT / observed address
TURN-like relay                      circuit relay v2
ICE-like hole punching               DCUtR + candidate addresses
automatic port mapping               portmapper: UPnP / NAT-PMP / PCP
anonymous transports                 Tor / I2P, optional
```

This is not WebRTC wire compatibility. A browser-facing WebRTC transport would be
a separate future decision.

Relay server mode requires a public/reachable listen address and adds it as an
external address before accepting reservations. Relay client mode requires
`--relay`; it reserves a circuit and listens on
`/p2p/<relay>/p2p-circuit/p2p/<self>`. Port mapping is opt-in and only reports an
external endpoint after the router grants a mapping. There is no public
bootstrap or relay deployment in the repository.

## Security

- `privacy` fails closed; a cleartext listener is rejected.
- `direct` rejects relayed addresses.
- UPnP/NAT-PMP/PCP are opt-in; opening a port is a security-sensitive action.
- Relay/TURN see IP and traffic metadata even though task content remains
  end-to-end encrypted.
- Relay/TURN see IP and traffic metadata even though task content remains
  end-to-end encrypted.
- Tor/I2P reduce network-level IP exposure but do not guarantee full anonymity
  or traffic-analysis resistance.
