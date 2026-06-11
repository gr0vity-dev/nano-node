# Testing Without Mocks Test Inventory

Date: 2026-06-10

Scope: every workspace crate in `Cargo.toml`, with crate-level test inventory and
TWM KPI scoring. This is a repository-wide first pass, not a replacement for
line-by-line review of every assertion. Scores are evidence-based bands derived
from current test files, representative manual samples, and static signals.

## KPI Rubric

Each testcase can be scored from 0 to 12 using six KPIs, 0 to 2 each.

| KPI | 2 | 1 | 0 |
| --- | --- | --- | --- |
| Real production path | Runs actual component and production collaborators | Runs component but bypasses part of workflow | Mostly tests helper/fake behavior |
| Boundary stop | Stops only at true infrastructure boundary | Stops at broad fixture or mixed seam | No clear boundary |
| Nullable compliance | Official wrapper null mode, same public API | Nullable-shaped but has runtime branches or side channels | Test fake/mock/stub owns behavior |
| State assertions | Observable returned, persisted, emitted, or tracked state | Mixed state and structural checks | Interaction/call choreography |
| Determinism | No wall clock, sleep, thread timing, or polling | Bounded polling or injected time with rough edges | Real sleeps or racy timing authority |
| Scope locality | One behavior with clear failure locality | Medium integration scope | Broad smoke/E2E authority |

Score bands used below:

| Band | Meaning |
| --- | --- |
| 10-12 Strong | Strong TWM fit; usually pure logic, value object, or narrow sociable test |
| 7-9 Good/Mixed | Useful TWM coverage but has nullable parity, setup, or runtime-branch concerns |
| 4-6 Smoke/System | Valuable regression coverage, weak as primary TWM authority |
| 0-3 Weak/Missing | Missing direct tests or tests mostly prove fake/helper behavior |
| N/A | No direct tests found in the crate |

## Repository Test Shape

```text
workspace tests
├─ pure/value tests
│  ├─ types
│  ├─ messages
│  ├─ network_protocol
│  └─ ledger/block_insertion/validation
├─ nullable wrapper tests
│  ├─ nullables/*
│  ├─ store_lmdb over nullable LMDB
│  └─ ledger over nullable LMDB
├─ narrow sociable application tests
│  ├─ node subcomponents that use new_null constructors
│  ├─ wallet fragments
│  └─ cli command helpers
└─ broad smoke/system tests
   ├─ node/tests via System::make_node
   ├─ rpc/server/tests via setup_rpc_client_and_server
   ├─ websocket/server tests
   └─ network tests with real TCP or fake channels
```

## Workspace Crate Inventory

Static signal columns:

- `tests`: count of `#[test]` / `#[tokio::test]` in the crate.
- `test files`: files with explicit tests, `#[cfg(test)]`, or integration-test
  path placement.
- `signals`: dominant static indicators that influenced the score.

| Crate | Test files | Tests | Dominant TWM score | Behavior and evidence |
| --- | ---: | ---: | --- | --- |
| `rsnano_output_tracker` (`nullables/output_tracker`) | 2 | 10 | 8/12 Good/Mixed | Output Tracking primitive. Mostly state-based tracker output. Signal noise includes generic `network` words from names/types, not actual network I/O. Primary check: tracker records behavior objects rather than method-call choreography. |
| `rsnano_nullable_fs` (`nullables/fs`) | 1 | 13 | 8/12 Good/Mixed | Real filesystem wrapper with `new_null()` and builder configuration. Good external-boundary ownership for filesystem writes/reads. Concern: public `track()` and builder methods are powerful side channels; accept only when treated as Output Tracking/configuration, not test-only behavior injection. |
| `rsnano_nullable_tcp` (`nullables/tcp`) | 2 | 10 | 7/12 Good/Mixed | TCP wrapper owns socket/stream boundary and offers null streams. Good boundary placement. Concern: many stub/null constructors and configured input paths need parity proof against real stream behavior. |
| `rsnano_nullable_lmdb` (`nullables/lmdb`) | 2 | 15 | 7/12 Good/Mixed | Official LMDB wrapper null mode disables LMDB/filesystem persistence and backs higher-level ledger/store tests. Concern: public methods branch on `EnvironmentStrategy` and some real/null operations are no-ops or `todo!`, so parity coverage is incomplete. |
| `rsnano_nullable_http_client` (`nullables/http_client`) | 1 | 9 | 8/12 Good/Mixed | Strong wrapper intent: real narrow HTTP test, configurable responses, and Output Tracking for requests. Concern: `Response::error_for_status()` has real/null branches and different error text, so nullable parity is not fully proven. |
| `rsnano_nullable_clock` (`nullables/clock`) | 2 | 9 | 8/12 Good/Mixed | Good time boundary ownership with configurable null time. Some real-clock tests use sleep to prove real behavior; acceptable as low-level wrapper integration, but high-level tests should use configured time. |
| `rsnano_nullable_random` (`nullables/random`) | 1 | 6 | 10/12 Strong | Clear deterministic random boundary. Tests are narrow and value/output oriented. |
| `rsnano_nullable_console` (`nullables/console`) | 1 | 3 | 8/12 Good/Mixed | Console output wrapper with output tracking. Good side-effect boundary when assertions inspect recorded output events. |
| `rsnano_nullable_tracing_subscriber` (`nullables/tracing_subscriber`) | 1 | 1 | 7/12 Good/Mixed | Narrow tracing initialization seam. Limited coverage; should stay wrapper-level, not become broad global-state authority. |
| `rsnano_nullable_env` (`nullables/env`) | 1 | 3 | 10/12 Strong | Simple environment variable boundary with deterministic null behavior. Narrow, low risk. |
| `rsnano_nullable_condvar` (`nullables/condvar`) | 1 | 15 | 7/12 Good/Mixed | Owns synchronization boundary. Null mode supports deterministic tests, but real condvar checks use threads/sleeps. Keep as low-level wrapper integration; avoid this timing shape in higher layers. |
| `rsnano_types` (`types`) | 30 | 126 | 10/12 Strong | Mostly value objects, block builders, signatures, amounts, timestamps, and serialization-like behavior. Strong TWM fit: no true infrastructure boundary, narrow state assertions. Watch for occasional real time helpers but most tests are pure/value. |
| `rsnano_utils` (`utils`) | 11 | 40 | 6/12 Smoke/System | Mixed crate: pure containers/stats plus thread pools, tickers, backpressure, cancellation. Timing/thread tests lower determinism. Split future scoring by module: pure helpers should score 10-12, runtime/thread wrappers should be wrapper integration only. |
| `rsnano_work` (`work`) | 4 | 23 | 6/12 Smoke/System | Work generation includes CPU/thread behavior and timing. Pure threshold/xorshift tests are stronger; work pool/thread tests are timing-sensitive and should be treated as integration/regression coverage. |
| `rsnano_messages` (`messages`) | 18 | 88 | 9/12 Good/Mixed | Message serialization/deserialization and protocol values are mostly narrow state-based tests. Minor timing/network signal is domain naming, not necessarily live I/O. Good candidate for per-testcase scores near 10. |
| `rsnano_network` (`network`) | 6 | 40 | 5/12 Smoke/System | Tests mix network containers with channel/TCP adapter behavior. Many timing and network signals. `Network::new_null()` helps, but real TCP/channel lifecycle tests should be classified as narrow integration or smoke, not pure TWM authority. |
| `rsnano_rpc_messages` (`rpc/messages`) | 94 | 316 | 9/12 Good/Mixed | Large set of request/response message DTO tests. Mostly value/serialization behavior with low infrastructure risk. Some storage/time words are domain fields rather than direct I/O. |
| `rsnano_rpc_client` (`rpc/client`) | 0 | 0 | N/A | No direct tests found. Needs either narrow HTTP-client wrapper tests at the client boundary or explicit reliance on higher-level RPC server smoke tests. |
| `rsnano_rpc_server` (`rpc/server`) | 102 | 201 | 4/12 Smoke/System | Integration suite uses `System::make_node()` and `setup_rpc_client_and_server()`. Good user-visible regression coverage, but broad fixture, real runtime, real bound ports, and polling make it weak primary TWM authority. Command handlers need narrower sociable tests around nulled node/ledger/wallet seams. |
| `rsnano_websocket_messages` (`websocket/messages`) | 0 | 0 | N/A | No direct tests found. If messages remain simple DTOs, add value/serialization tests or document coverage through websocket server/client tests. |
| `rsnano_websocket_client` (`websocket/client`) | 0 | 0 | N/A | No direct tests found. Needs wrapper-level tests or explicit smoke-only status. |
| `rsnano_websocket_server` (`websocket/server`) | 2 | 16 | 5/12 Smoke/System | Websocket listener/session behavior involves network/runtime timing. Valuable integration coverage but should be backed by narrower message factory/options tests and nullable network/session seams. |
| `rsnano_store_lmdb` (`store_lmdb`) | 14 | 71 | 8/12 Good/Mixed | Store tests commonly use `rsnano_nullable_lmdb`, so they are sociable over the storage wrapper. Stronger than raw mocks. Main gap is parity with real LMDB for low-level wrapper behavior and avoiding duplicated store setup assumptions. |
| `rsnano_ledger` (`ledger`) | 33 | 165 | 9/12 Good/Mixed | Strong mix: pure validation tests score near 11, ledger tests over `Ledger::new_null()` score near 9. Gaps are helper setup that manually fills production internals and reliance on nullable LMDB parity. |
| `rsnano_wallet` (`wallet`) | 2 | 3 | 7/12 Good/Mixed | Limited direct tests. Wallet code touches ledger/storage/filesystem concepts, so current direct coverage is thin. Needs more narrow sociable tests with real wallet logic and nulled storage/fs boundaries. |
| `rsnano_node` (`node`) | 118 | 774 | 5/12 Smoke/System | Largest suite. Contains many valuable subcomponent tests with `new_null()`, but also many broad `System` tests, sleeps, `assert_timely`, real network setup, fake channels, and lifecycle threads. Future inventory should split node into per-module ratings. |
| `rsnano_cli` (`cli`) | 3 | 5 | 7/12 Good/Mixed | Sparse tests around command behavior and nullable filesystem/storage. Needs more command-level state/output assertions over nullable fs/lmdb. |
| `rsnano_daemon` (`daemon`) | 0 | 0 | N/A | No direct tests found. Daemon owns broad lifecycle, RPC, websocket, and callbacks; should have one or two smoke tests plus narrower tests for lifecycle decision logic and nullable HTTP callback behavior. |
| `rsnano_network_protocol` (`network_protocol`) | 3 | 28 | 10/12 Strong | Protocol/handshake/receiver logic appears mostly deterministic and state-based. Network terms are domain/protocol naming, not necessarily live sockets. |
| `record-rep-weights` (`tools/record-rep-weights`) | 0 | 0 | N/A | No direct tests found. Tool likely relies on node/ledger behavior; if retained, add narrow parsing/output tests or mark as manually exercised utility. |
| `nanospam` (`tools/nanospam`) | 4 | 18 | 7/12 Good/Mixed | Domain logic tests exist, but timing/lifecycle signals appear. Strongest tests should live under `domain/*`; setup/launcher/network pieces need nullable seams or smoke-only classification. |
| `test_helpers` (`tools/test_helpers`) | 1 | 2 | 4/12 Smoke/System | Test infrastructure, not production behavior. Contains broad `System`, polling helpers, real filesystem cleanup, and fake channel construction. Use as fixture support only; do not treat helper tests as TWM proof for production behavior. |
| `signature-checker` (`tools/signature-checker`) | 0 | 0 | N/A | No direct tests found. It touches LMDB and threaded work; add narrow tests around signature validation flow or document as external tool smoke-only. |
| `rsnano-insight` (`tools/insight`) | 6 | 15 | 5/12 Smoke/System | GUI/app tool with nullable runtime pieces and some domain model tests. Runtime/UI/network/lifecycle areas need clearer wrapper seams; pure view-model/model tests can score much higher. |
| `rep-weights-converter` (`tools/rep-weights-converter`) | 0 | 0 | N/A | No direct tests found. Add file conversion tests over nullable filesystem or temp real-file narrow integration. |

## Manual Calibration Samples

These samples anchor the score bands.

| Test area | Score | Reason |
| --- | ---: | --- |
| `ledger/src/block_insertion/validation/tests/validate_state_send.rs` | 11/12 | Pure logic, fixed timestamp, returned `BlockInsertInstructions`, state assertions. Minor concern: helper manually constructs `BlockValidator` inputs. |
| `ledger/src/ledger_tests/empty_ledger.rs` | 9/12 | Real `Ledger` over nulled LMDB and state assertions. Depends on nullable LMDB parity. |
| `nullables/http_client/src/lib.rs` | 8/12 | Real narrow HTTP test plus null configurable responses and request Output Tracking. Real/null response behavior diverges in error text. |
| `rpc/server/tests/tests/node/uptime.rs` | 4/12 | Broad RPC/node fixture, real sleep, real runtime/server, weak `> 0` assertion. |
| `node/tests/tests/network.rs::last_contacted` | 4/12 | Real TCP, polling, race-handling comments, broad node fixture. Good smoke test, weak TWM authority. |
| `node/tests/tests/network.rs` fake-channel vote paths | 5/12 | Enters production inbound queue but uses `make_fake_channel()` side-channel. Needs production-owned behavior simulation seam. |

## Common Rewrite Paths

| Current pattern | TWM replacement mechanism | Owner |
| --- | --- | --- |
| `System::make_node()` for command/handler correctness | Narrow sociable test with real handler/service and nulled ledger/network/wallet wrappers | Production and tests |
| `setup_rpc_client_and_server()` for all RPC behavior | Keep one smoke path; add handler-level tests using production request parsing and nulled application seams | Production and tests |
| `sleep`, `assert_timely`, `Instant::now`, `SystemTime::now` | Injectable `SteadyClock` or `SystemTimeFactory` configured through official null constructor | Production seam |
| `make_fake_channel()` | Production-owned network/message behavior simulation API that feeds the same input path as real TCP | Production seam |
| Nullable wrapper with runtime mode branches | Initialization-selected private implementation/strategy with parity tests at wrapper boundary | Production wrapper |
| Output tracker recording method-like calls | Track domain behavior records: request sent, message broadcast, file written, block persisted | Tests and wrapper |
| High-level `new_null()` constructors assembling many nulled collaborators | Keep only if each disabled external path is named and owned; otherwise descend to narrower component tests | Production and tests |

## Gaps To Track

1. Missing direct tests: `rpc/client`, `websocket/messages`, `websocket/client`,
   `daemon`, `record-rep-weights`, `signature-checker`, and
   `rep-weights-converter`.
2. Nullable parity proof is incomplete for several wrappers. `LmdbEnvironment`,
   `HttpClient`, TCP, filesystem, clock, and condvar should each have explicit
   real/null parity tests for their public operational behavior.
3. Node and RPC suites are over-represented as broad smoke tests. They should
   remain as regression coverage, but most command, consensus, transport, and
   wallet behaviors need narrower sociable tests to become TWM authorities.
4. Timing helpers in `tools/test_helpers` are pervasive. They lower
   determinism scores unless the test is explicitly a low-level lifecycle or
   integration test.
5. Production-source `new_null()` constructors exist across application types,
   not only low-level infrastructure wrappers. Each one needs audit against the
   strict Nullable checklist: owner, disabled communication path, public method
   parity, init-only selection, no side channels, and parity proof.

## Verification Notes

Evidence gathered from:

- Workspace members in root `Cargo.toml`.
- All crate `Cargo.toml` package names.
- Rust files under each workspace member, excluding `target`.
- Test counts from `#[test]` and `#[tokio::test]`.
- Static signals for `new_null`, `null_builder`, `sleep`, `assert_timely`,
  `System::make_node`, `setup_rpc_client_and_server`, `make_fake_channel`,
  filesystem, network, LMDB/storage, and Output Tracking.
- Manual inspection of representative tests and wrapper implementations.

Atomic reasoning checks:

| Atom | Component/rule | Independence | Evidence status |
| --- | --- | --- | --- |
| Workspace coverage | Every member in root `Cargo.toml` appears in the inventory | Independent of score accuracy | Verified by crate table matching workspace members |
| Test presence | Test counts and test-file counts per crate | Independent of TWM interpretation | Verified by static scan of current `.rs` files |
| KPI definition | Six KPI dimensions from the initial audit | Independent scoring dimensions | Documented in rubric |
| Boundary classification | Pure/value, nullable seam, narrow sociable, broad smoke | Independent from individual score | Inferred from static signals plus manual samples |
| Nullable strictness | Nullables must be official wrapper modes with parity | Independent from current repo convention | Taken from `testing-without-mocks` skill |
| Residual risk | Static scan cannot prove every assertion shape | Independent from inventory completeness | Called out explicitly in scope and gaps |

