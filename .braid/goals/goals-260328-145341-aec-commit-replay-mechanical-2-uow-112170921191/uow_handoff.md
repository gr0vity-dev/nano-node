Current UoW: Unit B. Mutable ownership slice
Verdict: APPROVED for prior unit; next unit ready
Accepted shape: Unit A landed as commit `69b84906a4e0b8d39da0980dbad2a7e1d8658b4f` with rename-only vocabulary propagation across the active-election fact surface; later-unit files remain unstaged and the preserved replay diff is saved under `.braid/tmp/aec-replay-unit/`.
Exact next boundary: Complete Unit B only by staging the mutable-owner caller cluster so `AecService` becomes the sole mutable owner and mutator entrypoint, while excluding node tests that publish to `AecDelivery`, all `aec_ticker` read-traversal cleanup, all RPC `confirmation_active` cleanup, and any additional rename churn.
