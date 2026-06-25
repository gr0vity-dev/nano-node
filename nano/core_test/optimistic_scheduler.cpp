#include <nano/lib/blocks.hpp>
#include <nano/lib/files.hpp>
#include <nano/node/active_elections.hpp>
#include <nano/node/backlog_scan.hpp>
#include <nano/node/bootstrap/frontier_strategy.hpp>
#include <nano/node/node.hpp>
#include <nano/node/election.hpp>
#include <nano/node/nodeconfig.hpp>
#include <nano/node/scheduler/component.hpp>
#include <nano/node/scheduler/frontier_optimistic.hpp>
#include <nano/node/scheduler/hinted.hpp>
#include <nano/node/scheduler/optimistic.hpp>
#include <nano/node/scheduler/priority.hpp>
#include <nano/node/vote_router.hpp>
#include <nano/secure/ledger.hpp>
#include <nano/secure/network_params.hpp>
#include <nano/test_common/chains.hpp>
#include <nano/test_common/system.hpp>
#include <nano/test_common/testutil.hpp>

#include <gtest/gtest.h>

#include <chrono>
#include <vector>

using namespace std::chrono_literals;

namespace
{
std::size_t scheduler_state (nano::container_info const & info, std::string const & name)
{
	auto const & entries = info.entries ();
	auto existing = std::find_if (entries.begin (), entries.end (), [&name] (auto const & entry) {
		return entry.name == name;
	});
	release_assert (existing != entries.end ());
	return existing->size;
}

nano::node_flags quiet_live_node_flags ()
{
	nano::node_flags flags;
	flags.disable_add_initial_peers = true;
	flags.disable_backup = true;
	flags.disable_bootstrap_listener = true;
	flags.disable_legacy_bootstrap = true;
	flags.disable_lazy_bootstrap = true;
	flags.disable_ongoing_bootstrap = true;
	flags.disable_reachout = true;
	flags.disable_reachout_preconfigured = true;
	flags.disable_rep_crawler = true;
	flags.disable_request_loop = true;
	flags.disable_tcp_realtime = true;
	flags.disable_wallet_bootstrap = true;
	return flags;
}

std::shared_ptr<nano::node> start_live_below_bootstrap_node (nano::work_pool & work, nano::node_config const & config)
{
	auto node = std::make_shared<nano::node> (nano::unique_path (), config, work, quiet_live_node_flags ());
	node->start ();
	return node;
}

nano::node_config frontier_optimistic_test_config ()
{
	nano::node_config config;
	config.priority_scheduler->enable = false;
	config.hinted_scheduler->enable = false;
	config.optimistic_scheduler->enable = false;
	config.frontier_optimistic->enable = true;
	config.frontier_optimistic->retry_interval = 10ms;
	return config;
}

bool has_sample (nano::node & node, nano::stat::sample sample)
{
	return !node.stats.samples (sample).empty ();
}

bool start_frontier_scheduler_if_needed (nano::node & node)
{
	if (node.stats.count (nano::stat::type::optimistic_election, nano::stat::detail::frontier_scheduler_enabled) == 0)
	{
		node.scheduler.frontier_optimistic.start ();
		return true;
	}
	return false;
}
}

TEST (optimistic_scheduler, default_below_bootstrap_starts_frontier_only)
{
	nano::network_params live_params{ nano::network_type::nano_live_network };
	nano::work_pool work{ live_params.network, 1 };
	nano::node_config config{ live_params };
	config.optimistic_scheduler->activation_delay = 10ms;

	auto node = start_live_below_bootstrap_node (work, config);
	ASSERT_FALSE (node->ledger.bootstrap_height_reached ());

	auto info = node->scheduler.container_info ();
	ASSERT_EQ (scheduler_state (info, "frontier_bootstrap_mode"), 1);
	ASSERT_EQ (scheduler_state (info, "frontier_scheduler_started"), 1);
	ASSERT_EQ (scheduler_state (info, "normal_schedulers_started"), 0);
	ASSERT_EQ (node->stats.count (nano::stat::type::optimistic_election, nano::stat::detail::frontier_scheduler_enabled), 1);

	node->stop ();
	work.stop ();
}

TEST (optimistic_scheduler, bootstrap_height_reached_starts_normal_schedulers)
{
	nano::node_config config;
	nano::work_pool work{ config.network_params.network, 1 };

	auto node = start_live_below_bootstrap_node (work, config);
	ASSERT_TRUE (node->ledger.bootstrap_height_reached ());

	auto info = node->scheduler.container_info ();
	ASSERT_EQ (scheduler_state (info, "frontier_bootstrap_mode"), 0);
	ASSERT_EQ (scheduler_state (info, "frontier_scheduler_started"), 0);
	ASSERT_EQ (scheduler_state (info, "normal_schedulers_started"), 1);
	ASSERT_EQ (node->stats.count (nano::stat::type::optimistic_election, nano::stat::detail::frontier_scheduler_enabled), 0);

	node->stop ();
	work.stop ();
}

TEST (optimistic_scheduler, frontier_classification_emits_only_local_uncemented_heads)
{
	nano::test::system system;
	auto & node = *system.add_node (frontier_optimistic_test_config ());

	auto chains = nano::test::setup_chains (system, node, /* single chain */ 1, /* block count */ 3, nano::dev::genesis_key, /* do not confirm */ false);
	auto const & [account, blocks] = chains.front ();
	auto const & head = blocks.back ();

	std::deque<std::pair<nano::account, nano::block_hash>> frontiers;
	frontiers.emplace_back (account, head->hash ());

	auto transaction = node.ledger.tx_begin_read ();
	auto result = nano::bootstrap::classify_frontiers (transaction, node.ledger, node, frontiers);

	ASSERT_EQ (result.local_head_matched, 1);
	ASSERT_EQ (result.block_present, 1);
	ASSERT_EQ (result.candidates.size (), 1);
	ASSERT_EQ (result.candidates.front ().first, account);
	ASSERT_EQ (result.candidates.front ().second, head->hash ());
	ASSERT_TRUE (result.prioritize.empty ());
}

TEST (optimistic_scheduler, frontier_activation_starts_real_optimistic_election_and_samples_diagnostics)
{
	nano::test::system system;
	auto & node = *system.add_node (frontier_optimistic_test_config ());
	node.ledger.bootstrap_weights.max_blocks = node.ledger.block_count () + 1000;
	auto manually_started = start_frontier_scheduler_if_needed (node);

	auto chains = nano::test::setup_chains (system, node, /* single chain */ 1, /* block count */ 3, nano::dev::genesis_key, /* do not confirm */ false);
	auto const & [account, blocks] = chains.front ();
	auto const & head = blocks.back ();

	ASSERT_FALSE (node.ledger.bootstrap_height_reached ());
	ASSERT_GT (node.active.vacancy (nano::election_behavior::optimistic), 0);
	node.scheduler.frontier_optimistic.activate (account, head->hash ());

	ASSERT_TIMELY (5s, node.stats.count (nano::stat::type::optimistic_election, nano::stat::detail::frontier_started) == 1);
	auto election = node.active.election (head->qualified_root ());
	ASSERT_NE (election, nullptr);
	ASSERT_EQ (election->behavior (), nano::election_behavior::optimistic);
	ASSERT_EQ (node.stats.count (nano::stat::type::optimistic_election, nano::stat::detail::frontier_verified), 1);
	ASSERT_EQ (node.stats.count (nano::stat::type::optimistic_election, nano::stat::detail::frontier_dropped), 0);

	std::vector<nano::stat::sample> expected_samples{
		nano::stat::sample::frontier_candidate_age,
		nano::stat::sample::frontier_retry_count,
		nano::stat::sample::frontier_optimistic_election_duration,
		nano::stat::sample::frontier_cemented_depth,
		nano::stat::sample::frontier_accounts_touched,
		nano::stat::sample::frontier_same_account_blocks,
		nano::stat::sample::frontier_other_account_blocks,
		nano::stat::sample::frontier_target_height_gap
	};
	for (auto sample : expected_samples)
	{
		ASSERT_TRUE (has_sample (node, sample));
	}
	if (manually_started)
	{
		node.scheduler.frontier_optimistic.stop ();
	}
}

TEST (optimistic_scheduler, frontier_activation_reports_already_active_and_already_confirmed)
{
	nano::test::system system;
	auto & node = *system.add_node (frontier_optimistic_test_config ());
	node.ledger.bootstrap_weights.max_blocks = node.ledger.block_count () + 1000;
	auto manually_started = start_frontier_scheduler_if_needed (node);

	auto chains = nano::test::setup_chains (system, node, /* single chain */ 1, /* block count */ 3, nano::dev::genesis_key, /* do not confirm */ false);
	auto const & [account, blocks] = chains.front ();
	auto const & head = blocks.back ();

	node.scheduler.frontier_optimistic.activate (account, head->hash ());
	ASSERT_TIMELY (5s, node.stats.count (nano::stat::type::optimistic_election, nano::stat::detail::frontier_started) == 1);

	node.scheduler.frontier_optimistic.activate (account, head->hash ());
	ASSERT_TIMELY (5s, node.stats.count (nano::stat::type::optimistic_election, nano::stat::detail::frontier_already_active) == 1);

	auto confirmed_chains = nano::test::setup_chains (system, node, /* single chain */ 1, /* block count */ 2, nano::dev::genesis_key, /* confirm */ true);
	auto const & [confirmed_account, confirmed_blocks] = confirmed_chains.front ();
	auto const & confirmed_head = confirmed_blocks.back ();
	node.scheduler.frontier_optimistic.activate (confirmed_account, confirmed_head->hash ());
	ASSERT_TIMELY (5s, node.stats.count (nano::stat::type::optimistic_election, nano::stat::detail::frontier_already_confirmed) == 1);
	if (manually_started)
	{
		node.scheduler.frontier_optimistic.stop ();
	}
}

TEST (optimistic_scheduler, frontier_activation_reports_disabled_and_backlog_full_without_drops_in_normal_path)
{
	nano::test::system system;

	auto config = frontier_optimistic_test_config ();
	config.active_elections->optimistic_limit_percentage = 0;
	config.frontier_optimistic->max_backlog = 1;
	auto & node = *system.add_node (config);
	node.ledger.bootstrap_weights.max_blocks = node.ledger.block_count () + 1000;

	auto chains = nano::test::setup_chains (system, node, /* chain count */ 2, /* block count */ 2, nano::dev::genesis_key, /* do not confirm */ false);
	node.scheduler.frontier_optimistic.activate (chains[0].first, chains[0].second.back ()->hash ());
	ASSERT_TIMELY (5s, node.stats.count (nano::stat::type::optimistic_election, nano::stat::detail::frontier_backlog_insert) == 1);
	ASSERT_EQ (node.stats.count (nano::stat::type::optimistic_election, nano::stat::detail::frontier_retry), 0);

	node.scheduler.frontier_optimistic.activate (chains[1].first, chains[1].second.back ()->hash ());
	ASSERT_EQ (node.stats.count (nano::stat::type::optimistic_election, nano::stat::detail::frontier_backlog_full), 1);
	ASSERT_EQ (node.stats.count (nano::stat::type::optimistic_election, nano::stat::detail::frontier_dropped), 1);

	node.ledger.bootstrap_weights.max_blocks = 0;
	node.scheduler.frontier_optimistic.disable_after_bootstrap ();
	ASSERT_EQ (node.stats.count (nano::stat::type::optimistic_election, nano::stat::detail::frontier_scheduler_disabled_after_bootstrap), 1);
}

/*
 * Ensure account gets activated for a single unconfirmed account chain
 */
TEST (optimistic_scheduler, activate_one)
{
	nano::test::system system;

	nano::node_config config;
	config.priority_scheduler->enable = false; // Disable priority scheduler to avoid interference
	auto & node = *system.add_node (config);

	// Needs to be greater than optimistic scheduler `gap_threshold`
	const int howmany_blocks = 64;

	auto chains = nano::test::setup_chains (system, node, /* single chain */ 1, howmany_blocks, nano::dev::genesis_key, /* do not confirm */ false);
	auto & [account, blocks] = chains.front ();

	// Confirm block towards at the beginning the chain, so gap between confirmation and account frontier is larger than `gap_threshold`
	nano::test::confirm (node.ledger, blocks.at (11));

	// Ensure unconfirmed account head block gets activated
	auto const & block = blocks.back ();
	std::shared_ptr<nano::election> election;
	ASSERT_TIMELY (5s, election = node.active.election (block->qualified_root ()));
	ASSERT_EQ (election->behavior (), nano::election_behavior::optimistic);
}

/*
 * Ensure account gets activated for a single unconfirmed account chain with nothing yet confirmed
 */
TEST (optimistic_scheduler, activate_one_zero_conf)
{
	nano::test::system system;

	nano::node_config config;
	config.priority_scheduler->enable = false; // Disable priority scheduler to avoid interference
	auto & node = *system.add_node (config);

	// Can be smaller than optimistic scheduler `gap_threshold`
	// This is meant to activate short account chains (eg. binary tree spam leaf accounts)
	const int howmany_blocks = 6;

	auto chains = nano::test::setup_chains (system, node, /* single chain */ 1, howmany_blocks, nano::dev::genesis_key, /* do not confirm */ false);
	auto & [account, blocks] = chains.front ();

	// Ensure unconfirmed account head block gets activated
	auto const & block = blocks.back ();
	std::shared_ptr<nano::election> election;
	ASSERT_TIMELY (5s, election = node.active.election (block->qualified_root ()));
	ASSERT_EQ (election->behavior (), nano::election_behavior::optimistic);
}

/*
 * Ensure account gets activated for a multiple unconfirmed account chains
 */
TEST (optimistic_scheduler, activate_many)
{
	nano::test::system system;

	nano::node_config config;
	config.priority_scheduler->enable = false; // Disable priority scheduler to avoid interference
	auto & node = *system.add_node (config);

	// Needs to be greater than optimistic scheduler `gap_threshold`
	const int howmany_blocks = 64;
	const int howmany_chains = 16;

	auto chains = nano::test::setup_chains (system, node, howmany_chains, howmany_blocks, nano::dev::genesis_key, /* do not confirm */ false);

	// Ensure all unconfirmed account head blocks get activated
	ASSERT_TIMELY (15s, std::all_of (chains.begin (), chains.end (), [&] (auto const & entry) {
		auto const & [account, blocks] = entry;
		auto const & block = blocks.back ();
		auto election = node.active.election (block->qualified_root ());
		return election && election->behavior () == nano::election_behavior::optimistic;
	}));
}

/*
 * Ensure accounts with some blocks already confirmed and with less than `gap_threshold` blocks do not get activated
 */
TEST (optimistic_scheduler, under_gap_threshold)
{
	nano::test::system system;

	nano::node_config config = system.default_config ();
	config.backlog_scan->enable = false;
	auto & node = *system.add_node (config);

	// Must be smaller than optimistic scheduler `gap_threshold`
	const int howmany_blocks = 64;

	auto chains = nano::test::setup_chains (system, node, /* single chain */ 1, howmany_blocks, nano::dev::genesis_key, /* do not confirm */ false);
	auto & [account, blocks] = chains.front ();

	// Confirm block towards the end of the chain, so gap between confirmation and account frontier is less than `gap_threshold`
	nano::test::confirm (node.ledger, blocks.at (55));

	// Manually trigger backlog scan
	node.backlog_scan.trigger ();

	// Ensure unconfirmed account head block gets activated
	auto const & block = blocks.back ();
	ASSERT_NEVER (3s, node.vote_router.active (block->hash ()));
}
