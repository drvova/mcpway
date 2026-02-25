# mcpway

`mcpway` runs MCP stdio-based servers over:

- SSE (Server-Sent Events)
- WebSocket
- Streamable HTTP

## Install

```bash
cargo install mcpway
```

The installed command is:

```bash
mcpway --help
```

## Quick start

Expose a stdio MCP server over SSE:

```bash
mcpway --stdio "./my-mcp-server --root ." --port 8000
```

Generate a shareable CLI artifact from an MCP definition:

```bash
mcpway generate \
  --definition ./mcp-servers.json \
  --server myServer \
  --out ./dist/my-server
```

Regenerate artifacts from metadata:

```bash
mcpway regenerate \
  --metadata ./dist/my-server/mcpway-artifact.json
```

Ad-hoc connection to an MCP endpoint:

```bash
mcpway connect https://example.com/mcp
mcpway connect wss://example.com/ws --protocol ws
mcpway connect https://example.com/mcp \
  --oauth-issuer https://issuer.example.com \
  --oauth-client-id my-client \
  --oauth-scope mcp.read
mcpway connect --stdio-cmd "npx -y @modelcontextprotocol/server-everything"
mcpway logs tail --lines 200 --level info
```

Zero-config discovery and import:

```bash
mcpway discover --from auto
mcpway discover --from nodecode
mcpway import --from auto --registry ~/.mcpway/imported-mcp-registry.json
```

Connect using an imported server definition:

```bash
mcpway connect --server github --registry ~/.mcpway/imported-mcp-registry.json
```

## Source and docs

- Repository: <https://github.com/drvova/mcpway>
- Full usage guide: repository root `README.md`
