#pragma once

#include <nano/lib/logging.hpp>
#include <nano/lib/numbers.hpp>
#include <nano/lib/stats.hpp>
#include <nano/node/blockprocessor.hpp>
#include <nano/node/fwd.hpp>

#include <chrono>
#include <deque>

namespace nano
{
class node;

class bootstrap_heuristic final
{
public:
	bootstrap_heuristic (nano::node & node, nano::stats & stats, nano::logger & logger);

	// Returns true if node appears to be bootstrapping
	bool is_bootstrapping () const;
	bool trigger_randomly (std::chrono::minutes min_interval, std::chrono::minutes max_interval) const;

	void start ();
	void stop ();

private: // Dependencies
	nano::node & node;
	nano::stats & stats;
	nano::logger & logger;

private:
	void process_batch (nano::block_processor::processed_batch_t const & batch);

	// Configuration
	static constexpr size_t window_size = 1000; // Number of blocks to analyze
	static constexpr double bootstrap_threshold = 0.1; // % of blocks from bootstrap to consider bootstrapping (mostly vs unchecked & live)

	std::atomic<bool> stopped{ false };
	mutable nano::mutex mutex;
	std::deque<nano::block_source> recent_blocks;
};
}