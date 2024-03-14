#pragma once

#include <nano/lib/logging.hpp>
#include <nano/secure/common.hpp>

#include <chrono>
#include <future>
#include <memory>
#include <optional>
#include <thread>

namespace nano
{
class block;
class node;
class write_database_queue;
}

namespace nano::store
{
class write_transaction;
}

namespace nano
{

enum class block_source
{
	unknown = 0,
	live,
	bootstrap,
	bootstrap_legacy,
	unchecked,
	local,
	forced,
};

std::string_view to_string (block_source);
nano::stat::detail to_stat_detail (block_source);

/**
 * Processing blocks is a potentially long IO operation.
 * This class isolates block insertion from other operations like servicing network operations
 */
class block_processor final
{
public: // Context
	class context
	{
	public:
		context (std::shared_ptr<nano::block> block, block_source source);

		std::shared_ptr<nano::block> const block;
		block_source const source;
		std::chrono::steady_clock::time_point const arrival{ std::chrono::steady_clock::now () };

	public:
		using result_t = nano::block_status;
		std::future<result_t> get_future ();

	private:
		void set_result (result_t const &);
		std::promise<result_t> promise;

		friend class block_processor;
	};

public:
	block_processor (nano::node &, nano::write_database_queue &);
	~block_processor ();

	void start ();
	void stop ();

	std::size_t size ();
	bool full ();
	bool half_full ();
	void add (std::shared_ptr<nano::block> const &, block_source = block_source::live);
	std::optional<nano::block_status> add_blocking (std::shared_ptr<nano::block> const & block, block_source);
	void force (std::shared_ptr<nano::block> const &);
	bool should_log ();
	bool have_blocks_ready ();
	bool have_blocks ();

	std::unique_ptr<container_info_component> collect_container_info (std::string const & name);

	std::atomic<bool> flushing{ false };

public: // Events
	using processed_t = std::tuple<nano::block_status, context>;
	using processed_batch_t = std::deque<processed_t>;

	// The batch observer feeds the processed observer
	nano::observer_set<nano::block_status const &, context const &> block_processed;
	nano::observer_set<processed_batch_t const &> batch_processed;
	nano::observer_set<std::shared_ptr<nano::block> const &> rolled_back;

private:
	void run ();
	// Roll back block in the ledger that conflicts with 'block'
	void rollback_competitor (store::write_transaction const &, nano::block const & block);
	nano::block_status process_one (store::write_transaction const &, context const &, bool forced = false);
	void queue_unchecked (store::write_transaction const &, nano::hash_or_account const &);
	processed_batch_t process_batch (nano::unique_lock<nano::mutex> &);
	context next ();
	void add_impl (context);

private: // Dependencies
	nano::node & node;
	nano::write_database_queue & write_database_queue;

private:
	std::deque<context> blocks;
	std::deque<context> forced;

	std::chrono::steady_clock::time_point next_log;

	bool stopped{ false };
	nano::condition_variable condition;
	nano::mutex mutex{ mutex_identifier (mutexes::block_processor) };
	std::thread thread;
};
}
