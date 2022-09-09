#include <nano/node/lmdb/block_store.hpp>
#include <nano/node/lmdb/lmdb.hpp>
#include <nano/secure/parallel_traversal.hpp>

nano::store_iterator<nano::block_hash, nano::block_w_sideband> to_block_iterator (rsnano::LmdbIteratorHandle * it_handle)
{
	if (it_handle == nullptr)
	{
		return nano::store_iterator<nano::block_hash, nano::block_w_sideband> (nullptr);
	}

	return nano::store_iterator<nano::block_hash, nano::block_w_sideband> (
	std::make_unique<nano::mdb_iterator<nano::block_hash, nano::block_w_sideband>> (it_handle));
}

nano::lmdb::block_store::block_store (nano::lmdb::store & store_a) :
	handle{ rsnano::rsn_lmdb_block_store_create (store_a.env ().handle) },
	store{ store_a } {};

nano::lmdb::block_store::~block_store ()
{
	rsnano::rsn_lmdb_block_store_destroy (handle);
}

void nano::lmdb::block_store::put (nano::write_transaction const & transaction, nano::block_hash const & hash, nano::block const & block)
{
	rsnano::rsn_lmdb_block_store_put (handle, transaction.get_rust_handle (), hash.bytes.data (), block.get_handle ());
}

void nano::lmdb::block_store::raw_put (nano::write_transaction const & transaction_a, std::vector<uint8_t> const & data, nano::block_hash const & hash_a)
{
	rsnano::rsn_lmdb_block_store_raw_put (handle, transaction_a.get_rust_handle (), data.data (), data.size (), hash_a.bytes.data ());
}

nano::block_hash nano::lmdb::block_store::successor (nano::transaction const & transaction_a, nano::block_hash const & hash_a) const
{
	nano::block_hash result;
	rsnano::rsn_lmdb_block_store_successor (handle, transaction_a.get_rust_handle (), hash_a.bytes.data (), result.bytes.data ());
	return result;
}

void nano::lmdb::block_store::successor_clear (nano::write_transaction const & transaction, nano::block_hash const & hash)
{
	rsnano::rsn_lmdb_block_store_successor_clear (handle, transaction.get_rust_handle (), hash.bytes.data ());
}

std::shared_ptr<nano::block> nano::lmdb::block_store::get (nano::transaction const & transaction, nano::block_hash const & hash) const
{
	auto block_handle = rsnano::rsn_lmdb_block_store_get (handle, transaction.get_rust_handle (), hash.bytes.data ());
	return nano::block_handle_to_block (block_handle);
}

std::shared_ptr<nano::block> nano::lmdb::block_store::get_no_sideband (nano::transaction const & transaction, nano::block_hash const & hash) const
{
	auto block_handle = rsnano::rsn_lmdb_block_store_get_no_sideband (handle, transaction.get_rust_handle (), hash.bytes.data ());
	return nano::block_handle_to_block (block_handle);
}

std::shared_ptr<nano::block> nano::lmdb::block_store::random (nano::transaction const & transaction)
{
	std::shared_ptr<nano::block> result;
	auto block_handle = rsnano::rsn_lmdb_block_store_random (handle, transaction.get_rust_handle ());
	if (block_handle != nullptr)
	{
		result = std::move (nano::block_handle_to_block (block_handle));
	}
	return result;
}

void nano::lmdb::block_store::del (nano::write_transaction const & transaction_a, nano::block_hash const & hash_a)
{
	rsnano::rsn_lmdb_block_store_del (handle, transaction_a.get_rust_handle (), hash_a.bytes.data ());
}

bool nano::lmdb::block_store::exists (nano::transaction const & transaction, nano::block_hash const & hash)
{
	return rsnano::rsn_lmdb_block_store_exists (handle, transaction.get_rust_handle (), hash.bytes.data ());
}

uint64_t nano::lmdb::block_store::count (nano::transaction const & transaction_a)
{
	return rsnano::rsn_lmdb_block_store_count (handle, transaction_a.get_rust_handle ());
}

nano::account nano::lmdb::block_store::account (nano::transaction const & transaction_a, nano::block_hash const & hash_a) const
{
	nano::account result;
	rsnano::rsn_lmdb_block_store_account (handle, transaction_a.get_rust_handle (), hash_a.bytes.data (), result.bytes.data ());
	return result;
}

nano::account nano::lmdb::block_store::account_calculated (nano::block const & block_a) const
{
	nano::account result;
	rsnano::rsn_lmdb_block_store_account_calculated (handle, block_a.get_handle (), result.bytes.data ());
	return result;
}

nano::store_iterator<nano::block_hash, nano::block_w_sideband> nano::lmdb::block_store::begin (nano::transaction const & transaction) const
{
	auto it_handle{ rsnano::rsn_lmdb_block_store_begin (handle, transaction.get_rust_handle ()) };
	return to_block_iterator (it_handle);
}

nano::store_iterator<nano::block_hash, nano::block_w_sideband> nano::lmdb::block_store::begin (nano::transaction const & transaction, nano::block_hash const & hash) const
{
	auto it_handle{ rsnano::rsn_lmdb_block_store_begin_at_hash (handle, transaction.get_rust_handle (), hash.bytes.data ()) };
	return to_block_iterator (it_handle);
}

nano::store_iterator<nano::block_hash, nano::block_w_sideband> nano::lmdb::block_store::end () const
{
	return nano::store_iterator<nano::block_hash, nano::block_w_sideband> (nullptr);
}

nano::uint128_t nano::lmdb::block_store::balance (nano::transaction const & transaction_a, nano::block_hash const & hash_a)
{
	nano::amount result;
	rsnano::rsn_lmdb_block_store_balance (handle, transaction_a.get_rust_handle (), hash_a.bytes.data (), result.bytes.data ());
	return result.number ();
}

nano::uint128_t nano::lmdb::block_store::balance_calculated (std::shared_ptr<nano::block> const & block_a) const
{
	nano::amount result;
	rsnano::rsn_lmdb_block_store_balance_calculated (handle, block_a->get_handle (), result.bytes.data ());
	return result.number ();
}

nano::epoch nano::lmdb::block_store::version (nano::transaction const & transaction_a, nano::block_hash const & hash_a)
{
	auto block = get (transaction_a, hash_a);
	if (block && block->type () == nano::block_type::state)
	{
		return block->sideband ().details ().epoch ();
	}

	return nano::epoch::epoch_0;
}

void nano::lmdb::block_store::for_each_par (std::function<void (nano::read_transaction const &, nano::store_iterator<nano::block_hash, block_w_sideband>, nano::store_iterator<nano::block_hash, block_w_sideband>)> const & action_a) const
{
	parallel_traversal<nano::uint256_t> (
	[&action_a, this] (nano::uint256_t const & start, nano::uint256_t const & end, bool const is_last) {
		auto transaction (this->store.tx_begin_read ());
		action_a (*transaction, this->begin (*transaction, start), !is_last ? this->begin (*transaction, end) : this->end ());
	});
}

// Converts a block hash to a block height
uint64_t nano::lmdb::block_store::account_height (nano::transaction const & transaction_a, nano::block_hash const & hash_a) const
{
	auto block = get (transaction_a, hash_a);
	return block->sideband ().height ();
}

MDB_dbi nano::lmdb::block_store::get_blocks_handle () const
{
	return rsnano::rsn_lmdb_block_store_blocks_handle (handle);
}

void nano::lmdb::block_store::set_blocks_handle (MDB_dbi dbi)
{
	rsnano::rsn_lmdb_block_store_set_blocks_handle (handle, dbi);
}

void nano::lmdb::block_store::block_raw_get (nano::transaction const & transaction, nano::block_hash const & hash, nano::mdb_val & value) const
{
	rsnano::rsn_lmdb_block_store_block_raw_get (handle, transaction.get_rust_handle (), hash.bytes.data (), reinterpret_cast<rsnano::MdbVal *> (&value.value));
}

nano::block_type nano::lmdb::block_store::block_type_from_raw (void * data_a)
{
	// The block type is the first byte
	return static_cast<nano::block_type> ((reinterpret_cast<uint8_t const *> (data_a))[0]);
}
