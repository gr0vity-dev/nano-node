#pragma once

#include <nano/lib/interval.hpp>
#include <nano/node/fwd.hpp>
#include <nano/node/wallet.hpp>

#include <atomic>
#include <chrono>

namespace nano
{
class vote_rebroadcaster_config final
{
public:
	std::chrono::milliseconds interval{ std::chrono::milliseconds{ 15'000 } }; // 15 seconds default

	nano::error serialize (nano::tomlconfig & toml) const;
	nano::error deserialize (nano::tomlconfig & toml);
};

class vote_rebroadcaster final
{
public:
	static size_t constexpr max_queue = 1024 * 16;

public:
	vote_rebroadcaster (vote_rebroadcaster_config const &, nano::vote_router &, nano::network &, nano::wallets &, nano::stats &, nano::logger &);
	~vote_rebroadcaster ();

	void start ();
	void stop ();

	bool put (std::shared_ptr<nano::vote> const &);

	nano::container_info container_info () const;

public: // Dependencies
	nano::vote_router & vote_router;
	nano::network & network;
	nano::wallets & wallets;
	nano::stats & stats;
	nano::logger & logger;

private:
	void run ();

	std::atomic<bool> enable{ true }; // Enable vote rebroadcasting only if the node does not host a representative
	vote_rebroadcaster_config const & config;
	std::deque<std::shared_ptr<nano::vote>> queue;
	nano::wallet_representatives reps;
	nano::interval refresh_interval;

	bool stopped{ false };
	std::condition_variable condition;
	mutable std::mutex mutex;
	std::thread thread;
};
}