#include <nano/node/active_elections.hpp>
#include <nano/node/node.hpp>
#include <nano/node/nodeconfig.hpp>
#include <nano/node/scheduler/component.hpp>
#include <nano/node/scheduler/frontier_optimistic.hpp>
#include <nano/node/scheduler/hinted.hpp>
#include <nano/node/scheduler/manual.hpp>
#include <nano/node/scheduler/optimistic.hpp>
#include <nano/node/scheduler/priority.hpp>
#include <nano/lib/utility.hpp>
#include <nano/secure/ledger.hpp>

using namespace std::chrono_literals;

nano::scheduler::component::component (nano::node_config & node_config, nano::node & node, nano::ledger & ledger, nano::ledger_notifications & ledger_notifications, nano::bucketing & bucketing, nano::active_elections & active, nano::online_reps & online_reps, nano::vote_cache & vote_cache, nano::cementing_set & cementing_set, nano::stats & stats, nano::logger & logger) :
	hinted_impl{ std::make_unique<nano::scheduler::hinted> (node_config.hinted_scheduler, node, vote_cache, active, online_reps, stats) },
	frontier_optimistic_impl{ std::make_unique<nano::scheduler::frontier_optimistic> (node_config.frontier_optimistic, node, ledger, active, stats) },
	manual_impl{ std::make_unique<nano::scheduler::manual> (node) },
	optimistic_impl{ std::make_unique<nano::scheduler::optimistic> (node_config.optimistic_scheduler, node, ledger, active, node_config.network_params.network, stats) },
	priority_impl{ std::make_unique<nano::scheduler::priority> (node_config, node, ledger, ledger_notifications, bucketing, active, cementing_set, stats, logger) },
	hinted{ *hinted_impl },
	frontier_optimistic{ *frontier_optimistic_impl },
	manual{ *manual_impl },
	optimistic{ *optimistic_impl },
	priority{ *priority_impl },
	node_config{ node_config },
	ledger{ ledger }
{
	// Notify election schedulers when AEC frees election slot
	active.vacancy_updated.add ([this] () {
		priority.notify ();
		hinted.notify ();
		frontier_optimistic.notify ();
		optimistic.notify ();
	});
}

nano::scheduler::component::~component ()
{
}

void nano::scheduler::component::start ()
{
	manual.start ();

	{
		nano::lock_guard<nano::mutex> guard{ mutex };
		stopped = false;
		apply_bootstrap_height_policy ();
	}

	monitor_thread = std::thread{ [this] () {
		bootstrap_height_monitor ();
	} };
}

void nano::scheduler::component::stop ()
{
	bool stop_frontier_scheduler = false;
	bool stop_normal_scheduler_set = false;
	{
		nano::lock_guard<nano::mutex> guard{ mutex };
		stopped = true;
		stop_frontier_scheduler = frontier_scheduler_started;
		stop_normal_scheduler_set = normal_schedulers_started;
	}
	condition.notify_all ();
	join_or_pass (monitor_thread);

	if (stop_frontier_scheduler)
	{
		frontier_optimistic.stop ();
		nano::lock_guard<nano::mutex> guard{ mutex };
		frontier_scheduler_started = false;
	}
	if (stop_normal_scheduler_set)
	{
		hinted.stop ();
		optimistic.stop ();
		priority.stop ();
		nano::lock_guard<nano::mutex> guard{ mutex };
		normal_schedulers_started = false;
	}
	manual.stop ();
}

bool nano::scheduler::component::bootstrap_height_reached () const
{
	return ledger.bootstrap_height_reached ();
}

bool nano::scheduler::component::frontier_bootstrap_mode_enabled () const
{
	return node_config.frontier_optimistic->enable && !bootstrap_height_reached ();
}

void nano::scheduler::component::apply_bootstrap_height_policy ()
{
	debug_assert (!mutex.try_lock ());
	if (frontier_bootstrap_mode_enabled ())
	{
		start_frontier_scheduler ();
		return;
	}

	stop_frontier_scheduler_after_bootstrap ();
	start_normal_schedulers ();
}

void nano::scheduler::component::start_frontier_scheduler ()
{
	debug_assert (!mutex.try_lock ());
	if (frontier_scheduler_started || !node_config.frontier_optimistic->enable)
	{
		return;
	}
	frontier_optimistic.start ();
	frontier_scheduler_started = true;
}

void nano::scheduler::component::stop_frontier_scheduler_after_bootstrap ()
{
	debug_assert (!mutex.try_lock ());
	frontier_optimistic.disable_after_bootstrap ();
	if (frontier_scheduler_started)
	{
		frontier_optimistic.stop ();
		frontier_scheduler_started = false;
	}
}

void nano::scheduler::component::start_normal_schedulers ()
{
	debug_assert (!mutex.try_lock ());
	if (normal_schedulers_started)
	{
		return;
	}
	hinted.start ();
	optimistic.start ();
	priority.start ();
	normal_schedulers_started = true;
}

void nano::scheduler::component::bootstrap_height_monitor ()
{
	nano::unique_lock<nano::mutex> lock{ mutex };
	while (!stopped && frontier_bootstrap_mode_enabled ())
	{
		condition.wait_for (lock, 250ms, [this] () {
			return stopped.load () || bootstrap_height_reached ();
		});
		if (!stopped)
		{
			apply_bootstrap_height_policy ();
		}
	}
}

bool nano::scheduler::component::contains (nano::block_hash const & hash) const
{
	return manual.contains (hash) || priority.contains (hash);
}

void nano::scheduler::component::activate_backlog (nano::secure::transaction const & transaction, nano::account const & account, nano::account_info const & account_info, nano::confirmation_height_info const & conf_info)
{
	{
		nano::lock_guard<nano::mutex> guard{ mutex };
		if (frontier_bootstrap_mode_enabled ())
		{
			return;
		}
		apply_bootstrap_height_policy ();
	}

	optimistic.activate (account, account_info, conf_info);
	priority.activate (transaction, account, account_info, conf_info);
}

nano::container_info nano::scheduler::component::container_info () const
{
	nano::container_info info;
	{
		nano::lock_guard<nano::mutex> guard{ mutex };
		info.put ("frontier_bootstrap_mode", frontier_bootstrap_mode_enabled () ? 1 : 0);
		info.put ("frontier_scheduler_started", frontier_scheduler_started ? 1 : 0);
		info.put ("normal_schedulers_started", normal_schedulers_started ? 1 : 0);
	}
	info.add ("hinted", hinted.container_info ());
	info.add ("frontier_optimistic", frontier_optimistic.container_info ());
	info.add ("manual", manual.container_info ());
	info.add ("optimistic", optimistic.container_info ());
	info.add ("priority", priority.container_info ());
	return info;
}
