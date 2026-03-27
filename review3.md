 ### Issues to Address

  ———

  #### [HIGH] AEC event delivery still has split ownership

  The branch introduces AecService as the boundary, but Node still owns and uses a second handle to the same delivery channel, so event lifecycle and publication no longer have one authoritative owner.

  ┌──────────────────────────────────────────────────────────┬──────────────────────────────────────┐
  │ Evidence                                                 │ Location                             │
  ├──────────────────────────────────────────────────────────┼──────────────────────────────────────┤
  │ `AecService` stores its own `delivery: Arc<AecDelivery>` │ node/src/consensus/active_elections  │
  │                                                          │ /aec_service.rs:30                   │
  │ `Node` also stores `aec_delivery` beside `active`        │ node/src/node.rs:134                 │
  │ Construction returns both service and delivery handles   │ node/src/node.rs:623                 │
  │ `Node` starts the processor through raw delivery         │ node/src/node.rs:1255                │
  │ `Node` stops the queue through raw delivery              │ node/src/node.rs:1657                │
  │ Test bypasses service and publishes raw AEC facts        │ node/src/node.rs:1798                │
  └──────────────────────────────────────────────────────────┴──────────────────────────────────────┘

  Refs: node/src/consensus/active_elections/aec_service.rs:30, node/src/node.rs:134, node/src/node.rs:623, node/src/node.rs:1255, node/src/node.rs:1657, node/src/node.rs:1798

  Fix: Make one owner responsible for AEC delivery lifecycle and publication. Either AecService should own start/stop/publish-test-seam, or delivery should be lifted into a distinct owner that AecService does not also retain.

  ———

  #### [MEDIUM] AecService still ends the branch as a transitional catch-all

  The final API explicitly says some helpers are temporary, but the branch leaves that temporary surface in place across unrelated workflows, which means future readers still need migration context to know what belongs here.

  ┌──────────────────────────────────────────────────────────┬──────────────────────────────────────┐
  │ Evidence                                                 │ Location                             │
  ├──────────────────────────────────────────────────────────┼──────────────────────────────────────┤
  │ Comment says caller-specific helpers are temporary       │ node/src/consensus/active_elections  │
  │ and should disappear as boundary narrows                 │ /aec_service.rs:205                  │
  │ Service still carries RPC-shaped read helper             │ node/src/consensus/active_elections  │
  │ `confirmation_active`                                    │ /aec_service.rs:147                  │
  │ Service still carries workflow-specific mutators         │ node/src/consensus/active_elections  │
  │ `confirm_dependent_elections`, `try_add_fork`,           │ /aec_service.rs:207                  │
  │ `transition_time`, `remove_votes`                        │                                      │
  │ Service still carries scheduler-specific helpers         │ node/src/consensus/active_elections  │
  │ `priority_bucket_state`, `next_vote_to_broadcast...`     │ /aec_service.rs:256                  │
  │ Distinct callers still depend on these caller-shaped     │ node/src/ledger_event_processor.rs:91│
  │ helpers instead of one stable service contract           │ node/src/consensus/aec_fork_inserter │
  │                                                          │ .rs:51                               │
  │                                                          │ node/src/consensus/vote_generation/  │
  │                                                          │ aec_voter.rs:60                      │
  └──────────────────────────────────────────────────────────┴──────────────────────────────────────┘

  Refs: node/src/consensus/active_elections/aec_service.rs:147, node/src/consensus/active_elections/aec_service.rs:205, node/src/consensus/active_elections/aec_service.rs:256, node/src/ledger_event_processor.rs:91, node/src/
  consensus/aec_fork_inserter.rs:51, node/src/consensus/vote_generation/aec_voter.rs:60

  Fix: Collapse the temporary surface into explicit owner-aligned seams. At minimum, separate activation, ticker traversal, dependent-confirmation handling, and RPC snapshot generation instead of leaving one broad service to absorb
  all caller-specific shapes.

  ———

  #### [MEDIUM] The activation cleanup sequence did not converge cleanly

  The branch claims to delete activation compatibility leftovers, but the very next commit restores a removed upgrade path. That makes the sequence harder to trust and harder to bisect.

  ┌──────────────────────────────────────────────────────────┬──────────────────────────────────────┐
  │ Evidence                                                 │ Location                             │
  ├──────────────────────────────────────────────────────────┼──────────────────────────────────────┤
  │ Commit `0bf6d9ac8995` says leftovers were deleted        │ git log                              │
  │ Commit `7192d362c35c` immediately restores behavior      │ git log                              │
  │ `0bf6d9ac8995` changed existing-candidate priority path  │ active_elections_container.rs diff   │
  │ from upgrade-through-insert to duplicate                 │                                      │
  │ `7192d362c35c` restores `return self.insert(request,     │ node/src/consensus/active_elections  │
  │ now)` and adds a regression test                         │ /active_elections_container.rs:237   │
  │ Same branch also contains two later reverts of test      │ commits `a55b2bebcaf0`,              │
  │ harness detours                                          │ `45e7f1a8dccf`                       │
  └──────────────────────────────────────────────────────────┴──────────────────────────────────────┘

  Refs: node/src/consensus/active_elections/active_elections_container.rs:237


  ### Justified Concepts (no action needed)

  ┌──────────────────────────────┬────────────────────────────────────────────────────────────────┐
  │ Concept                      │ Verdict                                                        │
  ├──────────────────────────────┼────────────────────────────────────────────────────────────────┤
  │ `AecActivateRequest`         │ Justified — external activation policy differs by caller.      │
  │ `AecInsertRequest`           │ Justified — private normalized insert shape inside container.  │
  │ `ConfirmationActiveInfo`     │ Justified — narrow RPC snapshot avoids cloning whole elections.│
  │ `AecFacts`                   │ Justified — internal fact batching without broad API leakage.  │
  │ `AecDelivery`                │ NOT justified — current wiring splits ownership with `Node`.   │
  │ `AecFact`                    │ Justified — final state correctly collapses `AecEvent`/fact.  │
  └──────────────────────────────┴────────────────────────────────────────────────────────────────┘

  ### Reference Locations

  ┌──────────────────────────────────────────────────────────┬──────────────────────────────────────┐
  │ Change                                                   │ Location                             │
  ├──────────────────────────────────────────────────────────┼──────────────────────────────────────┤
  │ New service boundary introduced                          │ node/src/consensus/active_elections  │
  │                                                          │ /aec_service.rs:30                   │
  │ Activation ownership now routes through container API    │ node/src/consensus/active_elections  │
  │                                                          │ /active_elections_container.rs:108   │
  │ AEC facts are now the single event vocabulary            │ node/src/consensus/active_elections  │
  │                                                          │ /mod.rs:42                           │
  │ Event processor consumes facts directly                  │ node/src/aec_event_processor.rs:49   │
  └──────────────────────────────────────────────────────────┴──────────────────────────────────────┘

  Refs: node/src/consensus/active_elections/aec_service.rs:30, node/src/consensus/active_elections/active_elections_container.rs:108, node/src/consensus/active_elections/mod.rs:42, node/src/aec_event_processor.rs:49

  ### Open Questions / Assumptions

  - I’m inferring the intended end state is “AecService becomes the single AEC boundary” from the commit messages and resulting call-site moves.
  - I did not run the test suite; this is a repository-evidence review, not an execution-verified review.
  - I did not find surviving AecEvent/AecFact duplication at HEAD; that part of the branch does converge.

  ### Branch Summary

  - Target shape: move active-election reads, writes, and fact publication behind a single AEC boundary.
  - Improves: final HEAD removes the AecEvent/AecFact split, centralizes publication better than the base, and narrows several direct container call sites.
  - Debt remaining: delivery ownership is still split, and AecService remains a broad transitional facade rather than a steady-state boundary.

  ### Commit Sequence Notes

  - Converges cleanly: the late AecEvent to AecFact unification is a real simplification.
  - Temporary structure survives too long: AecService still contains explicitly temporary caller-shaped helpers at HEAD.
  - Reviewability risk: the sequence includes reverted test-harness detours and an immediate post-cleanup behavior restore (0bf6d9ac8995 -> 7192d362c35c).