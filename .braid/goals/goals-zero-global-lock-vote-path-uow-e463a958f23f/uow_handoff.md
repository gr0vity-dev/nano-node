Current UoW: Unit 4. Reimplement quorum preparation without a global `OnlineReps` mutex
Verdict: APPROVED
Accepted shape: Unit 3 is complete. `VoteQuorumPreparer` is now the named synchronous owner for vote-path quorum preparation, `VoteApplier` depends on that owner, and `AecFactProcessor` no longer performs duplicate authoritative `vote_observed()` hot-path updates.
Exact next boundary: Start Unit 4 only. Keep the Unit 3 ownership contract stable while replacing the process-wide exclusive `OnlineReps` mutex in quorum preparation with a narrower seam that still preserves quorum-before-tally behavior and compatibility for non-hot-path readers.
