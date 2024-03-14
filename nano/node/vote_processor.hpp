#pragma once

#include <nano/lib/numbers.hpp>
#include <nano/lib/utility.hpp>
#include <nano/secure/common.hpp>

#include <deque>
#include <memory>
#include <thread>
#include <unordered_set>

namespace nano
{
class active_transactions;
namespace store
{
	class component;
}
class node_observers;
class stats;
class node_config;
class logger;
class online_reps;
class rep_crawler;
class ledger;
class network_params;
class node_flags;
class stats;
class rep_tiers;

namespace transport
{
	class channel;
}

class vote_processor final
{
public:
	vote_processor (nano::active_transactions &, nano::node_observers &, nano::stats &, nano::node_config &, nano::node_flags &, nano::logger &, nano::online_reps &, nano::rep_crawler &, nano::ledger &, nano::network_params &, nano::rep_tiers &);
	~vote_processor ();

	void start ();
	void stop ();

	/** Returns false if the vote was processed */
	bool vote (std::shared_ptr<nano::vote> const &, std::shared_ptr<nano::transport::channel> const &);
	/** Note: node.active.mutex lock is required */
	nano::vote_code vote_blocking (std::shared_ptr<nano::vote> const &, std::shared_ptr<nano::transport::channel> const &, bool = false);

	/** Function blocks until either the current queue size (a established flush boundary as it'll continue to increase)
	 * is processed or the queue is empty (end condition or cutoff's guard, as it is positioned ahead) */
	void flush ();
	std::size_t size () const;
	bool empty () const;

	std::atomic<uint64_t> total_processed{ 0 };

private: // Dependencies
	nano::active_transactions & active;
	nano::node_observers & observers;
	nano::stats & stats;
	nano::node_config & config;
	nano::logger & logger;
	nano::online_reps & online_reps;
	nano::rep_crawler & rep_crawler;
	nano::ledger & ledger;
	nano::network_params & network_params;
	nano::rep_tiers & rep_tiers;

private:
	void run ();
	void verify_votes (std::deque<std::pair<std::shared_ptr<nano::vote>, std::shared_ptr<nano::transport::channel>>> const &);

private:
	std::size_t const max_votes;
	std::deque<std::pair<std::shared_ptr<nano::vote>, std::shared_ptr<nano::transport::channel>>> votes;

private:
	bool stopped{ false };
	nano::condition_variable condition;
	mutable nano::mutex mutex{ mutex_identifier (mutexes::vote_processor) };
	std::thread thread;

	friend std::unique_ptr<container_info_component> collect_container_info (vote_processor & vote_processor, std::string const & name);
};

std::unique_ptr<container_info_component> collect_container_info (vote_processor & vote_processor, std::string const & name);
}
