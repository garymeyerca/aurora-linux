# J613 Aurora G15 port snapshot

This branch records the experimental `7.1.12-aurora-g15-port1+` source tree
based on Aurora `aurora-wip` commit `f9572b54204e2cd81c764ce8bdaf0fa1d31ae762`.
It is shared for review and integration work, not as a supported boot target.

- The T8122 GPU and mailbox device-tree nodes remain disabled by default.
- The G15G admission path rejects unsupported or incomplete handoff state
  before normal GPU startup.
- The isolated Asahi driver, kernel Image, and device trees built locally.
  This snapshot does not claim a working render node or GPU acceleration.
- The matching opt-in m1n1 ASC preflight source is commit `ddcd6c0` in the
  separate `m3-m1n1` WIP branch.

The local boot artifacts, firmware, device captures, and machine-specific
diagnostics are not part of this repository. See the private diagnostic bundle
for J613 test evidence.
