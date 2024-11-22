#include <nano/lib/enum_util.hpp>
#include <nano/lib/thread_roles.hpp>
#include <nano/lib/utility.hpp>
#include <nano/node/active_elections.hpp>
#include <nano/node/election.hpp>
#include <nano/node/vote_cache.hpp>
#include <nano/node/vote_router.hpp>
#include <nano/secure/ledger.hpp>
#include <nano/secure/ledger_set_any.hpp>
#include <nano/secure/ledger_set_confirmed.hpp>
#include <nano/secure/vote.hpp>

#include <chrono>

using namespace std::chrono_literals;

nano::stat::detail nano::to_stat_detail (nano::vote_code code)
{
	return nano::enum_util::cast<nano::stat::detail> (code);
}

nano::stat::detail nano::to_stat_detail (nano::vote_source source)
{
	return nano::enum_util::cast<nano::stat::detail> (source);
}

nano::vote_router::vote_router (nano::vote_cache & vote_cache_a,
nano::recently_confirmed_cache & recently_confirmed_a,
nano::ledger & ledger_a,
nano::active_elections & active_a) :
	vote_cache{ vote_cache_a },
	recently_confirmed{ recently_confirmed_a },
	ledger{ ledger_a },
	active{ active_a }
{
}

nano::vote_router::~vote_router ()
{
	// Thread must be stopped before destruction
	debug_assert (!thread.joinable ());
}

void nano::vote_router::connect (nano::block_hash const & hash, std::weak_ptr<nano::election> election)
{
	std::unique_lock lock{ mutex };
	elections.insert_or_assign (hash, election);
}

void nano::vote_router::disconnect (nano::election const & election)
{
	std::unique_lock lock{ mutex };
	for (auto const & [hash, _] : election.blocks ())
	{
		elections.erase (hash);
	}
}

void nano::vote_router::disconnect (nano::block_hash const & hash)
{
	std::unique_lock lock{ mutex };
	[[maybe_unused]] auto erased = elections.erase (hash);
	debug_assert (erased == 1);
}

std::shared_ptr<nano::block> nano::vote_router::get_block (nano::block_hash const & hash)
{
	std::shared_lock cache_lock{ cache_mutex };
	auto cache_it = block_cache.find (hash);
	if (cache_it != block_cache.end ())
	{
		cache_it->second.last_access = std::chrono::steady_clock::now ();
		return cache_it->second.block;
	}
	cache_lock.unlock ();

	// Not in cache, check ledger
	auto transaction = ledger.tx_begin_read ();
	auto block = ledger.any.block_get (transaction, hash);

	if (block)
	{
		// Add to cache
		std::unique_lock write_lock{ cache_mutex };
		prune_cache ();
		block_cache[hash] = { block, std::chrono::steady_clock::now () };
	}

	return block;
}

void nano::vote_router::prune_cache ()
{
	// TODO: prune cache (name it trim_overflow maybe?)
}
std::unordered_map<nano::block_hash, nano::vote_code> nano::vote_router::vote (std::shared_ptr<nano::vote> const & vote, nano::vote_source source, nano::block_hash filter)
{
	debug_assert (!vote->validate ()); // false => valid vote
	debug_assert (filter.is_zero () || std::any_of (vote->hashes.begin (), vote->hashes.end (), [&filter] (auto const & hash) {
		return hash == filter;
	}));

	std::unordered_map<nano::block_hash, nano::vote_code> results;
	std::unordered_map<nano::block_hash, std::shared_ptr<nano::election>> process;
	std::vector<std::pair<nano::block_hash, std::shared_ptr<nano::election>>> to_connect;

	// Single loop to handle both existing and new elections
	for (auto const & hash : vote->hashes)
	{
		// Ignore votes for other hashes if a filter is set
		if (!filter.is_zero () && hash != filter)
		{
			continue;
		}
		// Ignore duplicate hashes (should not happen with a well-behaved voting node)
		if (results.find (hash) != results.end ())
		{
			continue;
		}

		// First check existing elections and confirmed blocks under shared lock
		bool needs_new_election = false;
		{
			std::shared_lock lock{ mutex };
			if (auto existing = elections.find (hash); existing != elections.end ())
			{
				if (auto election = existing->second.lock ())
				{
					process[hash] = election;
					continue;
				}
			}
			else if (recently_confirmed.exists (hash))
			{
				results[hash] = nano::vote_code::replay;
				continue;
			}
			needs_new_election = true;
		}

		// If we get here, we need to try creating a new election
		if (needs_new_election)
		{
			if (auto block = get_block (hash))
			{
				auto result = active.insert (block, nano::election_behavior::passive);
				if (result.inserted && result.election)
				{
					process[hash] = result.election;
					to_connect.emplace_back (hash, result.election);
				}
				else
				{
					results[hash] = nano::vote_code::indeterminate;
				}
			}
			else
			{
				results[hash] = nano::vote_code::indeterminate;
			}
		}
	}

	// Connect any new elections
	if (!to_connect.empty ())
	{
		std::unique_lock lock{ mutex };
		for (auto const & [hash, election] : to_connect)
		{
			elections.insert_or_assign (hash, election);
		}
	}

	// Process votes for all elections
	for (auto const & [block_hash, election] : process)
	{
		auto const vote_result = election->vote (vote->account, vote->timestamp (), block_hash, source);
		results[block_hash] = vote_result;
	}

	// All hashes should have their result set
	debug_assert (!filter.is_zero () || std::all_of (vote->hashes.begin (), vote->hashes.end (), [&results] (auto const & hash) {
		return results.find (hash) != results.end ();
	}));

	vote_processed.notify (vote, source, results);
	return results;
}

bool nano::vote_router::is_active (nano::block_hash const & hash) const
{
	std::shared_lock lock{ mutex };
	if (auto existing = elections.find (hash); existing != elections.end ())
	{
		if (auto election = existing->second.lock (); election != nullptr)
		{
			return true;
		}
	}
	return false;
}

std::shared_ptr<nano::election> nano::vote_router::election (nano::block_hash const & hash) const
{
	std::shared_lock lock{ mutex };
	if (auto existing = elections.find (hash); existing != elections.end ())
	{
		if (auto election = existing->second.lock (); election != nullptr)
		{
			return election;
		}
	}
	return nullptr;
}

void nano::vote_router::start ()
{
	thread = std::thread{ [this] () {
		nano::thread_role::set (nano::thread_role::name::vote_router);
		run ();
	} };
}

void nano::vote_router::stop ()
{
	std::unique_lock lock{ mutex };
	stopped = true;
	lock.unlock ();
	condition.notify_all ();
	if (thread.joinable ())
	{
		thread.join ();
	}
}

void nano::vote_router::run ()
{
	std::unique_lock lock{ mutex };
	while (!stopped)
	{
		std::erase_if (elections, [] (auto const & pair) { return pair.second.lock () == nullptr; });
		condition.wait_for (lock, 15s, [&] () { return stopped; });
	}
}

nano::container_info nano::vote_router::container_info () const
{
	std::shared_lock lock{ mutex };

	nano::container_info info;
	info.put ("elections", elections);
	return info;
}
