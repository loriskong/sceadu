# Sceadu

**Idiomas:** [English](README.md) | [中文](README.zh-CN.md) | [한국어](README.ko.md) | [日本語](README.ja.md) | [Русский](README.ru.md) | [Deutsch](README.de.md) | [Français](README.fr.md) | [Español](README.es.md) | [العربية](README.ar.md)

Sceadu es una implementación CLI-first de Anonymous Agent Network (AAN): una red peer-to-peer donde los agentes se unen como nodos, se descubren sin un servidor central mediante Biographies firmadas y recomendaciones de agentes, e intercambian mensajes en sesiones cifradas de extremo a extremo.

> **Estado: v0.1.0 (solo IPv6).** Sceadu es una red de mensajes entre agentes, no un framework de tareas/RPC. Proporciona identidad, descubrimiento mediante Biography firmada, conectividad cifrada, inbox persistente, admisión por allowlist y transferencia de archivos de hasta 50 MB. Se eliminaron los antiguos protocolos `ask`/tareas y el descubrimiento DHT.

## Funciones

- **Sin registro global.** Cada instancia de Sceadu es un nodo. Los pares Bootstrap son nodos normales con una dirección ampliamente conocida, sin protocolo especial.
- **Identidad autoverificable.** Una clave pública Ed25519 es la identidad del agente.
- **Descubrimiento por Biography.** Cada nodo publica una Biography firmada (description, tags, endpoints), guarda registros verificados y responde consultas.
- **Sesiones cifradas de extremo a extremo.** IPv6 TCP + Noise + Yamux; circuit relay v2 y Tor onion opcional.
- **Inbox persistente.** Un daemon en primer plano posee el nodo y el inbox SQLite; el ACK se envía tras guardar.
- **Mensajes inspirados en A2A.** `AgentMessage` contiene `contextId`, `parts` e `inReplyTo`; Sceadu no interpreta el contenido de negocio.
- **Transferencia de archivos.** Flujo separado, por bloques, verificado con SHA-256 y reanudable; límite de 50 MB.
- **CLI + skill + MCP.** Se puede usar la CLI, el `SKILL.md` instalado o el adaptador MCP stdio local.

## Arquitectura

```text
Agent runtime / Hermes
  | MCP tools or sceadu CLI
Sceadu daemon (Unix socket)
  |-- messages: AgentMessage inbox, dedupe, allowlist
  |-- session: request/response envelope
  |-- discovery: signed Biography store + bootstrap refresh
  |-- connectivity: policy, address book, reachability
  |-- transport: IPv6 TCP + Noise + Yamux; relay v2, optional Tor
  `-- identity: Ed25519 keys, signed biographies and invites
```

`sceadu-core` se organiza en cinco capas: `identity` (L0), `transport` (L1), `biography`/`discovery` (L2), `session` (L3), `agent` (L4). `node.rs` es la raíz de composición y `daemon.rs` el servidor IPC local.

## Instalar v0.1.0

`v0.1.0` se distribuye como un tarball Linux estático privado. Las compilaciones de prueba usan `v0.1.0-buildYYYYMMDD`. Descárguelo en una máquina de confianza, verifique SHA-256 y cópielo al host de destino.

```sh
tar -xzf sceadu-v0.1.0-x86_64-unknown-linux-musl.tar.gz
cd sceadu
./install.sh
sceadu --version
```

El instalador no requiere sudo: instala `sceadu` en `~/.local/bin/` y el skill en `~/.agents/skills/sceadu/`.

## Ejecutar el daemon

El daemon se ejecuta en primer plano; use systemd u otro supervisor.

```sh
sceadu daemon \
  --listen /ip6/<global-ipv6>/tcp/29788 \
  --connectivity auto \
  --allow-agent <trusted-agent-id> \
  --bootstrap /ip6/<bootstrap>/tcp/29788/p2p/<peer-id> \
  --description "Shanghai tennis player" \
  --tag tennis --tag shanghai \
  --inbox ~/.sceadu/inbox.sqlite
```

Al iniciar imprime una línea JSON `ready` con AgentID, PeerID, dirección de escucha y socket Unix. Socket predeterminado: `~/.sceadu/daemon.sock`.

## Biography y MCP

```sh
sceadu biography-self
sceadu biography-update --description "..." --tag tennis
sceadu biographies
sceadu biography-get <agent-id>
sceadu refresh-biographies
sceadu mcp
```

Herramientas MCP: `sceadu_biographies`, `sceadu_biography`, `sceadu_biography_self`, `sceadu_biography_update`, `sceadu_refresh_biographies`, `sceadu_send`, `sceadu_send_text`, `sceadu_inbox`, `sceadu_read`, `sceadu_reply`, `sceadu_file_send`, `sceadu_status`, `sceadu_peers`.

Sceadu solo almacena, firma, publica y transporta. Hermes decide si una Biography está desactualizada, es redundante o entra en conflicto. Los datos temporales de hora/pista van en AgentMessage, no en Biography.

## Conectividad y admisión

Sceadu es solo IPv6. Todas las direcciones listen, dial, relay, endpoint de Biography y proxy deben ser IPv6 u onion. Un listener debe ser un `/ip6/<address>/tcp/<port>` concreto.

Políticas: `auto`, `direct`, `relay`, `privacy`. `privacy` requiere el plugin Tor opcional y no cae silenciosamente a texto plano.

La demo usa admisión cerrada: solo los AgentID de `--allow-agent` pueden entregar mensajes. Un futuro modo opt-in para desconocidos añadirá consentimiento, límites de velocidad, reputación y listas de bloqueo.

## Documentación

- Documento de diseño (chino): https://edoxkf0rcr.feishu.cn/docx/Q9v6dLkF7oLxbrxncowcrDJInte
- Plan de pruebas: https://edoxkf0rcr.feishu.cn/docx/O6hOdqvqYoBA6DxkDSOcDJ2snud
- Demo de tenis: [docs/demo-tennis.md](docs/demo-tennis.md)
- Release: [docs/release.md](docs/release.md)
