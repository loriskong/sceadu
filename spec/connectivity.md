# Connectivity Policy

Sceadu uses IPv6-only peer transport. IPv4 is not accepted in listen, dial,
relay, manifest endpoint, invite seed, or SOCKS proxy addresses. The transport is
TCP + Noise + Yamux. Circuit relay v2, Tor onion, and I2P remain available.

## Policies

```text
auto      use direct IPv6 TCP or a configured relay path
direct    use only direct IPv6 TCP; reject /p2p-circuit, onion, and I2P
relay     require a relay path; requires --relay or --relay-server
privacy   require Tor or I2P; --tor-proxy/--i2p-proxy, plus an externally
          configured --onion/--i2p-address for inbound
```

A listener must be a concrete `/ip6/<address>/tcp/<port>` multiaddr. `::`,
link-local, multicast, and IPv4-mapped addresses are rejected. There is no
external-address parameter: the bound IPv6 address is the advertised address.
Privacy listeners use `::1`; their public endpoint is the onion or I2P address.

A relay client must not create a direct listener. A relay server must use a
concrete globally reachable IPv6 listen address. There is no silent downgrade.

## Reachability

```text
loopback          ::1
private           IPv6 unique-local (fc00::/7)
link_local        IPv6 link-local (fe80::/10)
public_unverified global unicast that has not been confirmed
public_verified   a public address confirmed by a connection
relay             reached through a relay reservation
tor               .onion endpoint
i2p               I2P destination
unknown           unspecified, multicast, documentation, or unsupported
```

Documentation prefixes such as `2001:db8::/32` are not treated as public.

## Address book

```text
EndpointRecord {
  addr          multiaddr string
  transport     tcp | relay | tor | i2p
  reachability  see above
  source        listen | external | dial | ...
  verified      whether a connection confirmed the address
}
```

Dialed endpoints are added when the dial starts and marked verified when the
connection is established. The local listen endpoint is included with source
`listen`. When a new listen address is confirmed, the node re-signs its manifest
with the additional endpoint. DHT records are not automatically republished
after an endpoint change; callers should publish again.

## `net status`

```json
{
  "agent_id": "aan:...",
  "peer_id": "12D3Koo...",
  "listen": "/ip6/::1/tcp/4001/p2p/12D3Koo...",
  "policy": "auto",
  "reachability": "loopback",
  "transports": [
    {"transport": "tcp", "available": true, "note": "IPv6 TCP + Noise"},
    {"transport": "relay", "available": false, "note": "pass --relay or --relay-server to enable"},
    {"transport": "tor", "available": false, "note": "pass --tor-proxy to enable"},
    {"transport": "i2p", "available": false, "note": "pass --i2p-proxy to enable"}
  ],
  "endpoints": [],
  "relay_reservation": null
}
```

The CLI reports transports that are not configured as unavailable. Agents must
read the status instead of inferring availability from the policy name.

## Relay and anonymous transports

Relay server mode uses circuit relay v2. A relay client reserves a circuit and
listens on `/p2p/<relay>/p2p-circuit/p2p/<self>`. Relay addresses are validated
as IPv6 and advertised in the signed manifest.

Tor and I2P are dial-only inside Sceadu. `TorTransport` resolves `/onion3` to a
base32 hostname; `I2pTransport` resolves `/dns4/<name>.i2p` (the I2P naming
exception). Both connect through a SOCKS5 proxy bound to an IPv6 loopback
address, for example `[::1]:9050` and `[::1]:4447`. Inbound onion/I2P traffic is
handled by an external Tor daemon or I2P server tunnel mapping to a loopback
listener. The signed manifest advertises only the configured onion/I2P endpoint
in `privacy` mode.

## Security

- `privacy` fails closed; a cleartext listener and clearnet dials are rejected.
- IPv4 is rejected everywhere in Sceadu configuration and wire-facing address
  handling. The operating system and libp2p dependencies still contain IPv4
  code, but Sceadu does not expose an IPv4 path.
- Tor/I2P hide the peer IP from the remote Sceadu node, but the local daemon and
  SOCKS proxy still see local traffic; this is not a guarantee against traffic
  analysis.
- Relay/TURN see IP and traffic metadata even though task content remains
  end-to-end encrypted.
- Tor/I2P reduce network-level IP exposure but do not guarantee full anonymity
  or traffic-analysis resistance.
