#pragma once

#include <nano/lib/locks.hpp>
#include <nano/lib/numbers.hpp>
#include <nano/lib/timer.hpp>
#include <nano/node/fwd.hpp>
#include <nano/secure/common.hpp>

#include <atomic>
#include <chrono>
#include <condition_variable>
#include <deque>
#include <map>
#include <thread>

namespace nano::scheduler
{
class frontier_optimistic_config final
{
public:
	nano::error deserialize (nano::tomlconfig & toml);
	nano::error serialize (nano::tomlconfig & toml) const;

public:
	bool enable{ false };
	std::size_t max_backlog{ 65536 };
	std::chrono::milliseconds retry_interval{ std::chrono::milliseconds{ 250 } };
};

class frontier_optimistic final
{
public:
	frontier_optimistic (frontier_optimistic_config const &, nano::node &, nano::ledger &, nano::active_elections &, nano::stats &);
	~frontier_optimistic ();

	void start ();
	void stop ();
	void activate (nano::account const &, nano::block_hash const &);
	void notify ();
	void disable_after_bootstrap ();

	nano::container_info container_info () const;

private:
	enum class terminal_result
	{
		none,
		started,
		already_active,
		already_confirmed,
		stale_missing,
		disabled_after_bootstrap,
		backlog_full
	};

	struct entry
	{
		nano::account account;
		nano::block_hash hash;
		std::chrono::steady_clock::time_point first_seen;
		std::chrono::steady_clock::time_point last_attempt;
		std::size_t attempt_count{ 0 };
		terminal_result terminal{ terminal_result::none };
	};

	void run ();
	bool predicate () const;
	void run_one ();
	terminal_result try_activate (entry &);
	void set_terminal (entry &, terminal_result);
	void set_terminal_locked (entry &, terminal_result);
	void sample_terminal_diagnostics (entry const &);
	void trim_terminal ();
	static nano::stat::detail detail_for (terminal_result);

private:
	frontier_optimistic_config const & config;
	nano::node & node;
	nano::ledger & ledger;
	nano::active_elections & active;
	nano::stats & stats;

	std::map<nano::block_hash, entry> entries;

	std::atomic<bool> stopped{ false };
	nano::condition_variable condition;
	mutable nano::mutex mutex;
	std::thread thread;
};
}
