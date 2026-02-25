**MCPway** runs **MCP stdio-based servers** over **SSE (Server-Sent Events)** or **WebSockets (WS)** with one command. This is useful for remote access, debugging, or connecting to clients when your MCP server only supports stdio.

Supported by the mcpway community.

## Installation & Usage

Build the Rust binary:

```bash
cd rust
cargo build --release
```

Run MCPway using the built binary:

```bash
./rust/target/release/mcpway --stdio "./my-mcp-server --root ."
```

For development:

```bash
cargo run --manifest-path rust/Cargo.toml -- --stdio "./my-mcp-server --root ."
```

Install from crates.io:

```bash
cargo install mcpway
```

If the canonical crate name is unavailable, install:

```bash
cargo install mcpway-cli
```

Generate a shareable CLI artifact from an MCP definition:

```bash
mcpway generate \
  --definition ./mcp-servers.json \
  --server myServer \
  --out ./dist/my-server
```

Regenerate artifacts later from metadata:

```bash
mcpway regenerate \
  --metadata ./dist/my-server/mcpway-artifact.json
```

Ad-hoc connection to any MCP endpoint:

```bash
mcpway connect https://example.com/mcp
mcpway connect https://example.com/sse --protocol sse
mcpway connect wss://example.com/ws --protocol ws
mcpway connect https://example.com/mcp \
  --oauth-issuer https://issuer.example.com \
  --oauth-client-id my-client \
  --oauth-scope mcp.read
mcpway connect --stdio-cmd "npx -y @modelcontextprotocol/server-everything" --stdio-arg --verbose
```

Zero-config discovery and import:

```bash
mcpway discover --from auto
mcpway import --from auto --registry ~/.mcpway/imported-mcp-registry.json
```

- **`--stdio "command"`**: Command that runs an MCP server over stdio
- **`--sse "https://example.com/sse"`**: SSE URL to connect to (SSE→stdio mode)
- **`--streamableHttp "https://mcp-server.example.com/mcp"`**: Streamable HTTP URL to connect to (StreamableHttp→stdio mode)
- **`--outputTransport stdio | sse | ws | streamableHttp`**: Output MCP transport (default: `sse` with `--stdio`, `stdio` with `--sse` or `--streamableHttp`). Rust CLI accepts both `streamableHttp` and `streamable-http`.
- **`--port 8000`**: Port to listen on (stdio→SSE or stdio→WS mode, default: `8000`)
- **`--baseUrl "http://localhost:8000"`**: Base URL for SSE or WS clients (stdio→SSE mode; optional)
- **`--ssePath "/sse"`**: Path for SSE subscriptions (stdio→SSE mode, default: `/sse`)
- **`--messagePath "/message"`**: Path for messages (stdio→SSE or stdio→WS mode, default: `/message`)
- **`--streamableHttpPath "/mcp"`**: Path for Streamable HTTP (stdio→Streamable HTTP mode, default: `/mcp`)
- **`--stateful`**: Run stdio→Streamable HTTP in stateful mode
- **`--sessionTimeout 60000`**: Session timeout in milliseconds (stateful stdio→Streamable HTTP mode only)
- **`--header "x-user-id: 123"`**: Add one or more headers (stdio→SSE, SSE→stdio, or Streamable HTTP→stdio mode; can be used multiple times)
- **`--oauth2Bearer "some-access-token"`**: Adds an `Authorization` header with the provided Bearer token
- **`--logLevel debug | info | none`**: Controls logging level (default: `info`). Use `debug` for more verbose logs, `none` to suppress all logs.
- **`--cors`**: Enable CORS (stdio→SSE or stdio→WS mode). Use `--cors` with no values to allow all origins, or supply one or more allowed origins (e.g. `--cors "http://example.com"` or `--cors "/example\\.com$/"` for regex matching).
- **`--healthEndpoint /healthz`**: Register one or more endpoints (stdio→SSE or stdio→WS mode; can be used multiple times) that respond with `"ok"`
- **`--env "KEY=VALUE"`**: Pass one or more child process environment values in stdio modes (can be used multiple times)
- **`connect <endpoint>`**: Ad-hoc endpoint→stdio bridge with protocol auto-detection (`http(s)` defaults to streamable HTTP unless path/query indicates SSE; `ws(s)` maps to WebSocket)
- **`connect --protocol sse|streamable-http|ws`**: Explicit protocol override for endpoint mode
- **`connect --server <name> --registry <path>`**: Resolve imported server definitions (remote or stdio) from registry
- **`connect --save-profile ./dir --profile-name my-endpoint`**: Save reusable connect profile metadata + launcher script
- **`connect --oauth-issuer ... --oauth-client-id ... [--oauth-scope ...]`**: Acquire/cache OAuth access tokens (device/auth-code), then auto-inject `Authorization: Bearer ...`
- **`connect --oauth-login`**: Force interactive OAuth login (ignores cached token reuse)
- **`connect --oauth-logout`**: Remove cached OAuth token for the selected OAuth profile key and exit
- **`connect --stdio-cmd <cmd> [--stdio-arg ...] [--stdio-env KEY=VALUE]`**: Run stdio servers directly from the `connect` interface
- **`connect --stdio-wrapper <path>`**: Run stdio mode from a wrapper/metadata JSON source
- **`connect --save-wrapper <dir>`**: Save resolved stdio wrapper JSON for easy reuse
- **`logs tail [--lines N] [--level ...] [--transport ...] [--json] [--no-follow]`**: Tail structured local logs from `~/.mcpway/logs/mcpway.ndjson`
- **`discover --from auto|cursor|claude|codex|windsurf|opencode|nodecode|vscode`**: Discover MCP server definitions from supported client configs
- **`import --from ... --registry <path> [--save-profiles <dir>]`**: Persist discovered servers to mcpway registry and optionally generate runnable profile artifacts

## MCP Definition Artifact Generation

Use this when you want to turn an MCP server definition into runnable local artifacts:

```bash
mcpway generate --definition ./mcp-servers.json --server myServer --out ./dist/my-server
```

Input definition formats:
- Claude/Cursor style object with `mcpServers`
- Single server object with `command`, optional `args`, optional `env`, optional `headers`

Generated output (default):
- `bin/<artifact-name>` launcher script
- `bin/<artifact-name>-wrapper` compiled host wrapper binary
- `bin/mcpway` bundled mcpway binary
- `.env.example` secret placeholders
- `mcpway-artifact.json` metadata for deterministic regeneration

Secrets are redacted in generated metadata and launcher artifacts. At runtime, values are loaded from environment variables.

## Ad-hoc Endpoint Connections

Use `connect` when you want to bridge a remote endpoint directly without creating a definition first:

```bash
mcpway connect https://example.com/mcp
```

Optional profile save:

```bash
mcpway connect wss://example.com/ws \
  --protocol ws \
  --header "Authorization: Bearer token" \
  --save-profile ./profiles/example \
  --profile-name example-ws
```

Saved profile output includes:
- `bin/<profile-name>` launcher script
- `.env.example` placeholder file for redacted secrets
- `mcpway-artifact.json` metadata (connect mode)

## Release Channels (Rust CLI)

- `main` pushes publish a crates.io dev prerelease automatically:
  - `X.Y.Z-dev.<UTC_TIMESTAMP>.<GITHUB_RUN_NUMBER>`
- `v*` tags publish stable crates:
  - `v0.2.0` tag publishes `0.2.0`
- CI publish auth uses GitHub secret:
  - `CARGO_REGISTRY_TOKEN`

## Runtime MCP Args Injection

You can update MCP server args and headers during runtime instead of only at startup:

- **Interactive prompt**: `--runtimePrompt`
- **Local admin endpoint**: `--runtimeAdminPort 7777` (binds to `127.0.0.1`)

#### Admin API

- `POST /runtime/defaults`
- `POST /runtime/session/{id}`
- `GET /runtime/sessions`

Payload example:

```json
{
  "extra_cli_args": ["--token", "abc123"],
  "env": { "API_KEY": "xyz" },
  "headers": { "Authorization": "Bearer 123" }
}
```

Notes:
- `extra_cli_args` and `env` updates trigger a child restart when applicable.
- `headers` updates are applied live.

### Telemetry (Rust)

The Rust build uses `tracing` with optional OpenTelemetry OTLP export. Logs always print locally (stdout or stderr depending on `--outputTransport`).

**OTLP env vars:**
- `OTEL_EXPORTER_OTLP_ENDPOINT` (base endpoint for traces and logs)
- `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` (override for traces)
- `OTEL_EXPORTER_OTLP_LOGS_ENDPOINT` (override for logs)

**Quick smoke test:**
```bash
cd rust
export OTEL_EXPORTER_OTLP_ENDPOINT="http://localhost:4318"
cargo run -- --stdio "./my-mcp-server --root ./my-folder" --port 8000
```
You should see spans/logs in your local collector. If you want to adjust local verbosity, set `RUST_LOG=debug` or `RUST_LOG=info`.

## Zeabur Deployment (Rust Buildpack)

This repo includes `zbpack.json` to deploy the Rust service from the `rust/` subdirectory and ignore the Dockerfile on Zeabur.

Key settings:
- `rust.app_dir`: `rust`
- `rust.entry`: `mcpway`
- `ignore_dockerfile`: `true`

Notes:
- Zeabur provides a `PORT` env var; MCPway uses it automatically when `--port` is not set.
- You can still pass normal CLI args in the Zeabur start command (for example, `--stdio "./my-mcp-server --root /data"`).

## stdio → SSE

Expose an MCP stdio server as an SSE server:

```bash
./rust/target/release/mcpway \
    --stdio "./my-mcp-server --root ./my-folder" \
    --port 8000 --baseUrl http://localhost:8000 \
    --ssePath /sse --messagePath /message
```

- **Subscribe to events**: `GET http://localhost:8000/sse`
- **Send messages**: `POST http://localhost:8000/message`

## SSE → stdio

Connect to a remote SSE server and expose locally via stdio:

```bash
./rust/target/release/mcpway --sse "https://example.com/sse"
```

Useful for integrating remote SSE MCP servers into local command-line environments.

You can also pass headers when sending requests. This is useful for authentication:

```bash
./rust/target/release/mcpway \
    --sse "https://example.com/sse" \
    --oauth2Bearer "some-access-token" \
    --header "X-My-Header: another-header-value"
```

## Streamable HTTP → stdio

Connect to a remote Streamable HTTP server and expose locally via stdio:

```bash
./rust/target/release/mcpway --streamableHttp "https://mcp-server.example.com/mcp"
```

This mode is useful for connecting to MCP servers that use the newer Streamable HTTP transport protocol. Like SSE mode, you can also pass headers for authentication:

```bash
./rust/target/release/mcpway \
    --streamableHttp "https://mcp-server.example.com/mcp" \
    --oauth2Bearer "some-access-token" \
    --header "X-My-Header: another-header-value"
```

## stdio → Streamable HTTP

Expose an MCP stdio server as a Streamable HTTP server.

### Stateless mode

```bash
./rust/target/release/mcpway \
    --stdio "./my-mcp-server --root ./my-folder" \
    --outputTransport streamableHttp \
    --port 8000
```

### Stateful mode

```bash
./rust/target/release/mcpway \
    --stdio "./my-mcp-server --root ./my-folder" \
    --outputTransport streamableHttp --stateful \
    --sessionTimeout 60000 --port 8000
```

The Streamable HTTP endpoint defaults to `http://localhost:8000/mcp` (configurable via `--streamableHttpPath`).

## stdio → WS

Expose an MCP stdio server as a WebSocket server:

```bash
./rust/target/release/mcpway \
    --stdio "./my-mcp-server --root ./my-folder" \
    --port 8000 --outputTransport ws --messagePath /message
```

- **WebSocket endpoint**: `ws://localhost:8000/message`

## Using with ngrok

Use [ngrok](https://ngrok.com/) to share your local MCP server publicly:

```bash
./rust/target/release/mcpway --port 8000 --stdio "./my-mcp-server --root ."

# In another terminal:
ngrok http 8000
```

ngrok provides a public URL for remote access.

MCP server will be available at URL similar to: https://1234-567-890-12-456.ngrok-free.app/sse

## Running with Docker (Rust)

Build and run the Rust image:

```bash
docker build -f docker/Dockerfile -t mcpway .

docker run -it --rm -p 8000:8000 mcpway \
    --stdio "/usr/local/bin/my-mcp-server --root /" \
    --port 8000
```

Docker pulls dependencies during build. The MCP server runs in the container’s root directory (`/`). You can mount host directories if needed.

## Using with Claude Desktop (SSE → stdio mode)

Claude Desktop can use MCPway’s SSE→stdio mode.

### Local Binary Example

```json
{
  "mcpServers": {
    "mcpwayExample": {
      "command": "/path/to/mcpway",
      "args": ["--sse", "https://example.com/sse"]
    }
  }
}
```

## Using with Cursor (SSE → stdio mode)

Cursor can also integrate with MCPway in SSE→stdio mode. The configuration is similar to Claude Desktop.

### Local Binary Example for Cursor

```json
{
  "mcpServers": {
    "cursorExample": {
      "command": "/path/to/mcpway",
      "args": ["--sse", "https://example.com/sse"]
    }
  }
}
```

**Note:** Although the setup supports sending headers via the `--header` flag, if you need to pass an Authorization header (which typically includes a space, e.g. `"Bearer 123"`), you must use the `--oauth2Bearer` flag due to a known Cursor bug with spaces in command-line arguments.

## Why MCP?

[Model Context Protocol](https://spec.modelcontextprotocol.io/) standardizes AI tool interactions. MCPway converts MCP stdio servers into SSE or WS services, simplifying integration and debugging with web-based or remote clients.

## Advanced Configuration

MCPway emphasizes modularity:

- Automatically manages JSON-RPC versioning.
- Retransmits package metadata where possible.
- stdio→SSE or stdio→WS mode logs via standard output; SSE→stdio mode logs via stderr.

## Additional resources

- [mcpway docs](https://github.com/drvova/mcpway) - runtime argument examples and usage guides.

## Contributors

- [@longfin](https://github.com/longfin)
- [@griffinqiu](https://github.com/griffinqiu)
- [@folkvir](https://github.com/folkvir)
- [@wizizm](https://github.com/wizizm)
- [@dtinth](https://github.com/dtinth)
- [@rajivml](https://github.com/rajivml)
- [@NicoBonaminio](https://github.com/NicoBonaminio)
- [@sibbl](https://github.com/sibbl)
- [@podarok](https://github.com/podarok)
- [@jmn8718](https://github.com/jmn8718)
- [@TraceIvan](https://github.com/TraceIvan)
- [@zhoufei0622](https://github.com/zhoufei0622)
- [@ezyang](https://github.com/ezyang)
- [@aleksadvaisly](https://github.com/aleksadvaisly)
- [@wuzhuoquan](https://github.com/wuzhuoquan)
- [@mantrakp04](https://github.com/mantrakp04)
- [@mheubi](https://github.com/mheubi)
- [@mjmendo](https://github.com/mjmendo)
- [@CyanMystery](https://github.com/CyanMystery)
- [@earonesty](https://github.com/earonesty)
- [@StefanBurscher](https://github.com/StefanBurscher)
- [@tarasyarema](https://github.com/tarasyarema)
- [@pcnfernando](https://github.com/pcnfernando)
- [@Areo-Joe](https://github.com/Areo-Joe)
- [@Joffref](https://github.com/Joffref)
- [@michaeljguarino](https://github.com/michaeljguarino)

## Contributing

Issues and PRs welcome. Please open one if you encounter problems or have feature suggestions.

## Tests

Rust no-Docker verification:

```bash
cd rust
./scripts/verify_no_docker.sh
```

Equivalent commands:

```bash
cd rust
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo check
cargo test --all-targets -- --nocapture
```

Parity-focused Rust tests cover:
- base URL endpoint event correctness for stdio→SSE
- protocolVersion propagation and auto-initialize behavior for SSE→stdio and Streamable HTTP→stdio
- session timeout lifecycle behavior for stateful stdio→Streamable HTTP
- high-concurrency request/event fanout for stdio→SSE, stdio→WS, stdio→Streamable HTTP, SSE→stdio, and Streamable HTTP→stdio

If you add Rust integration tests, keep them self-contained and offline-friendly.

## License

[MIT License](./LICENSE)
