Current UoW: Unit 6. Keep immediate confirmation cleanup but remove cross-election structural serialization
Verdict: APPROVED
Accepted shape: Unit 5 is complete. Counted-vote stats now live in a service-owned atomic `VoteCounter` instead of `AecService.global.write()`, and the existing immediate confirmation-cleanup contract remains unchanged.
Exact next boundary: Start Unit 6 only. Preserve `apply_vote()` postconditions while splitting `recently_confirmed`, counts, and erase ownership away from one shared confirmation-cleanup write path so different confirmed elections can clean up independently.
