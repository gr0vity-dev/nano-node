Current UoW: Unit 3. Centralize vote-path online-representative ownership
Verdict: APPROVED
Accepted shape: Unit 2 is complete. `VoteProcessorQueue` now keeps one public enqueue boundary but moves producer-side ingress through a channel plus per-key reservation ownership instead of one mutex-guarded global ingress queue; `VoteApplier`, `AecService`, and online-representative behavior remain unchanged.
Exact next boundary: Start Unit 3 only. Create one named owner for synchronous quorum preparation, remove duplicate authoritative `vote_observed()` ownership from `AecFactProcessor`, and preserve the current quorum-before-tally behavior without changing `AecService`.
