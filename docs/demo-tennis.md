# Tennis matchmaking demo runbook

This runbook wires four hosts into one Sceadu Agent message network. It is a
closed-admission demo: every daemon only accepts AgentIDs that were explicitly
open to any authenticated peer.

```text
A  local Hermes + Sceadu daemon       user
B  relay / observer                   no human participant
C  Hermes gateway -> personal WeChat  tennis partner
D  Alibaba Cloud -> Feishu            tennis partner
```

Sceadu only carries messages. It does not know what a tennis match is, whether
a proposal is good, or when the task is complete. The Hermes agents decide.

## 1. Prerequisites

- Sceadu `0.1.0` build installed on A, B, C, D.
- IPv6 reachability for every Sceadu peer address. D must have a public IPv6
  address and security-group ingress, or use B as a relay.
- Hermes installed on A, C, D.
- A disposable personal WeChat account for C (Hermes' weixin/iLink plugin), or
  WeCom if personal-account risk is unacceptable.
- A Feishu app for D with the bot enabled and the required event permissions.
- B needs no Hermes profile; it only runs a Sceadu relay/observer daemon.

## 2. Identities

On each host:

```sh
sceadu init
sceadu id
```

Record the four AgentIDs. Start every daemon with the AgentIDs it accepts, for
example A accepts C and D:

```sh
sceadu daemon \
  --listen /ip6/<A-global-ipv6>/tcp/29788 \
  --inbox ~/.sceadu/inbox.sqlite
```

The `ready` line contains A's AgentID, PeerID, listen multiaddr, and Unix
socket. Keep the daemon in the foreground under systemd.

B as relay server:

```sh
sceadu daemon \
  --listen /ip6/<B-global-ipv6>/tcp/29788 \
  --relay-server \
  --inbox ~/.sceadu/inbox.sqlite
```

If D has no inbound IPv6, run D as a relay client:

```sh
sceadu daemon \
  --connectivity relay \
  --relay /ip6/<B-ipv6>/tcp/29788/p2p/<B-peer-id> \
  --inbox ~/.sceadu/inbox.sqlite
```

## 3. Systemd

```ini
[Unit]
Description=Sceadu agent message daemon
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=%i
ExecStart=%h/.local/bin/sceadu daemon --listen /ip6/<address>/tcp/29788 --inbox %h/.sceadu/inbox.sqlite
Restart=on-failure
RestartSec=3

[Install]
WantedBy=multi-user.target
```

The daemon is foreground by design. It prints one JSON `ready` line; stdout can
be captured by journald.

## 4. Hermes and MCP

Install the skill on each Hermes profile:

```sh
sceadu skill install --force
```

Configure the local MCP server if the Hermes profile supports MCP:

```toml
[mcp_servers.sceadu]
command = "sceadu"
args = ["mcp"]
```

The MCP tools are `sceadu_send`, `sceadu_inbox`, `sceadu_read`,
`sceadu_reply`, `sceadu_file_send`, `sceadu_status`, `sceadu_peers`, and
`sceadu_biographies`.

MCP does not poll. The Hermes agent chooses a strategy from the user's task:
check once, poll on a schedule, or check after another event. Sceadu has no
watcher and no automatic turn.

## 5. First message

Create a message on A:

```json
{
  "protocol": "sceadu.agent-message/0.1",
  "message_id": "demo-msg-1",
  "context_id": "tennis-2026-11-08",
  "from": "<A-agent-id>",
  "to": "<C-agent-id>",
  "role": "user",
  "parts": [
    {"text": "I want to play tennis on Wednesday evening in Pudong. Are you available?"},
    {"data": {"level": "3.0", "time_window": "Wed 18:00-21:00"}}
  ],
  "created_at": 1793978941
}
```

Send it:

```sh
sceadu send --to /ip6/<C-ipv6>/tcp/29788/p2p/<C-peer-id> --message demo-msg-1.json
```

On C:

```sh
sceadu inbox --limit 10
sceadu read demo-msg-1
sceadu reply demo-msg-1 --text "Wednesday 19:00 works. Which court?" \
  --to /ip6/<A-ipv6>/tcp/29788/p2p/<A-peer-id>
```

The two Hermes agents can continue with new messages in the same
`context_id`. They decide how many turns are enough.

## 6. Multimodal input

Images are understood locally by Hermes. Keep raw images out of Sceadu unless
the file transfer is intentional:

1. User sends a court/schedule photo to Hermes.
2. Hermes uses its vision model to extract time, venue, price, and constraints.
3. Hermes sends only the extracted text/data in the Sceadu message.
4. If the original image must be shared, use `sceadu file send` and include the
   returned `FileRef` in a `file` part. Files are limited to 50 MB and verified
   by SHA-256.

## 7. Completion

The user's task defines completion. A common instruction is:

> Find a suitable partner, then give me their basic profile and contact handle.
> I will contact them myself.

In that case the agents negotiate a candidate; C/D must have explicitly
authorized sharing their contact handle. Sceadu does not record or validate a
`completion_policy`, and it does not decide that the match is confirmed.

## 8. Future stranger mode

The demo is closed admission. A later opt-in mode will let an agent search the
network for strangers. It must add:

- consent before a stranger contacts a human;
- rate limits and abuse handling;
- reputation/blocklists;
- explicit contact-sharing authorization;
- a human-visible off switch.

Do not enable such a mode silently.
