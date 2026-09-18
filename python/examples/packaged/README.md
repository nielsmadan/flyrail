# Checksum example

`flyrail-checksum digest FILE` prints a file's SHA-256 digest. Application version
`2.1.0` and bundle label `checksum-2026-09` are separate release decisions.

```sh
flyrail-checksum digest ./download.bin
flyrail-checksum ai status --target /path/to/agent-skills
flyrail-checksum ai install --target /path/to/agent-skills
flyrail-checksum ai update --target /path/to/agent-skills
flyrail-checksum ai uninstall --target /path/to/agent-skills
```

`Bundle.from_package("flyrail_example_checksum")` reads resources included in the
wheel. The manifest declares executable intent for `scripts/verify_checksum.py`; it does
not depend on the archive's executable mode. Binary sample bytes and the visible
scratch placeholder are packaged alongside `SKILL.md`.

The stable bundle ID lives in host code. Uninstall never loads the resources, so
it works even if their directory has disappeared. Change the packaged manifest's
label and resources when preparing the next distribution; Flyrail never derives
that label from the host's application version. See [the shared example guide](../../docs/examples.md)
for explicit replacement and return codes.

## Combined configuration

The `config` interface composes that packaged skill with generated instruction
guidance, stdio and HTTP MCP registrations, and a before-tool reminder hook with
its script and text assets. It uses Flyrail's renderer and lifecycle directly.
The configuration bundle ID is `example-checksum-config`; its label comes from
the skill manifest. Both labels remain independent of application version `2.1.0`.

From `python/`, with the example installed, select a disposable project and agent:

```sh
flyrail-checksum config update --agent claude --project .cache/combined-project \
  --node "$(command -v node)" --mcp-command checksum-mcp \
  --mcp-url https://checksum.example.invalid/mcp
flyrail-checksum config status --agent claude --project .cache/combined-project \
  --node "$(command -v node)" --mcp-command checksum-mcp \
  --mcp-url https://checksum.example.invalid/mcp
flyrail-checksum config uninstall --agent claude --project .cache/combined-project
```

In PowerShell use `--node (Get-Command node).Source` on one line. The selected
Node 24+ executable must be available later for native hook execution. These
sample MCP names/endpoints are placeholders: provide an installed stdio server
(extra arguments use repeatable `--mcp-arg`) and an actual HTTP endpoint before
activation. The example neither supplies nor starts an MCP server. It writes
references to `CHECKSUM_MCP_TOKEN` and `CHECKSUM_HTTP_TOKEN` without reading values.
Provide those variables in the agent's environment.

Claude and Codex can represent this complete bundle. Other agent choices may
reject the reminder outcome or HTTP authentication mapping; unsupported notices
block the whole installation before writes. Omitting `--node` for a native host
also reports unsupported. OpenCode/Pi's existing-runtime glue remains available
for bundles whose requested outcomes those hosts support; see the
[host matrix](../../../spec/hook-protocol.md).

`update` installs when absent. Repeating it with matching bytes returns unchanged.
`--guidance TEXT` demonstrates app-generated instructions under the same bundle
label; `--asset-root PATH` changes rendered support destinations without changing
bundle bytes. Repeating `status` with the new inputs reports
`configured_current: false` until `update` succeeds, even if `recorded_current`
is true. Status does not write. A new label in the packaged manifest is also an
update, regardless of how it sorts.

JSON output reports `configured_current` using `InstallationObservation.matches`,
render notices, required runtime/server/environment prerequisites and
`activation: "not-checked"`. Exit `0` means the requested configuration operation
succeeded or status matches; it does not mean an agent loaded the configuration.
Exit `1` includes partial, incomplete, unsupported and modified-content failures;
exit `2` means invalid usage or a source/request error. Review resource errors too.

Foreign document content survives. Update/uninstall refuse edited owned content;
`--replace-modified` explicitly permits replacement/removal of bounded same-owner
edits. Uninstall needs only agent/project and the retained index, even if package
resources disappear. Keep the project's `.cache`, `.flyrail-assets` (or selected
asset root), and generated authority metadata ignored as appropriate for your app.
Avoid installing the skill-only and combined bundles into the same skill container:
they are separate owners and an overlapping skill correctly conflicts.
