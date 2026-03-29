Current UoW: Unit 5. Remove counted-vote serialization in AEC global state
Verdict: APPROVED
Accepted shape: Unit 4 is complete. `VoteQuorumPreparer` now owns published quorum state for the hot path without taking the legacy `OnlineReps` mutex during `prepare()`, and the dependent threshold readers were rewired to consume that published state while preserving quorum-before-tally behavior.
Exact next boundary: Start Unit 5 only. Move counted-vote stats updates off `AecService.global.write()` while keeping current vote-result semantics and leaving immediate confirmation cleanup behavior unchanged.
