#pragma once

#include <nano/lib/locks.hpp>
#include <nano/lib/numbers.hpp>
#include <nano/node/fwd.hpp>
#include <nano/secure/common.hpp>
#include <nano/secure/fwd.hpp>

#include <atomic>
#include <condition_variable>
#include <memory>
#include <string>
#include <thread>

namespace nano::scheduler
{
class component final
{
public:
	component (nano::node_config &, nano::node &, nano::ledger &, nano::ledger_notifications &, nano::bucketing &, nano::active_elections &, nano::online_reps &, nano::vote_cache &, nano::cementing_set &, nano::stats &, nano::logger &);
	~component ();

	void start ();
	void stop ();

	/// Does the block exist in any of the schedulers
	bool contains (nano::block_hash const & hash) const;
	void activate_backlog (nano::secure::transaction const &, nano::account const &, nano::account_info const &, nano::confirmation_height_info const &);

	nano::container_info container_info () const;

private:
	bool bootstrap_height_reached () const;
	bool frontier_bootstrap_mode_enabled () const;
	void apply_bootstrap_height_policy ();
	void start_frontier_scheduler ();
	void stop_frontier_scheduler_after_bootstrap ();
	void start_normal_schedulers ();
	void bootstrap_height_monitor ();

	std::unique_ptr<nano::scheduler::hinted> hinted_impl;
	std::unique_ptr<nano::scheduler::frontier_optimistic> frontier_optimistic_impl;
	std::unique_ptr<nano::scheduler::manual> manual_impl;
	std::unique_ptr<nano::scheduler::optimistic> optimistic_impl;
	std::unique_ptr<nano::scheduler::priority> priority_impl;

	nano::node_config & node_config;
	nano::ledger & ledger;
	std::atomic<bool> stopped{ false };
	bool frontier_scheduler_started{ false };
	bool normal_schedulers_started{ false };
	mutable nano::mutex mutex;
	nano::condition_variable condition;
	std::thread monitor_thread;

public: // Schedulers
	nano::scheduler::hinted & hinted;
	nano::scheduler::frontier_optimistic & frontier_optimistic;
	nano::scheduler::manual & manual;
	nano::scheduler::optimistic & optimistic;
	nano::scheduler::priority & priority;
};
}
