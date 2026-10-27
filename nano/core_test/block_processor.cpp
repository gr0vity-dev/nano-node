#include <nano/lib/blockbuilders.hpp>
#include <nano/lib/blocks.hpp>
#include <nano/lib/stats_enums.hpp>
#include <nano/node/backlog_scan.hpp>
#include <nano/node/block_processor.hpp>
#include <nano/node/bounded_backlog.hpp>
#include <nano/node/ledger_notifications.hpp>
#include <nano/node/node.hpp>
#include <nano/node/nodeconfig.hpp>
#include <nano/secure/common.hpp>
#include <nano/secure/ledger.hpp>
#include <nano/secure/ledger_set_any.hpp>
#include <nano/test_common/chains.hpp>
#include <nano/test_common/system.hpp>
#include <nano/test_common/testutil.hpp>

#include <gtest/gtest.h>

#include <algorithm>
#include <any>
#include <atomic>
#include <mutex>
#include <string>
#include <type_traits>
#include <vector>

using namespace std::chrono_literals;

// Async add of a single block with callback, verifies processing and callback invocation
TEST (block_processor, add)
{
	nano::test::system system;
	auto & node = *system.add_node ();

	auto latest = nano::dev::genesis->hash ();
	auto balance = nano::dev::constants.genesis_amount;

	nano::keypair key;
	nano::block_builder builder;
	balance -= nano::Knano_ratio;
	auto send = builder
				.state ()
				.account (nano::dev::genesis_key.pub)
				.previous (latest)
				.representative (nano::dev::genesis_key.pub)
				.balance (balance)
				.link (key.pub)
				.sign (nano::dev::genesis_key.prv, nano::dev::genesis_key.pub)
				.work (*system.work.generate (latest))
				.build ();

	std::atomic<bool> callback_called{ false };
	std::atomic<nano::block_status> callback_status{};
	auto added = node.block_processor.add (send, nano::block_source::live, nullptr, [&] (nano::block_status status) {
		callback_status = status;
		callback_called = true;
	});
	ASSERT_TRUE (added);

	// Callback should fire once the block is processed
	ASSERT_TIMELY (5s, callback_called.load ());
	ASSERT_EQ (callback_status.load (), nano::block_status::progress);
	ASSERT_TRUE (node.block_or_pruned_exists (send->hash ()));
	ASSERT_EQ (node.ledger.block_count (), 2);
}

// Block with invalid work is rejected before queueing
TEST (block_processor, add_insufficient_work)
{
	nano::test::system system;
	auto & node = *system.add_node ();

	nano::keypair key;
	nano::block_builder builder;
	auto send = builder
				.state ()
				.account (nano::dev::genesis_key.pub)
				.previous (nano::dev::genesis->hash ())
				.representative (nano::dev::genesis_key.pub)
				.balance (nano::dev::constants.genesis_amount - nano::Knano_ratio)
				.link (key.pub)
				.sign (nano::dev::genesis_key.prv, nano::dev::genesis_key.pub)
				.work (0)
				.build ();

	// Block should be rejected without entering the queue
	auto added = node.block_processor.add (send, nano::block_source::live);
	ASSERT_FALSE (added);
	ASSERT_EQ (node.stats.count (nano::stat::type::block_processor, nano::stat::detail::insufficient_work), 1);
	ASSERT_EQ (node.ledger.block_count (), 1);
}

// Synchronous add blocks the caller until processing completes, also verifies duplicate block returns old status
TEST (block_processor, add_blocking)
{
	nano::test::system system;
	auto & node = *system.add_node ();

	auto latest = nano::dev::genesis->hash ();
	auto balance = nano::dev::constants.genesis_amount;

	nano::keypair key;
	nano::block_builder builder;
	balance -= nano::Knano_ratio;
	auto send = builder
				.state ()
				.account (nano::dev::genesis_key.pub)
				.previous (latest)
				.representative (nano::dev::genesis_key.pub)
				.balance (balance)
				.link (key.pub)
				.sign (nano::dev::genesis_key.prv, nano::dev::genesis_key.pub)
				.work (*system.work.generate (latest))
				.build ();

	// Blocks caller until result is available
	auto result = node.block_processor.add_blocking (send, nano::block_source::local);
	ASSERT_TRUE (result.has_value ());
	ASSERT_EQ (result.value (), nano::block_status::progress);
	ASSERT_TRUE (node.block_or_pruned_exists (send->hash ()));

	// Processing the same block again should return old
	auto result2 = node.block_processor.add_blocking (send, nano::block_source::local);
	ASSERT_TRUE (result2.has_value ());
	ASSERT_EQ (result2.value (), nano::block_status::old);
}

TEST (block_processor, tags)
{
	nano::test::system system;
	auto & node = *system.add_node ();

	std::mutex mutex;
	std::vector<std::pair<nano::block_hash, std::string>> observed;
	node.ledger_notifications.blocks_processed.add ([&] (auto const & batch) {
		std::lock_guard<std::mutex> lock{ mutex };
		for (auto const & entry : batch)
		{
			auto const & context = entry.second;
			if (context.block != nullptr)
			{
				if (auto tag = std::any_cast<std::string> (&context.tag))
				{
					observed.emplace_back (context.block->hash (), *tag);
				}
			}
		}
	});

	auto latest = nano::dev::genesis->hash ();
	auto balance = nano::dev::constants.genesis_amount;
	auto make_send = [&] (nano::account const & destination) {
		nano::block_builder builder;
		balance -= nano::Knano_ratio;
		auto send = builder
					.state ()
					.account (nano::dev::genesis_key.pub)
					.previous (latest)
					.representative (nano::dev::genesis_key.pub)
					.balance (balance)
					.link (destination)
					.sign (nano::dev::genesis_key.prv, nano::dev::genesis_key.pub)
					.work (*system.work.generate (latest))
					.build ();
		latest = send->hash ();
		return send;
	};

	nano::keypair key1;
	auto send1 = make_send (key1.pub);
	ASSERT_TRUE (node.block_processor.add (send1, nano::block_source::live, nullptr, nullptr, std::string{ "single" }));
	ASSERT_TIMELY (5s, node.block_or_pruned_exists (send1->hash ()));

	nano::keypair key2;
	auto send2 = make_send (key2.pub);
	nano::keypair key3;
	auto send3 = make_send (key3.pub);
	nano::keypair key4;
	auto send4 = make_send (key4.pub);
	std::deque<std::shared_ptr<nano::block>> batch{ send2, send3, send4 };
	ASSERT_EQ (node.block_processor.add_many (batch, nano::block_source::live, nullptr, nullptr, std::string{ "batch" }).added, batch.size ());
	for (auto const & block : batch)
	{
		ASSERT_TIMELY (5s, node.block_or_pruned_exists (block->hash ()));
	}

	nano::keypair key5;
	auto send5 = make_send (key5.pub);
	auto result = node.block_processor.add_blocking (send5, nano::block_source::local, std::string{ "blocking" });
	ASSERT_TRUE (result.has_value ());
	ASSERT_EQ (result.value (), nano::block_status::progress);

	auto has_observed = [&] (nano::block_hash const & hash, std::string const & tag) {
		std::lock_guard<std::mutex> lock{ mutex };
		return std::any_of (observed.begin (), observed.end (), [&] (auto const & entry) {
			return entry.first == hash && entry.second == tag;
		});
	};

	ASSERT_TIMELY (5s, has_observed (send1->hash (), "single"));
	for (auto const & block : batch)
	{
		ASSERT_TIMELY (5s, has_observed (block->hash (), "batch"));
	}
	ASSERT_TIMELY (5s, has_observed (send5->hash (), "blocking"));
}

// When queue is full, add_blocking returns nullopt because the promise is destroyed unfulfilled
TEST (block_processor, add_blocking_overfill)
{
	nano::test::system system;

	nano::node_config node_config;
	// Queue size 0 guarantees immediate rejection
	node_config.block_processor->max_system_queue = 0;
	auto & node = *system.add_node (node_config);

	nano::keypair key;
	nano::block_builder builder;
	auto send = builder
				.state ()
				.account (nano::dev::genesis_key.pub)
				.previous (nano::dev::genesis->hash ())
				.representative (nano::dev::genesis_key.pub)
				.balance (nano::dev::constants.genesis_amount - nano::Knano_ratio)
				.link (key.pub)
				.sign (nano::dev::genesis_key.prv, nano::dev::genesis_key.pub)
				.work (*system.work.generate (nano::dev::genesis->hash ()))
				.build ();

	// Block is dropped due to queue being full, promise is destroyed unfulfilled, add_blocking returns nullopt
	auto result = node.block_processor.add_blocking (send, nano::block_source::local);
	ASSERT_FALSE (result.has_value ());
	ASSERT_EQ (node.stats.count (nano::stat::type::block_processor, nano::stat::detail::overfill), 1);
}

// Batch add of a block chain with callback on the last block
TEST (block_processor, add_many)
{
	nano::test::system system;
	auto & node = *system.add_node ();

	auto latest = nano::dev::genesis->hash ();
	auto balance = nano::dev::constants.genesis_amount;

	std::deque<std::shared_ptr<nano::block>> blocks;
	for (int n = 0; n < 5; ++n)
	{
		nano::keypair key;
		nano::block_builder builder;
		balance -= nano::Knano_ratio;
		auto send = builder
					.state ()
					.account (nano::dev::genesis_key.pub)
					.previous (latest)
					.representative (nano::dev::genesis_key.pub)
					.balance (balance)
					.link (key.pub)
					.sign (nano::dev::genesis_key.prv, nano::dev::genesis_key.pub)
					.work (*system.work.generate (latest))
					.build ();
		latest = send->hash ();
		blocks.push_back (send);
	}

	std::atomic<bool> callback_called{ false };
	std::atomic<nano::block_status> callback_status{};

	// All blocks queued under a single lock, callback attached to the last block
	auto result = node.block_processor.add_many (blocks, nano::block_source::live, nullptr, [&] (nano::block_status status) {
		callback_status = status;
		callback_called = true;
	});
	ASSERT_EQ (result.added, 5);
	ASSERT_EQ (result.dropped, 0);

	// Callback fires when the last block in the batch is processed
	ASSERT_TIMELY (5s, callback_called.load ());
	ASSERT_EQ (callback_status.load (), nano::block_status::progress);
	ASSERT_EQ (node.ledger.block_count (), 6);
	for (auto const & block : blocks)
	{
		ASSERT_TRUE (node.block_or_pruned_exists (block->hash ()));
	}
}

// Batch add with limited queue size, some blocks are dropped due to overfill
TEST (block_processor, add_many_overfill)
{
	nano::test::system system;

	nano::node_config node_config;
	// Only 4 blocks fit in the system queue, remaining will be dropped
	node_config.block_processor->max_system_queue = 4;
	auto & node = *system.add_node (node_config);

	auto latest = nano::dev::genesis->hash ();
	auto balance = nano::dev::constants.genesis_amount;

	std::deque<std::shared_ptr<nano::block>> blocks;
	for (int n = 0; n < 8; ++n)
	{
		nano::keypair key;
		nano::block_builder builder;
		balance -= nano::Knano_ratio;
		auto send = builder
					.state ()
					.account (nano::dev::genesis_key.pub)
					.previous (latest)
					.representative (nano::dev::genesis_key.pub)
					.balance (balance)
					.link (key.pub)
					.sign (nano::dev::genesis_key.prv, nano::dev::genesis_key.pub)
					.work (*system.work.generate (latest))
					.build ();
		latest = send->hash ();
		blocks.push_back (send);
	}

	// Not all blocks should fit, overfill stat tracks the dropped ones
	auto result = node.block_processor.add_many (blocks, nano::block_source::local);
	ASSERT_LT (result.added, 8);
	ASSERT_EQ (result.dropped, 8 - result.added);
	ASSERT_GT (node.stats.count (nano::stat::type::block_processor, nano::stat::detail::overfill), 0);
}

// Batch add with a mix of valid and invalid work blocks, only valid ones are queued
TEST (block_processor, add_many_mixed_work)
{
	nano::test::system system;
	auto & node = *system.add_node ();

	auto latest = nano::dev::genesis->hash ();
	auto balance = nano::dev::constants.genesis_amount;

	// Use deterministic keys so block hashes (and therefore roots) are fixed across runs
	nano::keypair key1{ "1" };
	nano::keypair key2{ "2" };
	nano::keypair key3{ "3" };
	nano::block_builder builder;

	// Block 1: valid work
	balance -= nano::Knano_ratio;
	auto send1 = builder
				 .state ()
				 .account (nano::dev::genesis_key.pub)
				 .previous (latest)
				 .representative (nano::dev::genesis_key.pub)
				 .balance (balance)
				 .link (key1.pub)
				 .sign (nano::dev::genesis_key.prv, nano::dev::genesis_key.pub)
				 .work (*system.work.generate (latest))
				 .build ();

	// Block 2: invalid work (will be skipped)
	balance -= nano::Knano_ratio;
	auto send2 = builder
				 .state ()
				 .account (nano::dev::genesis_key.pub)
				 .previous (send1->hash ())
				 .representative (nano::dev::genesis_key.pub)
				 .balance (balance)
				 .link (key2.pub)
				 .sign (nano::dev::genesis_key.prv, nano::dev::genesis_key.pub)
				 .work (0)
				 .build ();

	// Block 3: valid work (will hit gap_previous since block 2 was skipped)
	balance -= nano::Knano_ratio;
	auto send3 = builder
				 .state ()
				 .account (nano::dev::genesis_key.pub)
				 .previous (send2->hash ())
				 .representative (nano::dev::genesis_key.pub)
				 .balance (balance)
				 .link (key3.pub)
				 .sign (nano::dev::genesis_key.prv, nano::dev::genesis_key.pub)
				 .work (*system.work.generate (send2->hash ()))
				 .build ();

	auto result = node.block_processor.add_many ({ send1, send2, send3 }, nano::block_source::live);

	// Only 2 blocks should be queued (send2 has invalid work)
	ASSERT_EQ (result.added, 2);
	ASSERT_EQ (result.dropped, 1);
	ASSERT_EQ (node.stats.count (nano::stat::type::block_processor, nano::stat::detail::insufficient_work), 1);

	// Block 1 should be processed, block 2 was never queued
	ASSERT_TIMELY (5s, node.block_or_pruned_exists (send1->hash ()));
	ASSERT_FALSE (node.block_or_pruned_exists (send2->hash ()));
}

// Batch add with empty deque is a no-op
TEST (block_processor, add_many_empty)
{
	nano::test::system system;
	auto & node = *system.add_node ();

	std::deque<std::shared_ptr<nano::block>> empty;
	auto result = node.block_processor.add_many (empty, nano::block_source::live);
	ASSERT_EQ (result.added, 0);
	ASSERT_EQ (result.dropped, 0);
	ASSERT_EQ (node.ledger.block_count (), 1);
}

/*
 * Tests that blocks are queued in separate fair queue buckets per (source, channel) origin and
 * that the size overloads read a single bucket, not an aggregate.
 */
TEST (block_processor, size_per_channel)
{
	nano::test::system system;
	auto & node = *system.add_node ();

	// Freeze processing so queued blocks stay in place for deterministic size checks
	node.block_processor.stop ();

	auto channel1 = node.create_null_channel ();
	auto channel2 = node.create_null_channel ();

	auto latest = nano::dev::genesis->hash ();
	auto balance = nano::dev::constants.genesis_amount;
	auto make_send = [&] () {
		nano::keypair key;
		nano::block_builder builder;
		balance -= nano::Knano_ratio;
		auto send = builder
					.state ()
					.account (nano::dev::genesis_key.pub)
					.previous (latest)
					.representative (nano::dev::genesis_key.pub)
					.balance (balance)
					.link (key.pub)
					.sign (nano::dev::genesis_key.prv, nano::dev::genesis_key.pub)
					.work (*system.work.generate (latest))
					.build ();
		latest = send->hash ();
		return send;
	};

	// Same source split across two channel keys plus the null channel: three distinct buckets
	ASSERT_TRUE (node.block_processor.add (make_send (), nano::block_source::bootstrap, channel1));
	ASSERT_TRUE (node.block_processor.add (make_send (), nano::block_source::bootstrap, channel1));
	ASSERT_TRUE (node.block_processor.add (make_send (), nano::block_source::bootstrap, channel1));
	ASSERT_TRUE (node.block_processor.add (make_send (), nano::block_source::bootstrap, channel2));
	ASSERT_TRUE (node.block_processor.add (make_send (), nano::block_source::bootstrap, channel2));
	ASSERT_TRUE (node.block_processor.add (make_send (), nano::block_source::bootstrap));

	ASSERT_EQ (node.block_processor.size (nano::block_source::bootstrap, channel1), 3);
	ASSERT_EQ (node.block_processor.size (nano::block_source::bootstrap, channel2), 2);
	// The single argument overload reads the null channel bucket, not the sum across channels
	ASSERT_EQ (node.block_processor.size (nano::block_source::bootstrap), 1);
	ASSERT_EQ (node.block_processor.size (), 6);

	// Same channel under a different source is yet another bucket
	ASSERT_EQ (node.block_processor.size (nano::block_source::live, channel1), 0);
}

TEST (block_processor, backlog_throttling)
{
	nano::test::system system;

	nano::node_config node_config;
	// Backlog won't be rolled back, as we want to test the throttling works when backlog is exceeded
	node_config.max_backlog = 5;
	node_config.backlog_scan->enable = false;
	node_config.bounded_backlog->enable = false; // Disable rollbacks
	// Allow at most 4 blocks per second when throttling
	node_config.block_processor->batch_size = 1;
	node_config.block_processor->backlog_throttle = 500ms;
	auto & node = *system.add_node (node_config);

	const int howmany_blocks = 2;
	const int howmany_chains = 5;

	auto chains = nano::test::setup_chains (system, node, howmany_chains, howmany_blocks, nano::dev::genesis_key, /* do not confirm */ false);

	ASSERT_TIMELY_EQ (20s, node.ledger.block_count (), 21);

	// Prepare live spam blocks
	int const spam_count = 20;

	auto latest = node.latest (nano::dev::genesis_key.pub);
	auto balance = node.balance (nano::dev::genesis_key.pub);

	std::vector<std::shared_ptr<nano::block>> blocks;
	for (int n = 0; n < spam_count; ++n)
	{
		nano::keypair throwaway;
		nano::block_builder builder;

		balance -= 1;
		auto send = builder
					.state ()
					.account (nano::dev::genesis_key.pub)
					.previous (latest)
					.representative (nano::dev::genesis_key.pub)
					.balance (balance)
					.link (throwaway.pub)
					.sign (nano::dev::genesis_key.prv, nano::dev::genesis_key.pub)
					.work (*system.work.generate (latest))
					.build ();

		latest = send->hash ();

		blocks.push_back (send);
	}

	// Send them to the node as live blocks
	nano::test::process_live (node, blocks);

	// Ensure no more than 10 blocks are processed in 5 seconds
	ASSERT_NEVER (5s, node.ledger.block_count () > 21 + 10);
}

// Fast production-path development gates; full core/RPC/CLI acceptance is separate.
TEST (block_processor, signature_writer_state_batch)
{
	nano::test::system system;
	auto & node = *system.add_node ();
	nano::block_builder builder;
	nano::keypair destination, wrong_signer;
	auto previous = nano::dev::genesis->hash ();
	auto balance = nano::dev::constants.genesis_amount;
	std::deque<std::shared_ptr<nano::block>> blocks;
	for (unsigned i = 0; i < 3; ++i)
	{
		auto block = builder.state ().account (nano::dev::genesis_key.pub).previous (previous).representative (nano::dev::genesis_key.pub).balance (--balance).link (destination.pub).sign (nano::dev::genesis_key.prv, nano::dev::genesis_key.pub).work (*system.work.generate (previous)).build ();
		previous = block->hash ();
		blocks.push_back (block);
	}
	auto bad_key = builder.state ().account (nano::dev::genesis_key.pub).previous (previous).representative (nano::dev::genesis_key.pub).balance (balance - 1).link (destination.pub).sign (wrong_signer.prv, wrong_signer.pub).work (*system.work.generate (previous)).build ();
	auto bad_hash = builder.state ().account (nano::dev::genesis_key.pub).previous (previous).representative (nano::dev::genesis_key.pub).balance (balance - 2).link (destination.pub).sign (nano::dev::genesis_key.prv, nano::dev::genesis_key.pub).work (*system.work.generate (previous)).build ();
	bad_hash->signature_set (blocks.back ()->block_signature ()); // Raw malformed signed input.
	blocks.push_back (bad_key);
	blocks.push_back (bad_hash);
	auto batch_duplicate = builder.state ().account (nano::dev::genesis_key.pub).previous (nano::dev::genesis->hash ()).representative (nano::dev::genesis_key.pub).balance (nano::dev::constants.genesis_amount - 1).link (destination.pub).sign (wrong_signer.prv, wrong_signer.pub).work (*system.work.generate (nano::dev::genesis->hash ())).build ();
	blocks.push_back (batch_duplicate); // Negative preverification cannot override writer-owned old status.
	ASSERT_EQ (node.block_processor.add_many (blocks, nano::block_source::local).added, 6);
	ASSERT_TIMELY (2s, node.block_or_pruned_exists (previous));
	ASSERT_TIMELY (2s, node.stats.count (nano::stat::type::block_processor_result, nano::stat::detail::bad_signature) == 2);
	ASSERT_TIMELY (2s, node.stats.count (nano::stat::type::block_processor_result, nano::stat::detail::old) == 1);
	ASSERT_EQ (node.balance (nano::dev::genesis_key.pub), balance);
	ASSERT_EQ (node.ledger.block_count (), 4);
	ASSERT_FALSE (node.block_or_pruned_exists (bad_key->hash ()));
	ASSERT_FALSE (node.block_or_pruned_exists (bad_hash->hash ()));
	// Stored duplicates remain old even with a changed signature.
	auto old = builder.state ().account (nano::dev::genesis_key.pub).previous (nano::dev::genesis->hash ()).representative (nano::dev::genesis_key.pub).balance (nano::dev::constants.genesis_amount - 1).link (destination.pub).sign (wrong_signer.prv, wrong_signer.pub).work (*system.work.generate (nano::dev::genesis->hash ())).build ();
	ASSERT_EQ (node.block_processor.add_blocking (old, nano::block_source::local), nano::block_status::old);
	ASSERT_EQ (node.balance (nano::dev::genesis_key.pub), balance);
	system.stop ();
}

TEST (block_processor, signature_writer_proof_binding)
{
	static_assert (!std::is_default_constructible_v<nano::state_signature_verification>);
	static_assert (!std::is_constructible_v<nano::state_signature_verification, nano::state_block const &, bool>);
	nano::test::system system;
	auto & node = *system.add_node ();
	nano::block_builder builder;
	nano::keypair destination, wrong_key;
	auto const previous = nano::dev::genesis->hash ();
	auto const balance = nano::dev::constants.genesis_amount;
	auto valid = builder.state ().account (nano::dev::genesis_key.pub).previous (previous).representative (nano::dev::genesis_key.pub).balance (balance - 1).link (destination.pub).sign (nano::dev::genesis_key.prv, nano::dev::genesis_key.pub).work (*system.work.generate (previous)).build ();
	// The ledger's own optional-proof contract: proof comes from its real upstream
	// cryptographic producer, never an assembled boolean/hash/cache entry.
	auto proof = nano::state_signature_verification::verify (*valid);
	for (unsigned change = 0; change < 4; ++change)
	{
		auto input = builder.state ().account (change == 2 ? wrong_key.pub : nano::dev::genesis_key.pub).previous (previous).representative (nano::dev::genesis_key.pub).balance (balance - (change == 1 ? 2 : 1)).link (destination.pub).sign (wrong_key.prv, wrong_key.pub).work (*system.work.generate (previous)).build ();
		if (change == 1 || change == 2)
		{
			input->signature_set (valid->signature);
		}
		{
			auto transaction = node.ledger.tx_begin_write ();
			ASSERT_EQ (node.ledger.process (transaction, input, change == 3 ? nullptr : &proof), nano::block_status::bad_signature);
		}
		ASSERT_FALSE (node.block_or_pruned_exists (input->hash ()));
		ASSERT_EQ (node.balance (nano::dev::genesis_key.pub), balance);
		ASSERT_EQ (node.ledger.block_count (), 1);
	}
	{
		auto transaction = node.ledger.tx_begin_write ();
		ASSERT_EQ (node.ledger.process (transaction, valid, &proof), nano::block_status::progress);
	}
	ASSERT_TRUE (node.block_or_pruned_exists (valid->hash ()));
	ASSERT_EQ (node.balance (nano::dev::genesis_key.pub), balance - 1);
	system.stop ();
}

TEST (block_processor, signature_writer_excluded_paths)
{
	nano::test::system system;
	auto & node = *system.add_node ();
	nano::block_builder builder;
	nano::keypair destination, other;
	auto const balance = nano::dev::constants.genesis_amount;
	auto const genesis = nano::dev::genesis->hash ();
	auto legacy = builder.send ().previous (genesis).destination (destination.pub).balance (balance - 1).sign (nano::dev::genesis_key.prv, nano::dev::genesis_key.pub).work (*system.work.generate (genesis)).build ();
	ASSERT_EQ (node.block_processor.add_blocking (legacy, nano::block_source::local), nano::block_status::progress);
	auto ordinary = builder.state ().account (nano::dev::genesis_key.pub).previous (legacy->hash ()).representative (nano::dev::genesis_key.pub).balance (balance - 2).link (destination.pub).sign (nano::dev::genesis_key.prv, nano::dev::genesis_key.pub).work (*system.work.generate (legacy->hash ())).build ();
	ASSERT_EQ (node.block_processor.add_blocking (ordinary, nano::block_source::local), nano::block_status::progress);
	auto forced = builder.state ().account (nano::dev::genesis_key.pub).previous (legacy->hash ()).representative (nano::dev::genesis_key.pub).balance (balance - 3).link (other.pub).sign (nano::dev::genesis_key.prv, nano::dev::genesis_key.pub).work (*system.work.generate (legacy->hash ())).build ();
	node.block_processor.force (forced);
	ASSERT_TIMELY (2s, node.block_or_pruned_exists (forced->hash ()) && !node.block_or_pruned_exists (ordinary->hash ()));
	ASSERT_EQ (node.balance (nano::dev::genesis_key.pub), balance - 3);
	auto epoch_open = builder.state ().account (destination.pub).previous (0).representative (0).balance (0).link (node.ledger.epoch_link (nano::epoch::epoch_1)).sign (nano::dev::genesis_key.prv, nano::dev::genesis_key.pub).work (*system.work.generate (destination.pub)).build ();
	ASSERT_EQ (node.block_processor.add_blocking (epoch_open, nano::block_source::local), nano::block_status::progress);
	ASSERT_TRUE (node.block_or_pruned_exists (epoch_open->hash ()));
	auto read = node.ledger.tx_begin_read ();
	auto info = node.ledger.any.account_get (read, destination.pub);
	ASSERT_TRUE (info);
	ASSERT_EQ (info->epoch (), nano::epoch::epoch_1);
	system.stop ();
}

TEST (block_processor, signature_writer_pruned_duplicate)
{
	nano::test::system system;
	auto config = system.default_config ();
	config.enable_voting = false; // Required by the production pruning mode.
	nano::node_flags flags;
	flags.enable_pruning = true;
	flags.disable_topo_index = true; // Production pruning/index compatibility contract.
	auto & node = *system.add_node (config, flags);
	nano::block_builder builder;
	nano::keypair destination, wrong_key;
	auto const genesis = nano::dev::genesis->hash ();
	auto send = builder.state ().account (nano::dev::genesis_key.pub).previous (genesis).representative (nano::dev::genesis_key.pub).balance (nano::dev::constants.genesis_amount - 1).link (destination.pub).sign (nano::dev::genesis_key.prv, nano::dev::genesis_key.pub).work (*system.work.generate (genesis)).build ();
	ASSERT_EQ (node.block_processor.add_blocking (send, nano::block_source::local), nano::block_status::progress);
	auto successor = builder.state ().account (nano::dev::genesis_key.pub).previous (send->hash ()).representative (nano::dev::genesis_key.pub).balance (nano::dev::constants.genesis_amount - 2).link (destination.pub).sign (nano::dev::genesis_key.prv, nano::dev::genesis_key.pub).work (*system.work.generate (send->hash ())).build ();
	ASSERT_EQ (node.block_processor.add_blocking (successor, nano::block_source::local), nano::block_status::progress);
	{
		auto write = node.ledger.tx_begin_write ();
		node.ledger.cement (write, send->hash ());
		ASSERT_EQ (node.ledger.pruning_action (write, send->hash (), 128), 1);
	}
	ASSERT_TRUE (node.block_or_pruned_exists (send->hash ()));
	ASSERT_EQ (node.block (send->hash ()), nullptr);
	auto duplicate = builder.state ().account (nano::dev::genesis_key.pub).previous (genesis).representative (nano::dev::genesis_key.pub).balance (nano::dev::constants.genesis_amount - 1).link (destination.pub).sign (wrong_key.prv, wrong_key.pub).work (*system.work.generate (genesis)).build ();
	ASSERT_EQ (node.block_processor.add_blocking (duplicate, nano::block_source::local), nano::block_status::old);
	ASSERT_EQ (node.ledger.pruned_count (), 1);
	ASSERT_EQ (node.balance (nano::dev::genesis_key.pub), nano::dev::constants.genesis_amount - 2);
	system.stop ();
}
