#include <nano/lib/stats.hpp>
#include <nano/lib/tomlconfig.hpp>
#include <nano/node/active_elections.hpp>
#include <nano/node/election.hpp>
#include <nano/node/election_behavior.hpp>
#include <nano/node/node.hpp>
#include <nano/node/scheduler/hinted.hpp>
#include <nano/node/vote_generator.hpp>
#include <nano/secure/ledger.hpp>
#include <nano/secure/ledger_set_any.hpp>

/*
 * hinted
 */

nano::scheduler::hinted::hinted (hinted_config const & config_a, nano::node & node_a, nano::active_elections & active_a, nano::online_reps & online_reps_a, nano::stats & stats_a) :
	config{ config_a },
	node{ node_a },
	active{ active_a },
	online_reps{ online_reps_a },
	stats{ stats_a }
{
}

nano::scheduler::hinted::~hinted ()
{
	// Thread must be stopped before destruction
	debug_assert (!thread.joinable ());
}

void nano::scheduler::hinted::start ()
{
	debug_assert (!thread.joinable ());

	if (!config.enable)
	{
		return;
	}

	thread = std::thread{ [this] () {
		nano::thread_role::set (nano::thread_role::name::scheduler_hinted);
		run ();
	} };
}

void nano::scheduler::hinted::stop ()
{
	{
		nano::lock_guard<nano::mutex> lock{ mutex };
		stopped = true;
	}
	notify ();
	nano::join_or_pass (thread);
}

void nano::scheduler::hinted::notify ()
{
	// Avoid notifying when there is very little space inside AEC
	auto const limit = active.limit (nano::election_behavior::hinted);
	if (active.vacancy (nano::election_behavior::hinted) >= (limit * config.vacancy_threshold_percent / 100))
	{
		condition.notify_all ();
	}
}

void nano::scheduler::hinted::run_iterative ()
{
	// std::cout << "Hinted scheduler: Starting iterative run with " << node.active.size () << " total elections" << std::endl;

	auto transaction = node.ledger.tx_begin_read ();
	auto elections = node.active.list_active (std::numeric_limits<size_t>::max ());

	// std::cout << "Hinted scheduler: Found " << elections.size () << " elections to process" << std::endl;

	size_t passive_count = 0;
	size_t already_confirmed = 0;
	size_t dependents_unconfirmed = 0;
	size_t processed = 0;

	for (auto const & election : elections)
	{
		if (election->behavior () != nano::election_behavior::passive)
		{
			continue;
		}
		passive_count++;

		auto winner = election->winner ();
		// std::cout << "Hinted scheduler: Processing passive election for block " << winner->hash ().to_string () << std::endl;

		if (election->confirmed ())
		{
			already_confirmed++;
			// std::cout << "Hinted scheduler: Skipping, already confirmed" << std::endl;
			continue;
		}

		if (!node.ledger.dependents_confirmed (transaction, *winner))
		{
			dependents_unconfirmed++;
			// std::cout << "Hinted scheduler: Skipping, dependents not confirmed" << std::endl;
			continue;
		}

		// Add to vote generator
		node.active.insert (election->winner (), nano::election_behavior::hinted);
		stats.inc (nano::stat::type::hinting, nano::stat::detail::activate);
		processed++;
	}

	// std::cout << "Hinted scheduler: Run complete. Stats:" << std::endl
	// 		  << "  Total passive elections: " << passive_count << std::endl
	// 		  << "  Already confirmed: " << already_confirmed << std::endl
	// 		  << "  Dependents unconfirmed: " << dependents_unconfirmed << std::endl
	// 		  << "  Successfully processed: " << processed << std::endl
	// 		  << "  Vote generator queue size: " << node.generator.size () << std::endl;
}

bool nano::scheduler::hinted::predicate () const
{
	// Check if there is space inside AEC for a new hinted election
	// return active.vacancy (nano::election_behavior::hinted) > 0 && node.generator.has_vacancy ();
	return node.generator.has_vacancy ();
}

void nano::scheduler::hinted::run ()
{
	nano::unique_lock<nano::mutex> lock{ mutex };
	while (!stopped)
	{
		if (!predicate ())
		{
			condition.wait_for (lock, std::chrono::milliseconds{ 100 });
			continue;
		}

		lock.unlock ();
		run_iterative ();
		lock.lock ();
	}
}

bool nano::scheduler::hinted::cooldown (const nano::block_hash & hash)
{
	nano::lock_guard<nano::mutex> guard{ mutex };

	auto const now = std::chrono::steady_clock::now ();

	// Check if the hash is still in the cooldown period using the hashed index
	auto const & hashed_index = cooldowns_m.get<tag_hash> ();
	if (auto it = hashed_index.find (hash); it != hashed_index.end ())
	{
		if (it->timeout > now)
		{
			return true; // Needs cooldown
		}
		cooldowns_m.erase (it); // Entry is outdated, so remove it
	}

	// Insert the new entry
	cooldowns_m.insert ({ hash, now + config.block_cooldown });

	// Trim old entries
	auto & seq_index = cooldowns_m.get<tag_timeout> ();
	while (!seq_index.empty () && seq_index.begin ()->timeout <= now)
	{
		seq_index.erase (seq_index.begin ());
	}

	return false; // No need to cooldown
}

nano::container_info nano::scheduler::hinted::container_info () const
{
	nano::lock_guard<nano::mutex> guard{ mutex };

	nano::container_info info;
	info.put ("cooldowns", cooldowns_m);
	return info;
}

/*
 * hinted_config
 */

nano::scheduler::hinted_config::hinted_config (nano::network_constants const & network)
{
	if (network.is_dev_network ())
	{
		check_interval = std::chrono::milliseconds{ 100 };
		block_cooldown = std::chrono::milliseconds{ 100 };
	}
}

nano::error nano::scheduler::hinted_config::serialize (nano::tomlconfig & toml) const
{
	toml.put ("enable", enable, "Enable or disable hinted elections\ntype:bool");
	toml.put ("hinting_threshold", hinting_threshold_percent, "Percentage of online weight needed to start a hinted election. \ntype:uint32,[0,100]");
	toml.put ("check_interval", check_interval.count (), "Interval between scans of the vote cache for possible hinted elections. \ntype:milliseconds");
	toml.put ("block_cooldown", block_cooldown.count (), "Cooldown period for blocks that failed to start an election. \ntype:milliseconds");
	toml.put ("vacancy_threshold", vacancy_threshold_percent, "Percentage of available space in the active elections container needed to trigger a scan for hinted elections (before the check interval elapses). \ntype:uint32,[0,100]");

	return toml.get_error ();
}

nano::error nano::scheduler::hinted_config::deserialize (nano::tomlconfig & toml)
{
	toml.get ("enable", enable);
	toml.get ("hinting_threshold", hinting_threshold_percent);

	auto check_interval_l = check_interval.count ();
	toml.get ("check_interval", check_interval_l);
	check_interval = std::chrono::milliseconds{ check_interval_l };

	auto block_cooldown_l = block_cooldown.count ();
	toml.get ("block_cooldown", block_cooldown_l);
	block_cooldown = std::chrono::milliseconds{ block_cooldown_l };

	toml.get ("vacancy_threshold", vacancy_threshold_percent);

	if (hinting_threshold_percent > 100)
	{
		toml.get_error ().set ("hinting_threshold must be a number between 0 and 100");
	}
	if (vacancy_threshold_percent > 100)
	{
		toml.get_error ().set ("vacancy_threshold must be a number between 0 and 100");
	}

	return toml.get_error ();
}
