// Command flyrail-checksum prints SHA-256 digests and installs its companion
// agent configuration from resources embedded in the binary with go:embed.
//
// Usage:
//
//	flyrail-checksum --version
//	flyrail-checksum digest FILE
//	flyrail-checksum ai status|install --target DIR [--target DIR ...] --state DIR
//	flyrail-checksum ai update|uninstall --target DIR [--target DIR ...] --state DIR [--replace-modified]
//	flyrail-checksum config status|update --project DIR --agent AGENT --mcp-command COMMAND --mcp-url URL [options]
//	flyrail-checksum config uninstall --project DIR --agent AGENT [--replace-modified]
//
// The ai commands manage the embedded skill in plain skill directories. The
// config commands render a combined bundle for one agent's project files: the
// skill, generated guidance, a stdio and an HTTP MCP server, and a
// before-tool hook with its support files. Config options are --node PATH,
// --mcp-arg ARG (repeatable), --guidance TEXT and --asset-root DIR.
//
// Every ai and config command prints a JSON report. Exit status 0 means
// success (or, for status, that everything is current), 1 that something
// differs or an operation did not complete, and 2 a usage or source error.
// Uninstall never reads the embedded resources.
package main
