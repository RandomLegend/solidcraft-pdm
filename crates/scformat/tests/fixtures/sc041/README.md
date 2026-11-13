Saved by SolidCraft 0.41.0 (design format 55) through its MCP server, 2026-11-13:

- `parts/plate.sc` (PL-100, rev A) and `parts/bracket.sc` (BR-200, rev A): one box each.
- `asm/pair.sc` (SUB-300): `bracket.sc` inserted, then inserted again reusing the link (one
  component, two occurrences).
- `asm/bench.sc` (ASM-1): `plate.sc` twice through one link, `pair.sc` once, and an ISO 4762
  M6x20 socket head cap screw (`parts.insert`, a standard part).

Every link records `file_sha256`, the SHA-256 of the linked file as saved here. Keep the files
byte for byte: the tests depend on those fingerprints matching.
