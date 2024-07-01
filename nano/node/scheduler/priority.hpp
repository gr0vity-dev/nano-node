#pragma once

#include <nano/lib/numbers.hpp>
#include <nano/node/scheduler/bucket.hpp>

#include <condition_variable>
#include <deque>
#include <memory>
#include <string>
#include <thread>

namespace nano
{
class account_info;
class confirmation_height_info;
class block;
class container_info_component;
class node;
class stats;
}
namespace nano::secure
{
class transaction;
}

namespace nano::scheduler
{
class priority_config
{
public:
	// TODO: Serialization & deserialization

public:
	bool enabled{ true };
};

class buckets;
class priority final
{
public:
	priority (nano::node &, nano::stats &);
	~priority ();

	void start ();
	void stop ();

	/**
	 * Activates the first unconfirmed block of \p account_a
	 * @return true if account was activated
	 */
	bool activate (secure::transaction const &, nano::account const &);
	bool activate (secure::transaction const &, nano::account const &, nano::account_info const &, nano::confirmation_height_info const &);

	void notify ();
	std::size_t size () const;
	bool empty () const;

	std::unique_ptr<container_info_component> collect_container_info (std::string const & name) const;

private: // Dependencies
	priority_config const & config;
	nano::node & node;
	nano::stats & stats;

private:
	void run ();
	void run_cleanup ();
	bool predicate () const;
	bucket & find_bucket (nano::uint128_t priority);

private:
	std::vector<std::unique_ptr<bucket>> buckets;

	bool stopped{ false };
	nano::condition_variable condition;
	mutable nano::mutex mutex;
	std::thread thread;
	std::thread cleanup_thread;
};
}
