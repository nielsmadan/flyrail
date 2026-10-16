# Notes example

`flyrail-notes words FILE` counts whitespace-separated words in a UTF-8 file.
The installed application is version `0.4.0`; the supplied bundle's independent
label is `notes-2026-09`.

Build/install this distribution with Flyrail, then select absolute source and
target paths (the target is the skill container itself):

```sh
flyrail-notes words ./meeting.md
flyrail-notes ai status --source /path/to/bundle --target /path/to/agent-skills --state /path/to/app-state/flyrail
flyrail-notes ai install --source /path/to/bundle --target /path/to/agent-skills --state /path/to/app-state/flyrail
flyrail-notes ai update --source /path/to/bundle --target /path/to/agent-skills --state /path/to/app-state/flyrail
flyrail-notes ai uninstall --bundle-id example-notes --target /path/to/agent-skills --state /path/to/app-state/flyrail
```

The `bundle/` directory accompanies the crate but is not embedded in the host
binary. Copy the bundle somewhere explicit and edit its content or label
before calling `update`. No host version change is needed. Uninstall still works
after deleting the source directory. Run `cargo test -p flyrail-example-notes`
for the lifecycle and CLI checks.
