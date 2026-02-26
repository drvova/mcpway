# mcpway

[![crates.io](https://img.shields.io/crates/v/mcpway.svg)](https://crates.io/crates/mcpway)

![MCPway](https://raw.githubusercontent.com/drvova/mcpway/main/mcpway-ascii-logo.svg)

`mcpway` runs MCP stdio servers over SSE, WebSocket, and Streamable HTTP.

## Install

```bash
cargo install mcpway
```

Crates.io: <https://crates.io/crates/mcpway>

## Supported Platforms

`mcpway` is supported on:

- Linux
- macOS
- Windows

Platform notes:

- OAuth browser launch uses platform-native open commands (`open` on macOS, `xdg-open` on Linux, `start` on Windows).
- `connect --oauth-no-browser` is available when launching a browser is not possible in your environment.

## Quick Start

```bash
cargo build --release -p mcpway
./target/release/mcpway --stdio "./my-mcp-server --root ." --port 8000
```

## Bridge Modes

Examples of supported input/output pairs:

```bash
# stdio -> sse (default output for --stdio)
mcpway --stdio "./my-mcp-server --root ." --port 8000

# stdio -> stdio (managed relay mode)
mcpway --stdio "./my-mcp-server --root ." --output-transport stdio

# stdio -> ws
mcpway --stdio "./my-mcp-server --root ." --output-transport ws --port 8000

# stdio -> streamable-http
mcpway --stdio "./my-mcp-server --root ." --output-transport streamable-http --port 8000

# sse -> stdio
mcpway --sse https://example.com/sse

# streamable-http -> stdio
mcpway --streamable-http https://example.com/mcp
```

For endpoint-first usage, use `connect`:

```bash
mcpway connect https://example.com/mcp
mcpway connect wss://example.com/ws --protocol ws
```

## Cargo Workspace

Run from repository root:

```bash
cargo metadata --no-deps
cargo check -p mcpway
cargo test -p mcpway
cargo run -p mcpway -- --help
```

## Release Smoke Test (macOS)

Run this on a macOS host before cutting a release:

```bash
cargo check -p mcpway
cargo test -p mcpway

# stdio -> stdio
cargo run -p mcpway -- --stdio "./my-mcp-server --root ." --output-transport stdio

# stdio -> sse
cargo run -p mcpway -- --stdio "./my-mcp-server --root ." --output-transport sse --port 8000

# oauth flow (verifies browser launch on macOS via `open`)
cargo run -p mcpway -- connect https://example.com/mcp --oauth-login
```

## Command Reference

Shortcuts:
- `-h` / `--help` is available everywhere.
- No custom short flags are defined; use full `--long-option` flags.

Commands:
- `mcpway [OPTIONS]` (run gateway)
- `mcpway generate --definition <PATH> --out <DIR> [OPTIONS]`
- `mcpway regenerate --metadata <PATH> [OPTIONS]`
- `mcpway connect [ENDPOINT] [OPTIONS]`
- `mcpway discover [OPTIONS]`
- `mcpway import [OPTIONS]`
- `mcpway logs <COMMAND>`
- `mcpway logs tail [OPTIONS]`

### mcpway [OPTIONS]
`--stdio` `--sse` `--streamable-http` `--output-transport` `--port` `--base-url` `--sse-path` `--message-path` `--streamable-http-path` `--log-level` `--cors` `--health-endpoint` `--header` `--env` `--oauth2-bearer` `--stateful` `--session-timeout` `--protocol-version` `--runtime-prompt` `--runtime-admin-port` `--runtime-admin-host` `--runtime-admin-token` `--retry-attempts` `--retry-base-delay-ms` `--retry-max-delay-ms` `--circuit-failure-threshold` `--circuit-cooldown-ms`

### mcpway generate
`--definition` `--server` `--out` `--artifact-name` `--bundle-mcpway` `--no-bundle-mcpway` `--mcpway-binary` `--compile-wrapper` `--no-compile-wrapper`

### mcpway regenerate
`--metadata` `--definition` `--server` `--out` `--bundle-mcpway` `--no-bundle-mcpway` `--mcpway-binary` `--compile-wrapper` `--no-compile-wrapper`

### mcpway connect
`--server` `--stdio-cmd` `--stdio-arg` `--stdio-env` `--stdio-wrapper` `--save-wrapper` `--protocol` `--header` `--oauth2-bearer` `--oauth-profile` `--oauth-issuer` `--oauth-client-id` `--oauth-scope` `--oauth-flow` `--oauth-no-browser` `--oauth-cache` `--oauth-login` `--oauth-logout` `--oauth-audience` `--save-profile` `--registry` `--profile-name` `--retry-attempts` `--retry-base-delay-ms` `--retry-max-delay-ms` `--circuit-failure-threshold` `--circuit-cooldown-ms` `--log-level` `--protocol-version`

### mcpway discover
`--from` `--project-root` `--json` `--strict-conflicts` `--search` `--transport` `--scope` `--enabled-only` `--sort` `--order` `--offset` `--limit`

### mcpway import
`--from` `--project-root` `--json` `--strict-conflicts` `--registry` `--save-profiles` `--bundle-mcpway` `--compile-wrapper`

### mcpway logs tail
`--file` `--lines` `--level` `--transport` `--json` `--no-follow`

### Runtime Admin API
When `--runtime-admin-port` is set, the admin server exposes:
- `GET /v1/runtime/health`
- `GET /v1/runtime/metrics` (JSON)
- `GET /v1/runtime/metrics.prom` (Prometheus text)
- `POST /v1/runtime/defaults`
- `POST /v1/runtime/session/{id}`
- `GET /v1/runtime/sessions`
- `POST /v1/discovery/search`

Auth controls:
- `--runtime-admin-token` (or `MCPWAY_RUNTIME_ADMIN_TOKEN`) accepts `Authorization: Bearer <token>`.
- `--runtime-admin-host` + loopback policy govern network exposure.

Migration notes (breaking change):
- Legacy `/runtime/*` routes were removed; use `/v1/runtime/*`.
- Legacy `x-mcpway-token` header was removed; use `Authorization: Bearer <token>`.
- Legacy camelCase CLI flags were removed; use canonical kebab-case flags.
- Tool API alias invocation was removed; call tools by canonical name only.
- Windsurf legacy config path `~/.codeium/mcp_config.json` is no longer read.
- Discovery source parsers now require canonical source keys (for example no `mcp.servers` fallback in Claude/VSCode parser paths).
- OpenCode discovery now reads only `environment` (no `env` fallback).
- Tool API transport now reads only `Mcp-Session-Id` for session capture.
- Gateway session handling now uses canonical `Mcp-Session-Id` only.
- Connect wrapper JSON now requires `normalized.command`, `normalized.args`, and `normalized.env_template`.
- `transport=sse` URL query hint inference was removed; only explicit type/path/scheme inference remains.
- Generate definition parsing now requires top-level `mcpServers`; root-level server objects are no longer accepted.

## Maintainer

[@drvova](https://github.com/drvova)

## License

[MIT](./LICENSE)
