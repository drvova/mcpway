
[![crates.io](https://img.shields.io/crates/v/mcpway.svg)](https://crates.io/crates/mcpway)

![MCPway](https://raw.githubusercontent.com/drvova/mcpway/main/mcpway-ascii-logo.svg)

`mcpway` runs MCP stdio servers over SSE, WebSocket, and Streamable HTTP.

## Install

```bash
cargo install mcpway
```

Crates.io: <https://crates.io/crates/mcpway>

## Quick Start

```bash
cargo build --release -p mcpway
./target/release/mcpway --stdio "./my-mcp-server --root ." --port 8000
```

## Cargo Workspace

Run from repository root:

```bash
cargo metadata --no-deps
cargo check -p mcpway
cargo test -p mcpway
cargo run -p mcpway -- --help
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

### `mcpway [OPTIONS]`
`--stdio` `--sse` `--streamableHttp` `--outputTransport` `--port` `--baseUrl` `--ssePath` `--messagePath` `--streamableHttpPath` `--logLevel` `--cors` `--healthEndpoint` `--header` `--env` `--oauth2Bearer` `--stateful` `--sessionTimeout` `--protocolVersion` `--runtimePrompt` `--runtimeAdminPort`

### `mcpway generate`
`--definition` `--server` `--out` `--artifact-name` `--bundle-mcpway` `--no-bundle-mcpway` `--mcpway-binary` `--compile-wrapper` `--no-compile-wrapper`

### `mcpway regenerate`
`--metadata` `--definition` `--server` `--out` `--bundle-mcpway` `--no-bundle-mcpway` `--mcpway-binary` `--compile-wrapper` `--no-compile-wrapper`

### `mcpway connect`
`--server` `--stdio-cmd` `--stdio-arg` `--stdio-env` `--stdio-wrapper` `--save-wrapper` `--protocol` `--header` `--oauth2Bearer` `--oauth-profile` `--oauth-issuer` `--oauth-client-id` `--oauth-scope` `--oauth-flow` `--oauth-no-browser` `--oauth-cache` `--oauth-login` `--oauth-logout` `--oauth-audience` `--save-profile` `--registry` `--profile-name` `--logLevel` `--protocolVersion`

### `mcpway discover`
`--from` `--project-root` `--json` `--strict-conflicts`

### `mcpway import`
`--from` `--project-root` `--json` `--strict-conflicts` `--registry` `--save-profiles` `--bundle-mcpway` `--compile-wrapper`

### `mcpway logs tail`
`--file` `--lines` `--level` `--transport` `--json` `--no-follow`

For detailed command docs, run:

```bash
mcpway --help
mcpway connect --help
mcpway logs --help
mcpway logs tail --help
```

## Maintainer

[@drvova](https://github.com/drvova)

## License

[MIT](./LICENSE)
