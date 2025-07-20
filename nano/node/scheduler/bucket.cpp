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
	burst_multiplier{ burst_multiplier }
{
	// Calculate quotas for each bucket
	double baseline_quota_per_minute = (baseline_cps * 60.0) / bucket_count;
	uint32_t baseline_quota = static_cast<uint32_t> (std::max (1.0, baseline_quota_per_minute));
	uint32_t burst_quota = static_cast<uint32_t> (burst_multiplier * baseline_quota);
	
	// Initialize all bucket states
	for (auto & bucket : buckets)
	{
		bucket.baseline_quota_per_minute = baseline_quota;
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
		bucket_state.baseline_used_this_minute = 0;
		bucket_state.burst_used_this_minute = 0;
	}
}

bool nano::scheduler::minute_based_cps_limiter::can_activate_now (size_t bucket_id, bool account_is_idle)
{
	debug_assert (bucket_id < buckets.size ());
	
	auto & state = buckets[bucket_id];
	update_minute_window (bucket_id);
	
	// Anyone can use baseline quota
	if (state.baseline_used_this_minute < state.baseline_quota_per_minute)
	{
		return true; // Will use baseline quota
	}
	
	// Burst requires ALL 3 conditions:
	bool condition1 = state.burst_used_this_minute < state.burst_quota_per_minute; // bursting quota > 0
	bool condition2 = account_is_idle; // account idle > 1 hour
	bool condition3 = (state.baseline_used_this_minute + state.burst_used_this_minute) < state.baseline_quota_per_minute; // current usage < baseline
	
	if (condition1 && condition2 && condition3)
	{
		return true; // Will use burst quota
	}
	
	return false; // Both quotas exhausted or conditions not met
}

void nano::scheduler::minute_based_cps_limiter::on_block_activated (size_t bucket_id, bool used_burst_quota)
{
	debug_assert (bucket_id < buckets.size ());
	
	auto & state = buckets[bucket_id];
	
	if (used_burst_quota)
	{
		state.burst_used_this_minute++;
	}
	else
	{
		state.baseline_used_this_minute++;
	}
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
	// Simple heuristic: if the priority timestamp is more than 1 hour old, consider account idle
	auto now = std::chrono::steady_clock::now ();
	auto timestamp_time = std::chrono::steady_clock::time_point (std::chrono::milliseconds (priority_timestamp));
	auto age = std::chrono::duration_cast<std::chrono::hours> (now - timestamp_time);
	return age.count () >= 1;
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
	bool used_burst_quota = false;

	// Check rate limit BEFORE removing from queue
	if (rate_limiter)
	{
		if (!rate_limiter->can_activate_now (index, account_is_idle))
		{
			// Determine why activation was denied for better stats
			auto & state = rate_limiter->buckets[index];
			rate_limiter->update_minute_window (index);
			
			if (state.baseline_used_this_minute >= state.baseline_quota_per_minute)
			{
				// Baseline exhausted, check burst conditions
				if (state.burst_used_this_minute >= state.burst_quota_per_minute)
				{
					stats.inc (nano::stat::type::election_bucket, nano::stat::detail::cps_burst_denied_quota);
				}
				else if (!account_is_idle)
				{
					stats.inc (nano::stat::type::election_bucket, nano::stat::detail::cps_burst_denied_not_idle);
				}
				else
				{
					stats.inc (nano::stat::type::election_bucket, nano::stat::detail::cps_burst_denied_over_baseline);
				}
			}
			else
			{
				stats.inc (nano::stat::type::election_bucket, nano::stat::detail::cps_rate_limited);
			}
			return false; // Block stays in queue, try next iteration
		}
		
		// Determine if this activation will use burst quota
		auto & state = rate_limiter->buckets[index];
		if (state.baseline_used_this_minute >= state.baseline_quota_per_minute)
		{
			used_burst_quota = true;
			stats.inc (nano::stat::type::election_bucket, nano::stat::detail::cps_burst_allowed);
		}
	}

	queue.erase (queue.begin ());

	auto block = top.block;
	auto priority = top.time;

	auto erase_callback = [this] (std::shared_ptr<nano::election> election) {
		nano::lock_guard<nano::mutex> lock{ mutex };
		elections.get<tag_root> ().erase (election->qualified_root);
	};

	auto result = active.insert (block, nano::election_behavior::priority, erase_callback);
	if (result.inserted)
	{
		release_assert (result.election);
		elections.get<tag_root> ().insert ({ result.election, result.election->qualified_root, priority });

		// Record successful activation for rate limiting
		if (rate_limiter)
		{
			rate_limiter->on_block_activated (index, used_burst_quota);
		}

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