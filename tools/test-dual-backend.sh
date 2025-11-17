#!/usr/bin/env bash
set -euo pipefail

echo "Running workspace tests with LMDB backend"
cargo test --workspace "$@"

echo "Running workspace tests with RocksDB backend"
RSNANO_TEST_LEDGER_BACKEND=rocksdb cargo test --workspace "$@"
