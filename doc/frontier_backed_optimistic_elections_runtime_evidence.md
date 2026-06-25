# Frontier-Backed Optimistic Elections Runtime Evidence

Date: 2026-06-24
Checkout: `/Users/bl/Git/pwo_nano-node/.braid/goals/worktrees/frontier-backed-optimistic-elections-dual-plan-6c4b411b21d1`
Base commit: `84017e243a861ecd1dd78a20ded138af20c4e56e`
Docker image: `pwo-nano-frontier:codex-20260624`
Docker image id: `sha256:8e8e90541063ebb140d3505f5d6f8035a6f73539b99dc962ee8cba8208865a34`

## Bootstrap Phase Config

The nanolab override is recorded at:

`nanolab_frontier_optimistic_experiment.override.yml`

Frontier-backed optimistic scheduling is enabled by default. While the ledger
is below the seeded bootstrap confirmation height (`bootstrap_weights.max_blocks`),
the node starts only the frontier-backed optimistic scheduler and keeps the
normal priority, hinted, and local-gap optimistic schedulers stopped. At or
above that height, frontier-backed activation stops and normal schedulers start.

The override below records the original runtime evidence settings and may still
be used to tune backlog and retry values:

```toml
[node.priority_scheduler]
enable = false

[node.hinted_scheduler]
enable = false

[node.optimistic_scheduler]
enable = false

[node.frontier_optimistic]
max_backlog = 65536
retry_interval = 250

[node.active_elections]
enable = true

[node.bootstrap]
enable_frontier_scan = true
enable_topo_scan = true

[node.diagnostics.optimistic_election]
enable = true
```

## RPC Evidence Commands

Counters:

```sh
curl -sS -X POST http://127.0.0.1:7076 -H 'Content-Type: application/json' -d '{"action":"stats","type":"counters"}'
```

Samples:

```sh
curl -sS -X POST http://127.0.0.1:7076 -H 'Content-Type: application/json' -d '{"action":"stats","type":"samples"}'
```

Expected bounded counter details under `optimistic_election`:

```text
frontier_scheduler_enabled
frontier_scheduler_disabled_after_bootstrap
frontier_peer_seen
frontier_local_head_matched
frontier_block_present
frontier_verified
frontier_started
frontier_already_active
frontier_already_confirmed
frontier_stale_missing
frontier_retry
frontier_backlog_full
frontier_backlog_insert
frontier_backlog_remove
frontier_dropped
```

Expected bounded sample names:

```text
frontier_candidate_age
frontier_retry_count
frontier_optimistic_election_duration
frontier_cemented_depth
frontier_accounts_touched
frontier_same_account_blocks
frontier_other_account_blocks
frontier_target_height_gap
```

## Prometheus Evidence Commands

Counters:

```promql
nano_stats_counters{type="optimistic_election"}
```

Samples:

```promql
nano_stats_samples_count{sample=~"frontier_.*"}
```

No account, block hash, root, representative, or endpoint values are introduced as stat keys or labels by this implementation.

## Grafana Evidence

Surface: `http://127.0.0.1:42005`

The same Nano stats counters and samples should be visible through the existing Prometheus-backed Nano stats dashboards after the node is running with the override above.

## Verification Attempt

Build and focused production-path test evidence:

```sh
git submodule update --init --recursive
cmake -S . -B build -DBUILD_TESTING=ON
cmake --build build --target core_test -j2
build/core_test --gtest_filter='bootstrap.*frontier*:frontier_optimistic.*:optimistic_scheduler.*:stats.*optimistic*:toml.*frontier*:enums.frontier_optimistic_stat_names'
cmake --build build --target rpc_test -j2
build/rpc_test --gtest_filter='rpc.stats_frontier_optimistic'
git diff --check
```

Result:

```text
Submodules initialized successfully.
CMake configuration completed successfully.
core_test target built successfully.
Focused core tests passed: 10 tests from 5 test suites.
rpc_test target built successfully.
Focused RPC stats test passed: 1 test from 1 test suite.
git diff --check completed without whitespace errors.
```

## Live Nanolab Runtime Evidence

The image was built from this checkout:

```sh
docker build -f docker/node/Dockerfile -t pwo-nano-frontier:codex-20260624 .
```

Result:

```text
Successfully built image id sha256:8e8e90541063ebb140d3505f5d6f8035a6f73539b99dc962ee8cba8208865a34
```

The requested nanolab entrypoint was attempted:

```sh
cd /Users/bl/Git/nanolab_playground
PATH=/Users/bl/Git/nanolab_playground/_bin:/Users/bl/Library/Python/3.9/bin:$PATH \
  nanolab run -t live_local_prom_bootstrap -i pwo-nano-frontier:codex-20260624
```

The command generated `/Users/bl/Git/nanolab_playground/nano_nodes/docker-compose.yml`, then stalled after the remote testcase fetch failed:

```text
https://api.github.com/repos/gr0vity-dev/nanolab-configs/contents/default/live_local_prom_bootstrap.json
Error retrieving the file...
Skip Fetching 'live_local_prom_bootstrap_config.json' ...
```

The generated compose stack was started directly after applying the experiment settings above to
`/Users/bl/Git/nanolab_playground/nano_nodes/ns_genesis/NanoTest/config-node.toml`:

```sh
docker compose -f nano_nodes/docker-compose.yml up -d --build
docker compose -f nano_nodes/docker-compose.yml restart ns_genesis ns_genesis_exporter
```

Runtime stack:

```text
nl_grafana            grafana/grafana:8.3.2        Up 0.0.0.0:42005->3000
nl_prometheus         prom/prometheus:latest       Up 0.0.0.0:42090->9090
nl_pushgateway        prom/pushgateway:latest      Up 0.0.0.0:42091->9091
ns_genesis            nano_nodes-ns_genesis        Up 0.0.0.0:7076->17076
ns_genesis_exporter   gr0v1ty/nano-prom-exporter   Up
```

Running node image check:

```text
docker inspect ns_genesis --format '{{.Image}} {{.Config.Image}}'
sha256:185ed544319170694d81c867106c903d737130ff59f9c8cd59f1fdb276e09fa8 nano_nodes-ns_genesis
```

The wrapper image was built by nanolab from `pwo-nano-frontier:codex-20260624`.

RPC version:

```json
{
  "rpc_version": "1",
  "store_version": "26",
  "protocol_version": "22",
  "node_vendor": "Nano DEV_BUILD",
  "store_vendor": "LMDB 0.9.70",
  "network": "live",
  "network_identifier": "991CF190094C00F0B68E2E5F75F6BEE95A2E0BD93CEAA4A6734DB9F19B728948",
  "build_info": "\"GNU C++ version \" \"13.3.0\" \"BOOST 108700\" BUILT \"Jun 24 2026\""
}
```

RPC frontier counter subset after the restart:

```text
optimistic_election	frontier_scheduler_enabled	in	1
optimistic_election	frontier_peer_seen	in	197000
optimistic_election	frontier_local_head_matched	in	4203
optimistic_election	frontier_block_present	in	4203
optimistic_election	frontier_verified	in	4203
optimistic_election	frontier_started	in	500
optimistic_election	frontier_retry	in	14
optimistic_election	frontier_backlog_insert	in	3401
optimistic_election	frontier_backlog_remove	in	500
```

RPC frontier samples:

```text
frontier_candidate_age		0	600000
frontier_retry_count		0	1024
```

Prometheus frontier counter query:

```promql
nano_stats_counters{type="optimistic_election",detail=~"frontier_.*"}
```

Result:

```json
{"metric":{"__name__":"nano_stats_counters","detail":"frontier_peer_seen","dir":"in","instance":"ns_genesis","job":"nanolab_live_local","type":"optimistic_election"},"value":[1782331890.515,"150000"]}
{"metric":{"__name__":"nano_stats_counters","detail":"frontier_block_present","dir":"in","instance":"ns_genesis","job":"nanolab_live_local","type":"optimistic_election"},"value":[1782331890.515,"3382"]}
{"metric":{"__name__":"nano_stats_counters","detail":"frontier_local_head_matched","dir":"in","instance":"ns_genesis","job":"nanolab_live_local","type":"optimistic_election"},"value":[1782331890.515,"3382"]}
{"metric":{"__name__":"nano_stats_counters","detail":"frontier_verified","dir":"in","instance":"ns_genesis","job":"nanolab_live_local","type":"optimistic_election"},"value":[1782331890.515,"3382"]}
{"metric":{"__name__":"nano_stats_counters","detail":"frontier_scheduler_enabled","dir":"in","instance":"ns_genesis","job":"nanolab_live_local","type":"optimistic_election"},"value":[1782331890.515,"1"]}
{"metric":{"__name__":"nano_stats_counters","detail":"frontier_backlog_insert","dir":"in","instance":"ns_genesis","job":"nanolab_live_local","type":"optimistic_election"},"value":[1782331890.515,"3365"]}
{"metric":{"__name__":"nano_stats_counters","detail":"frontier_backlog_remove","dir":"in","instance":"ns_genesis","job":"nanolab_live_local","type":"optimistic_election"},"value":[1782331890.515,"500"]}
{"metric":{"__name__":"nano_stats_counters","detail":"frontier_started","dir":"in","instance":"ns_genesis","job":"nanolab_live_local","type":"optimistic_election"},"value":[1782331890.515,"500"]}
{"metric":{"__name__":"nano_stats_counters","detail":"frontier_retry","dir":"in","instance":"ns_genesis","job":"nanolab_live_local","type":"optimistic_election"},"value":[1782331890.515,"14"]}
```

Prometheus sample-count query:

```promql
nano_stats_samples_count{sample=~"frontier_.*|optimistic_election_.*"}
```

Result:

```json
{"metric":{"__name__":"nano_stats_samples_count","instance":"ns_genesis","job":"nanolab_live_local","sample":"frontier_candidate_age"},"value":[1782331897.700,"500"]}
{"metric":{"__name__":"nano_stats_samples_count","instance":"ns_genesis","job":"nanolab_live_local","sample":"frontier_retry_count"},"value":[1782331897.700,"500"]}
```

Prometheus bounded-label proof:

```text
nano_stats_counters labels for optimistic_election: __name__, detail, dir, instance, job, type
nano_stats_samples_count labels for frontier samples: __name__, instance, job, sample
```

No account, block hash, root, representative, or endpoint labels appeared in the live optimistic/frontier Prometheus series.

Grafana proof:

```text
GET http://127.0.0.1:42005/api/health
{"commit":"afb9e8e5f3","database":"ok","version":"8.3.2"}

GET http://127.0.0.1:42005/api/datasources
{"name":"LocalNode","type":"prometheus","url":"http://nl_prometheus:9090","access":"proxy","isDefault":true}

GET http://127.0.0.1:42005/api/search?query=nano
{"title":"Nano Stats & Counters","type":"dash-db","url":"/d/QjjDzXZSk/nano-stats-and-counters","uri":"db/nano-stats-and-counters"}
{"title":"Nano Stats & Counters (+ Histograms V2)","type":"dash-db","url":"/d/nano-overview-histograms-v2/nano-stats-and-counters-histograms-v2","uri":"db/nano-stats-and-counters-histograms-v2"}
```

## Nanolab Runtime Memo

Build a Docker image from this goal worktree before collecting proof. The public `nanocurrency/nano:V28.2` image does not contain these frontier-backed changes unless it has been replaced by a locally built/tagged image from this checkout.

Known nanolab entrypoint:

```sh
cd /Users/bl/Git/nanolab_playground
PATH=/Users/bl/Git/nanolab_playground/_bin:/Users/bl/Library/Python/3.9/bin:$PATH \
  nanolab run -t live_local_prom_bootstrap -i <local-image-built-from-this-worktree>
```

Useful URLs after startup:

```text
Nano RPC:    http://127.0.0.1:7076
Grafana:     http://127.0.0.1:42005
Prometheus:  http://127.0.0.1:42090
Pushgateway: http://127.0.0.1:42091
```

Quick smoke check:

```sh
curl -sS -X POST http://127.0.0.1:7076 \
  -H 'Content-Type: application/json' \
  -d '{"action":"version"}'
```

## Follow-up Runtime Evidence - 2026-06-24 22:40 CEST

Follow-up image built from this checkout after adding the B6 sample writers:

```text
pwo-nano-frontier:codex-20260624-followup sha256:354abf1c79cc453b665a822706e01d20ff0760da7b48e6435ba6ab23d43ed624
pwo-nano-frontier:codex-20260624 retagged to the same image for the generated nanolab compose file
nanomock-ns_genesis rebuilt image sha256:6cdee1e46141875528359ebb58619a050cf9023f5b95795d3e88aba77978d3da
running ns_genesis image sha256:6cdee1e46141875528359ebb58619a050cf9023f5b95795d3e88aba77978d3da
```

The mounted nanolab node config at `/Users/bl/Git/nanolab_playground/nano_nodes/ns_genesis/NanoTest/config-node.toml` was checked and updated to include the experiment TOML shown above. After `docker compose -p nanomock up -d --force-recreate ns_genesis ns_genesis_exporter`, RPC reported the rebuilt node:

```json
{
  "rpc_version": "1",
  "store_version": "26",
  "protocol_version": "22",
  "node_vendor": "Nano DEV_BUILD",
  "store_vendor": "RocksDB 10.4.2",
  "network": "live",
  "build_info": "GNU C++ version 13.3.0 BOOST 108700 BUILT Jun 24 2026"
}
```

Frontier-only bootstrap counter subset from RPC `stats` counters after the restart:

```text
election	confirmation_request	in	3374
optimistic_election	frontier_scheduler_enabled	in	1
optimistic_election	frontier_peer_seen	in	813000
optimistic_election	frontier_local_head_matched	in	24355
optimistic_election	frontier_block_present	in	24355
optimistic_election	frontier_verified	in	24355
optimistic_election	frontier_started	in	1000
optimistic_election	frontier_retry	in	5683
optimistic_election	frontier_backlog_insert	in	8086
optimistic_election	frontier_backlog_remove	in	1000
```

Zero normal-start/drop check from the same RPC counter response. These details were absent from the response and therefore read as zero:

```text
optimistic_election	priority_started	0
optimistic_election	hinted_started	0
optimistic_election	optimistic_started	0
optimistic_election	frontier_dropped	0
optimistic_election	disabled_after_bootstrap	0
```

RPC `stats` samples listed all required B6 sample names and bounded min/max ranges. The exporter consumes sample values via the same drain-on-read RPC path, so the direct RPC value arrays were empty at this instant while Prometheus retained the exported sample counts:

```text
active_election_duration	0	600000	0
frontier_candidate_age	0	600000	0
frontier_retry_count	0	1024	0
frontier_optimistic_election_duration	0	3600000	0
frontier_cemented_depth	0	1048576	0
frontier_accounts_touched	0	1024	0
frontier_same_account_blocks	0	1048576	0
frontier_other_account_blocks	0	1048576	0
frontier_target_height_gap	0	1048576	0
```

Prometheus frontier counter query:

```promql
nano_stats_counters{type="optimistic_election",detail=~"frontier_.*"}
```

Result excerpt:

```json
{"metric":{"__name__":"nano_stats_counters","detail":"frontier_peer_seen","dir":"in","instance":"ns_genesis","job":"nanolab_live_local","type":"optimistic_election"},"value":"739000"}
{"metric":{"__name__":"nano_stats_counters","detail":"frontier_block_present","dir":"in","instance":"ns_genesis","job":"nanolab_live_local","type":"optimistic_election"},"value":"22068"}
{"metric":{"__name__":"nano_stats_counters","detail":"frontier_local_head_matched","dir":"in","instance":"ns_genesis","job":"nanolab_live_local","type":"optimistic_election"},"value":"22068"}
{"metric":{"__name__":"nano_stats_counters","detail":"frontier_verified","dir":"in","instance":"ns_genesis","job":"nanolab_live_local","type":"optimistic_election"},"value":"22068"}
{"metric":{"__name__":"nano_stats_counters","detail":"frontier_scheduler_enabled","dir":"in","instance":"ns_genesis","job":"nanolab_live_local","type":"optimistic_election"},"value":"1"}
{"metric":{"__name__":"nano_stats_counters","detail":"frontier_backlog_insert","dir":"in","instance":"ns_genesis","job":"nanolab_live_local","type":"optimistic_election"},"value":"8086"}
{"metric":{"__name__":"nano_stats_counters","detail":"frontier_backlog_remove","dir":"in","instance":"ns_genesis","job":"nanolab_live_local","type":"optimistic_election"},"value":"1000"}
{"metric":{"__name__":"nano_stats_counters","detail":"frontier_started","dir":"in","instance":"ns_genesis","job":"nanolab_live_local","type":"optimistic_election"},"value":"1000"}
{"metric":{"__name__":"nano_stats_counters","detail":"frontier_retry","dir":"in","instance":"ns_genesis","job":"nanolab_live_local","type":"optimistic_election"},"value":"5683"}
```

Prometheus B6 sample-count query:

```promql
nano_stats_samples_count{sample=~"frontier_.*"}
```

Result excerpt:

```json
{"metric":{"__name__":"nano_stats_samples_count","instance":"ns_genesis","job":"nanolab_live_local","sample":"frontier_candidate_age"},"value":"1000"}
{"metric":{"__name__":"nano_stats_samples_count","instance":"ns_genesis","job":"nanolab_live_local","sample":"frontier_retry_count"},"value":"1000"}
{"metric":{"__name__":"nano_stats_samples_count","instance":"ns_genesis","job":"nanolab_live_local","sample":"frontier_accounts_touched"},"value":"1000"}
{"metric":{"__name__":"nano_stats_samples_count","instance":"ns_genesis","job":"nanolab_live_local","sample":"frontier_cemented_depth"},"value":"1000"}
{"metric":{"__name__":"nano_stats_samples_count","instance":"ns_genesis","job":"nanolab_live_local","sample":"frontier_optimistic_election_duration"},"value":"1000"}
{"metric":{"__name__":"nano_stats_samples_count","instance":"ns_genesis","job":"nanolab_live_local","sample":"frontier_other_account_blocks"},"value":"1000"}
{"metric":{"__name__":"nano_stats_samples_count","instance":"ns_genesis","job":"nanolab_live_local","sample":"frontier_same_account_blocks"},"value":"1000"}
{"metric":{"__name__":"nano_stats_samples_count","instance":"ns_genesis","job":"nanolab_live_local","sample":"frontier_target_height_gap"},"value":"1000"}
```

Grafana proof with the Prometheus datasource:

```text
GET /api/health -> {"commit":"afb9e8e5f3","database":"ok","version":"8.3.2"}
GET /api/datasources as admin -> LocalNode prometheus http://nl_prometheus:9090 default=true
GET /api/search?query=nano as admin -> Nano Stats & Counters dashboards present
POST /api/ds/query as admin, datasource uid P56A86A2655F58D21, expr nano_stats_samples_count{sample=~"frontier_.*"}
```

Grafana datasource query result excerpt:

```text
frontier_candidate_age	2000
frontier_retry_count	2000
frontier_accounts_touched	2000
frontier_cemented_depth	2000
frontier_optimistic_election_duration	2000
frontier_other_account_blocks	2000
frontier_same_account_blocks	2000
frontier_target_height_gap	2000
```

Bounded-label proof remains satisfied: the live Prometheus frontier counter labels are `__name__`, `detail`, `dir`, `instance`, `job`, and `type`; the sample-count labels are `__name__`, `instance`, `job`, and `sample`. No account, block hash, root, representative, or endpoint labels appear in the frontier series.

The first live run did not reach the production live-network bootstrap threshold because the hardcoded live threshold is `207107949` blocks. The follow-up below uses the same nanolab live node, ledger, config, RPC, Prometheus, and Grafana stack with a controlled low-threshold harness binary so `ledger.bootstrap_height_reached()` flips during the run.

## Low-Threshold Nanolab B5 Runtime Evidence - 2026-06-24 22:54 CEST

Harness boundary:

```text
Source feature code: unchanged from this checkout.
Runtime container: ns_genesis from the existing nanomock nanolab stack.
Harness patch: copied /usr/bin/nano_node from ns_genesis and replaced the single little-endian uint64 occurrence of live max_blocks 207107949 with 1800000.
Patch proof: the original binary contained the 207107949 threshold once; the patched binary changed only that harness threshold constant.
Reason: accepted low-threshold nanolab proof for the bootstrap-height transition while preserving the production frontier activation path.
```

Before the transition, the restarted low-threshold nanolab node was below the harness threshold and the frontier scheduler was the active bootstrap scheduler:

```text
RPC version: Nano DEV_BUILD, network live, RocksDB 10.4.2, built Jun 24 2026
RPC block_count: count=1755405 unchecked=0 cemented=647895
Harness threshold: 1800000
RPC counters:
optimistic_election frontier_scheduler_enabled in 1
```

After the node crossed the harness threshold, RPC counters showed frontier-backed activation shutting off and classifying new verified candidates as disabled-after-bootstrap:

```text
RPC block_count: count=1810345 unchecked=26630 cemented=647895
RPC counters:
optimistic_election frontier_scheduler_disabled_after_bootstrap in 4433
optimistic_election frontier_verified in 6517
optimistic_election frontier_started in 978
optimistic_election frontier_dropped in 0 (absent from counters)
```

The scheduler object state from RPC `stats` type `objects` showed normal schedulers restored after the threshold:

```json
{
  "frontier_bootstrap_mode": { "count": "0", "size": "0" },
  "frontier_scheduler_started": { "count": "0", "size": "0" },
  "normal_schedulers_started": { "count": "1", "size": "0" }
}
```

No new frontier-backed starts occurred after the threshold. A delayed second read showed `frontier_started` unchanged while disabled-after-bootstrap continued to rise:

```text
RPC block_count: count=1815429 unchecked=27854 cemented=647895
RPC counters:
optimistic_election frontier_scheduler_disabled_after_bootstrap in 6167
optimistic_election frontier_verified in 8251
optimistic_election frontier_started in 978
optimistic_election frontier_dropped in 0 (absent from counters)
```

Prometheus observed the same after-threshold state through the existing exporter:

```promql
nano_stats_counters{type="optimistic_election",detail=~"frontier_scheduler_disabled_after_bootstrap|frontier_started|frontier_dropped"}
```

Result:

```text
frontier_started 978
frontier_scheduler_disabled_after_bootstrap 6503
frontier_dropped absent, therefore zero
```

B5 result: satisfied by low-threshold nanolab runtime proof. The node crossed `ledger.bootstrap_height_reached()`, frontier-backed activation turned off and reported `frontier_scheduler_disabled_after_bootstrap`, `frontier_started` did not increase after the threshold, and normal scheduler state reported enabled.
