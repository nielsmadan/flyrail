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
