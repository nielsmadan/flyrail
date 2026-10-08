---
name: checksum-review
description: Verify a file's SHA-256 checksum using flyrail-checksum.
---

Run `flyrail-checksum digest <file>`. Compare its output with the supplied SHA-256
digest and report whether the bytes match. Do not modify the input file.

`scripts/verify_checksum.py <file> <expected>` provides an equivalent explicit verification.
`assets/sample.bin` is a binary sample. `scratch/README.txt` keeps the scratch
directory present in distributions; place temporary output elsewhere unless asked.
