# Connectivity Policy

M4a defines the local connectivity policy and reachability model. It does not add
new wire transports yet.

## Policies

```text
auto      use the best available path; currently direct TCP
direct    use only direct TCP
relay     require a relay path; unavailable until M4b
privacy   require Tor or I2P; unavailable until M4c
```

`relay` and `privacy` fail closed. A node with either policy must not create a
cleartext listener, and an outbound operation that would use direct TCP is
rejected with `Unsupported`. There is no silent downgrade.

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

Bitcoin-style tried/new tables, feeler probes, persistence, and address gossip
are future work. Addresses are not advertised in manifests until the manifest
endpoint metadata lands in M4b.

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
    {"transport": "relay", "available": false, "note": "planned for M4b"},
    {"transport": "tor", "available": false, "note": "planned for M4c"},
    {"transport": "i2p", "available": false, "note": "planned for M4c"}
  ],
  "endpoints": []
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
automatic port mapping               UPnP / NAT-PMP / PCP
anonymous transports                 Tor / I2P, optional
```

This is not WebRTC wire compatibility. A browser-facing WebRTC transport would be
a separate future decision.

## Security

- `relay`/`privacy` fail closed; a cleartext listener is rejected.
- `privacy` never falls back to direct TCP.
- UPnP/NAT-PMP/PCP will be opt-in when implemented; opening a port is a
  security-sensitive action.
- Relay/TURN see IP and traffic metadata even though task content remains
  end-to-end encrypted.
- Tor/I2P reduce network-level IP exposure but do not guarantee full anonymity
  or traffic-analysis resistance.
