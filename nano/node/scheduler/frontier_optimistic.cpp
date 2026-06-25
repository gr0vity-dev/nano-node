#include <nano/lib/blocks.hpp>
#include <nano/lib/container_info.hpp>
#include <nano/lib/logging.hpp>
#include <nano/lib/stats.hpp>
#include <nano/lib/thread_roles.hpp>
#include <nano/lib/tomlconfig.hpp>
#include <nano/lib/utility.hpp>
#include <nano/node/active_elections.hpp>
#include <nano/node/election_behavior.hpp>
#include <nano/node/node.hpp>
#include <nano/node/scheduler/frontier_optimistic.hpp>
#include <nano/secure/ledger.hpp>
#include <nano/secure/ledger_set_any.hpp>
#include <nano/secure/ledger_set_cemented.hpp>

#include <algorithm>

nano::scheduler::frontier_optimistic::frontier_optimistic (frontier_optimistic_config const & config_a, nano::node & node_a, nano::ledger & ledger_a, nano::active_elections & active_a, nano::stats & stats_a) :
	config{ config_a },
	node{ node_a },
	ledger{ ledger_a },
	active{ active_a },
	stats{ stats_a }
{
}

nano::scheduler::frontier_optimistic::~frontier_optimistic ()
{
	debug_assert (!thread.joinable ());
}

void nano::scheduler::frontier_optimistic::start ()
{
	debug_assert (!thread.joinable ());

	if (!config.enable)
	{
		stats.inc (nano::stat::type::optimistic_election, nano::stat::detail::frontier_scheduler_disabled);
		return;
	}

	stats.inc (nano::stat::type::optimistic_election, nano::stat::detail::frontier_scheduler_enabled);
	stopped = false;
	thread = std::thread{ [this] () {
		nano::thread_role::set (nano::thread_role::name::scheduler_optimistic);
		run ();
	} };
}

void nano::scheduler::frontier_optimistic::stop ()
{
	{
		nano::lock_guard<nano::mutex> guard{ mutex };
		stopped = true;
	}
	condition.notify_all ();
	join_or_pass (thread);
}

void nano::scheduler::frontier_optimistic::disable_after_bootstrap ()
{
	nano::lock_guard<nano::mutex> guard{ mutex };
	for (auto & [hash, candidate] : entries)
	{
		if (candidate.terminal == terminal_result::none)
		{
			set_terminal_locked (candidate, terminal_result::disabled_after_bootstrap);
		}
	}
	condition.notify_all ();
}

void nano::scheduler::frontier_optimistic::activate (nano::account const & account, nano::block_hash const & hash)
{
	stats.inc (nano::stat::type::optimistic_election, nano::stat::detail::frontier_verified);

	if (!config.enable)
	{
		stats.inc (nano::stat::type::optimistic_election, nano::stat::detail::frontier_scheduler_disabled);
		return;
	}

	if (ledger.bootstrap_height_reached ())
	{
		stats.inc (nano::stat::type::optimistic_election, nano::stat::detail::frontier_scheduler_disabled_after_bootstrap);
		return;
	}

	nano::lock_guard<nano::mutex> guard{ mutex };
	auto now = std::chrono::steady_clock::now ();

	auto existing = entries.find (hash);
	if (existing != entries.end ())
	{
		if (existing->second.terminal != terminal_result::none)
		{
			existing->second = { account, hash, now, {}, 0, terminal_result::none };
			stats.inc (nano::stat::type::optimistic_election, nano::stat::detail::frontier_backlog_insert);
		}
		condition.notify_all ();
		return;
	}

	entries.emplace (hash, entry{ account, hash, now, {}, 0, terminal_result::none });
	stats.inc (nano::stat::type::optimistic_election, nano::stat::detail::frontier_backlog_insert);
	condition.notify_all ();
}

void nano::scheduler::frontier_optimistic::notify ()
{
	if (active.vacancy (nano::election_behavior::optimistic) > 0)
	{
		condition.notify_all ();
	}
}

bool nano::scheduler::frontier_optimistic::predicate () const
{
	debug_assert (!mutex.try_lock ());
	if (entries.empty ())
	{
		return false;
	}
	if (!ledger.bootstrap_height_reached () && active.vacancy (nano::election_behavior::optimistic) <= 0)
	{
		return false;
	}
	return std::any_of (entries.begin (), entries.end (), [] (auto const & item) {
		return item.second.terminal == terminal_result::none;
	});
}

void nano::scheduler::frontier_optimistic::run ()
{
	nano::unique_lock<nano::mutex> lock{ mutex };
	while (!stopped)
	{
		condition.wait_for (lock, config.retry_interval, [this] () {
			return stopped.load ();
		});
		if (stopped)
		{
			return;
		}
		if (predicate ())
		{
			lock.unlock ();
			run_one ();
			lock.lock ();
		}
	}
}

void nano::scheduler::frontier_optimistic::run_one ()
{
	std::deque<nano::block_hash> ready;
	auto const now = std::chrono::steady_clock::now ();
	{
		nano::lock_guard<nano::mutex> guard{ mutex };
		for (auto const & [hash, candidate] : entries)
		{
			if (candidate.terminal == terminal_result::none && (candidate.last_attempt == std::chrono::steady_clock::time_point{} || nano::elapsed (candidate.last_attempt, config.retry_interval, now)))
			{
				ready.push_back (hash);
			}
		}
	}

	for (auto const & hash : ready)
	{
		entry candidate;
		{
			nano::lock_guard<nano::mutex> guard{ mutex };
			auto existing = entries.find (hash);
			if (existing == entries.end () || existing->second.terminal != terminal_result::none)
			{
				continue;
			}
			candidate = existing->second;
		}

		auto result = try_activate (candidate);

		nano::lock_guard<nano::mutex> guard{ mutex };
		auto existing = entries.find (hash);
		if (existing == entries.end () || existing->second.terminal != terminal_result::none)
		{
			continue;
		}
		existing->second.last_attempt = candidate.last_attempt;
		existing->second.attempt_count = candidate.attempt_count;
		if (result == terminal_result::none)
		{
			stats.inc (nano::stat::type::optimistic_election, nano::stat::detail::frontier_retry);
		}
		else
		{
			set_terminal_locked (existing->second, result);
		}
	}
}

auto nano::scheduler::frontier_optimistic::try_activate (entry & candidate) -> terminal_result
{
	candidate.last_attempt = std::chrono::steady_clock::now ();
	++candidate.attempt_count;

	if (ledger.bootstrap_height_reached ())
	{
		return terminal_result::disabled_after_bootstrap;
	}

	if (active.vacancy (nano::election_behavior::optimistic) <= 0)
	{
		return terminal_result::none;
	}

	auto transaction = ledger.tx_begin_read ();
	auto account_info = ledger.any.account_get (transaction, candidate.account);
	if (!account_info || account_info->head != candidate.hash)
	{
		return terminal_result::stale_missing;
	}

	auto block = ledger.any.block_get (transaction, candidate.hash);
	if (!block)
	{
		return terminal_result::stale_missing;
	}

	if (node.block_confirmed_or_being_confirmed (transaction, candidate.hash))
	{
		return terminal_result::already_confirmed;
	}

	auto result = active.insert (block, nano::election_behavior::optimistic);
	if (result.inserted)
	{
		return terminal_result::started;
	}
	if (result.election)
	{
		return terminal_result::already_active;
	}
	if (node.block_confirmed_or_being_confirmed (transaction, candidate.hash))
	{
		return terminal_result::already_confirmed;
	}
	return terminal_result::none;
}

void nano::scheduler::frontier_optimistic::set_terminal (entry & candidate, terminal_result result)
{
	nano::lock_guard<nano::mutex> guard{ mutex };
	set_terminal_locked (candidate, result);
}

void nano::scheduler::frontier_optimistic::set_terminal_locked (entry & candidate, terminal_result result)
{
	debug_assert (!mutex.try_lock ());
	candidate.terminal = result;
	stats.inc (nano::stat::type::optimistic_election, detail_for (result));
	stats.inc (nano::stat::type::optimistic_election, nano::stat::detail::frontier_backlog_remove);
	stats.sample (nano::stat::sample::frontier_candidate_age, nano::log::milliseconds_delta (candidate.first_seen), { 0, 1000 * 60 * 10 });
	stats.sample (nano::stat::sample::frontier_retry_count, candidate.attempt_count, { 0, 1024 });
	sample_terminal_diagnostics (candidate);
}

void nano::scheduler::frontier_optimistic::sample_terminal_diagnostics (entry const & candidate)
{
	auto const duration = candidate.last_attempt == std::chrono::steady_clock::time_point{} ? nano::log::milliseconds_delta (candidate.first_seen) : std::chrono::duration_cast<std::chrono::milliseconds> (candidate.last_attempt - candidate.first_seen).count ();
	stats.sample (nano::stat::sample::frontier_optimistic_election_duration, duration, { 0, 1000 * 60 * 60 });

	auto transaction = ledger.tx_begin_read ();
	auto account_info = ledger.any.account_get (transaction, candidate.account);
	auto const target_height = ledger.any.block_height (transaction, candidate.hash);
	auto const local_height = account_info ? account_info->block_count : 0;
	auto const cemented_height = ledger.cemented.account_height (transaction, candidate.account);
	auto const same_account_blocks = target_height > cemented_height ? target_height - cemented_height : 0;
	auto const target_height_gap = local_height > target_height ? local_height - target_height : 0;

	stats.sample (nano::stat::sample::frontier_cemented_depth, cemented_height, { 0, 1024 * 1024 });
	stats.sample (nano::stat::sample::frontier_accounts_touched, account_info ? 1 : 0, { 0, 1024 });
	stats.sample (nano::stat::sample::frontier_same_account_blocks, same_account_blocks, { 0, 1024 * 1024 });
	stats.sample (nano::stat::sample::frontier_other_account_blocks, 0, { 0, 1024 * 1024 });
	stats.sample (nano::stat::sample::frontier_target_height_gap, target_height_gap, { 0, 1024 * 1024 });
}

nano::stat::detail nano::scheduler::frontier_optimistic::detail_for (terminal_result result)
{
	switch (result)
	{
		case terminal_result::started:
			return nano::stat::detail::frontier_started;
		case terminal_result::already_active:
			return nano::stat::detail::frontier_already_active;
		case terminal_result::already_confirmed:
			return nano::stat::detail::frontier_already_confirmed;
		case terminal_result::stale_missing:
			return nano::stat::detail::frontier_stale_missing;
		case terminal_result::disabled_after_bootstrap:
			return nano::stat::detail::frontier_scheduler_disabled_after_bootstrap;
		case terminal_result::none:
			break;
	}
	debug_assert (false);
	return nano::stat::detail::unknown;
}

nano::container_info nano::scheduler::frontier_optimistic::container_info () const
{
	nano::lock_guard<nano::mutex> guard{ mutex };
	nano::container_info info;
	info.put ("backlog", entries.size ());
	return info;
}

nano::error nano::scheduler::frontier_optimistic_config::deserialize (nano::tomlconfig & toml)
{
	toml.get ("enable", enable);
	toml.get_duration ("retry_interval", retry_interval);
	return toml.get_error ();
}

nano::error nano::scheduler::frontier_optimistic_config::serialize (nano::tomlconfig & toml) const
{
	toml.put ("enable", enable, "Enable or disable frontier-backed optimistic elections\ntype:bool");
	toml.put ("retry_interval", retry_interval.count (), "How often frontier-backed optimistic candidates are retried while active election capacity is unavailable\ntype:milliseconds");
	return toml.get_error ();
}
