#include <nano/lib/blocks.hpp>
#include <nano/node/active_elections.hpp>
#include <nano/node/election.hpp>
#include <nano/node/node.hpp>
#include <nano/node/scheduler/bucket.hpp>

#include <chrono>

/*
 * minute_based_cps_limiter
 */

nano::scheduler::minute_based_cps_limiter::minute_based_cps_limiter (double baseline_cps, size_t bucket_count, double burst_multiplier) :
	baseline_cps{ baseline_cps },
	bucket_count{ bucket_count },
	burst_multiplier{ burst_multiplier },
	per_bucket_cps{ baseline_cps / bucket_count }
{
	// Calculate quotas for each bucket
	uint32_t baseline_quota = static_cast<uint32_t> (baseline_cps * 60.0) / bucket_count;
	uint32_t burst_quota = static_cast<uint32_t> (burst_multiplier * baseline_quota);

	// Initialize all bucket states
	for (auto & bucket : buckets)
	{
		bucket.burst_quota_per_minute = burst_quota;
	}
}

void nano::scheduler::minute_based_cps_limiter::update_minute_window (size_t bucket_id)
{
	debug_assert (bucket_id < buckets.size ());

	auto now = std::chrono::steady_clock::now ();
	auto & bucket_state = buckets[bucket_id];

	// Check if we need to start a new minute
	auto elapsed = std::chrono::duration_cast<std::chrono::minutes> (now - bucket_state.minute_start);
	if (elapsed.count () >= 1)
	{
		// Reset counters for new minute
		bucket_state.minute_start = now;
		bucket_state.confirmations_this_minute = 0;
	}
}

bool nano::scheduler::minute_based_cps_limiter::can_activate_now (size_t bucket_id, bool account_is_idle)
{
	debug_assert (bucket_id < buckets.size ());

	auto & state = buckets[bucket_id];
	auto now = std::chrono::steady_clock::now ();
	update_minute_window (bucket_id);

	// Never exceed baseline_cps for single bucket (max burst speed)
	auto time_since_last = std::chrono::duration<double> (now - state.last_activation).count ();
	if (time_since_last < (1.0 / baseline_cps))
	{
		return false; // Too soon - prevents spikes
	}

	// Condition 1: Anyone can use baseline rate
	double baseline_interval = 1.0 / per_bucket_cps; // 3.15 seconds at 20 CPS
	if (time_since_last >= baseline_interval)
	{
		state.last_activation = now;
		return true; // Use baseline rate - evenly distributed
	}

	// Condition 2: Burst requires only 2 conditions
	bool burst_quota_available = state.confirmations_this_minute < state.burst_quota_per_minute;

	if (burst_quota_available && account_is_idle)
	{
		state.last_activation = now;
		return true;
	}

	return false;
}

void nano::scheduler::minute_based_cps_limiter::on_confirmation (size_t bucket_id)
{
	debug_assert (bucket_id < buckets.size ());

	auto & state = buckets[bucket_id];
	auto now = std::chrono::steady_clock::now ();
	update_minute_window (bucket_id);
	state.confirmations_this_minute++;
}

/*
 * bucket
 */

nano::scheduler::bucket::bucket (nano::bucket_index index_a, priority_bucket_config const & config_a, nano::active_elections & active_a, nano::stats & stats_a) :
	index{ index_a },
	config{ config_a },
	active{ active_a },
	stats{ stats_a }
{
	// Initialize CPS rate limiter if baseline_cps is configured
	if (config.baseline_cps > 0.0)
	{
		rate_limiter = std::make_unique<minute_based_cps_limiter> (config.baseline_cps, 63, config.burst_multiplier); // 63 buckets
	}
}

nano::scheduler::bucket::~bucket ()
{
}

bool nano::scheduler::bucket::available () const
{
	nano::lock_guard<nano::mutex> lock{ mutex };

	if (queue.empty ())
	{
		return false;
	}
	else
	{
		return election_vacancy (queue.begin ()->time);
	}
}

bool nano::scheduler::bucket::election_vacancy (nano::priority_timestamp candidate) const
{
	debug_assert (!mutex.try_lock ());

	if (elections.size () < config.reserved_elections || elections.size () < config.max_elections)
	{
		return active.vacancy (nano::election_behavior::priority) > 0;
	}
	if (!elections.empty ())
	{
		auto lowest = elections.get<tag_priority> ().begin ()->priority;

		// Compare to equal to drain duplicates
		if (candidate <= lowest)
		{
			// Bound number of reprioritizations
			return elections.size () < config.max_elections * 2;
		};
	}
	return false;
}

bool nano::scheduler::bucket::election_overfill () const
{
	debug_assert (!mutex.try_lock ());

	if (elections.size () < config.reserved_elections)
	{
		return false;
	}
	if (elections.size () < config.max_elections)
	{
		return active.vacancy (nano::election_behavior::priority) < 0;
	}
	return true;
}

bool nano::scheduler::bucket::is_account_idle (uint64_t priority_timestamp) const
{
	// priority_timestamp is seconds since epoch (Unix timestamp)
	auto now_seconds = nano::seconds_since_epoch ();
	auto age_seconds = now_seconds > priority_timestamp ? now_seconds - priority_timestamp : 0;
	return age_seconds >= 3600; // 1 hour = 3600 seconds
}

bool nano::scheduler::bucket::activate ()
{
	nano::lock_guard<nano::mutex> lock{ mutex };

	if (queue.empty ())
	{
		return false; // Not activated
	}

	block_entry top = *queue.begin ();
	bool account_is_idle = is_account_idle (top.time);

	// Check rate limit BEFORE removing from queue
	if (rate_limiter && !rate_limiter->can_activate_now (index, account_is_idle))
	{
		stats.inc (nano::stat::type::election_bucket, nano::stat::detail::cps_rate_limited);
		return false; // Block stays in queue, try next iteration
	}

	queue.erase (queue.begin ());

	auto block = top.block;
	auto priority = top.time;

	auto erase_callback = [this] (std::shared_ptr<nano::election> election) {
		nano::lock_guard<nano::mutex> lock{ mutex };
		elections.get<tag_root> ().erase (election->qualified_root);

		// Track confirmations for Burst Quota
		if (rate_limiter)
		{
			rate_limiter->on_confirmation (index);
		}
	};

	auto result = active.insert (block, nano::election_behavior::priority, erase_callback);
	if (result.inserted)
	{
		release_assert (result.election);
		elections.get<tag_root> ().insert ({ result.election, result.election->qualified_root, priority });

		stats.inc (nano::stat::type::election_bucket, nano::stat::detail::activate_success);
	}
	else
	{
		stats.inc (nano::stat::type::election_bucket, nano::stat::detail::activate_failed);
	}

	return result.inserted;
}

void nano::scheduler::bucket::update ()
{
	nano::lock_guard<nano::mutex> lock{ mutex };

	if (election_overfill ())
	{
		cancel_lowest_election ();
	}
}

// Returns true if the block was inserted
bool nano::scheduler::bucket::push (uint64_t time, std::shared_ptr<nano::block> block)
{
	nano::lock_guard<nano::mutex> lock{ mutex };

	auto [it, inserted] = queue.insert ({ time, block });
	release_assert (!queue.empty ());
	bool was_last = (it == --queue.end ());
	if (queue.size () > config.max_blocks)
	{
		queue.erase (--queue.end ());
		return inserted && !was_last;
	}
	return inserted;
}

bool nano::scheduler::bucket::contains (nano::block_hash const & hash) const
{
	nano::lock_guard<nano::mutex> lock{ mutex };
	return queue.get<tag_hash> ().contains (hash);
}

size_t nano::scheduler::bucket::size () const
{
	nano::lock_guard<nano::mutex> lock{ mutex };
	return queue.size ();
}

bool nano::scheduler::bucket::empty () const
{
	nano::lock_guard<nano::mutex> lock{ mutex };
	return queue.empty ();
}

size_t nano::scheduler::bucket::election_count () const
{
	nano::lock_guard<nano::mutex> lock{ mutex };
	return elections.size ();
}

void nano::scheduler::bucket::cancel_lowest_election ()
{
	debug_assert (!mutex.try_lock ());

	if (!elections.empty ())
	{
		elections.get<tag_priority> ().begin ()->election->cancel ();

		stats.inc (nano::stat::type::election_bucket, nano::stat::detail::cancel_lowest);
	}
}

std::deque<std::shared_ptr<nano::block>> nano::scheduler::bucket::blocks () const
{
	nano::lock_guard<nano::mutex> lock{ mutex };

	std::deque<std::shared_ptr<nano::block>> result;
	for (auto const & item : queue)
	{
		result.push_back (item.block);
	}
	return result;
}

void nano::scheduler::bucket::dump () const
{
	for (auto const & item : queue)
	{
		std::cerr << item.time << ' ' << item.block->hash ().to_string () << '\n';
	}
}

/*
 * priority_bucket_config
 */

nano::error nano::scheduler::priority_bucket_config::serialize (nano::tomlconfig & toml) const
{
	toml.put ("max_blocks", max_blocks, "Maximum number of blocks to sort by priority per bucket. \nType: uint64");
	toml.put ("reserved_elections", reserved_elections, "Number of guaranteed slots per bucket available for election activation. \nType: uint64");
	toml.put ("max_elections", max_elections, "Maximum number of slots per bucket available for election activation if the active election count is below the configured limit. \nType: uint64");
	toml.put ("baseline_cps", baseline_cps, "Baseline CPS rate limit across all buckets. Set to 0.0 to disable rate limiting. \nType: double");
	toml.put ("burst_multiplier", burst_multiplier, "Burst multiplier for idle accounts. Burst quota = burst_multiplier * baseline_quota. \nType: double");

	return toml.get_error ();
}

nano::error nano::scheduler::priority_bucket_config::deserialize (nano::tomlconfig & toml)
{
	toml.get ("max_blocks", max_blocks);
	toml.get ("reserved_elections", reserved_elections);
	toml.get ("max_elections", max_elections);
	toml.get ("baseline_cps", baseline_cps);
	toml.get ("burst_multiplier", burst_multiplier);

	return toml.get_error ();
}