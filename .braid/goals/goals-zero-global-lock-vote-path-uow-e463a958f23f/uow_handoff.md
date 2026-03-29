Current UoW: Unit 2. Delete the global vote-ingress mutex
Verdict: APPROVED
Accepted shape: Unit 1 is complete. `VoteProcessor` now hands off dequeued batch members through an internal execution queue so one worker no longer owns a private serial batch loop; queue ingress, online-representative ownership, and AEC behavior remain unchanged.
Exact next boundary: Start Unit 2 only. Keep one public enqueue boundary, replace the single mutex-protected ingress queue with an execution-ready ingress owner, and preserve rep-tier fairness and backpressure semantics without changing `VoteApplier`, `AecService`, or online-representative ownership.
