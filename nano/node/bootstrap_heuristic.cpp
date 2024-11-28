#include <nano/node/bootstrap_heuristic.hpp>
#include <nano/node/node.hpp>

#include <random>

nano::bootstrap_heuristic::bootstrap_heuristic (nano::node & node_a, nano::stats & stats_a, nano::logger & logger_a) :
	node (node_a),
	stats (stats_a),
	logger (logger_a)
{
}

void nano::bootstrap_heuristic::start ()
{
	// Subscribe to block processor notifications
	node.block_processor.batch_processed.add ([this] (auto const & batch) {
		process_batch (batch);
	});

	logger.info (nano::log::type::bootstrap, "Bootstrap heuristic started");
}

void nano::bootstrap_heuristic::stop ()
{
	nano::lock_guard<nano::mutex> lock{ mutex };
	stopped = true;
	logger.info (nano::log::type::bootstrap, "Bootstrap heuristic stopped");
}

bool nano::bootstrap_heuristic::is_bootstrapping () const
{
	nano::lock_guard<nano::mutex> lock{ mutex };

	if (recent_blocks.size () < window_size / 2)
	{
		logger.info (nano::log::type::bootstrap, "Bootstrap heuristic (check): true (low block count)");
		return true;
	}

	size_t bootstrap_count = std::count_if (recent_blocks.begin (), recent_blocks.end (),
	[] (auto source) {
		return source == nano::block_source::bootstrap || source == nano::block_source::bootstrap_legacy;
	});

	bool is_bootstrapping = (double)bootstrap_count / recent_blocks.size () >= bootstrap_threshold;
	logger.info (nano::log::type::bootstrap, "Bootstrap heuristic check: {} ({}/{} blocks from bootstrap)", is_bootstrapping, bootstrap_count, recent_blocks.size ());
	return is_bootstrapping;
}

bool nano::bootstrap_heuristic::trigger_randomly (std::chrono::minutes min_interval, std::chrono::minutes max_interval) const
{
	static std::chrono::steady_clock::time_point last_trigger_time = std::chrono::steady_clock::now ();
	auto const now = std::chrono::steady_clock::now ();
	auto const elapsed = std::chrono::duration_cast<std::chrono::minutes> (now - last_trigger_time);

	if (elapsed >= min_interval)
	{
		std::random_device rd;
		std::mt19937 gen (rd ());
		std::uniform_int_distribution<> dis (min_interval.count (), max_interval.count ());

		auto const random_interval = std::chrono::minutes{ dis (gen) };
		if (elapsed >= random_interval)
		{
			last_trigger_time = now;
			return true;
		}
	}
	return false;
}

void nano::bootstrap_heuristic::process_batch (nano::block_processor::processed_batch_t const & batch)
{
	if (stopped)
	{
		return;
	}

	nano::lock_guard<nano::mutex> lock{ mutex };

	for (auto const & [result, context] : batch)
	{
		if (result == nano::block_status::progress)
		{
			recent_blocks.push_back (context.source);
			if (recent_blocks.size () > window_size)
			{
				recent_blocks.pop_front ();
			}
		}
	}
}