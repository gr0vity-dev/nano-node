#include "nano/lib/threading.hpp"

#include <nano/node/lmdb/lmdb_env.hpp>

#include <boost/filesystem/operations.hpp>

nano::mdb_env::mdb_env (bool & error_a, boost::filesystem::path const & path_a, nano::mdb_env::options options_a)
{
	init (error_a, path_a, options_a);
}

void nano::mdb_env::init (bool & error_a, boost::filesystem::path const & path_a, nano::mdb_env::options options_a)
{
	boost::system::error_code error_mkdir, error_chmod;
	if (path_a.has_parent_path ())
	{
		boost::filesystem::create_directories (path_a.parent_path (), error_mkdir);
		nano::set_secure_perm_directory (path_a.parent_path (), error_chmod);
		if (!error_mkdir)
		{
			auto status1 (mdb_env_create (&environment));
			release_assert (status1 == 0);
			auto status2 (mdb_env_set_maxdbs (environment, options_a.config.max_databases));
			release_assert (status2 == 0);
			auto map_size = options_a.config.map_size;
			auto max_valgrind_map_size = 16 * 1024 * 1024;
			if (running_within_valgrind () && map_size > max_valgrind_map_size)
			{
				// In order to run LMDB under Valgrind, the maximum map size must be smaller than half your available RAM
				map_size = max_valgrind_map_size;
			}
			auto status3 (mdb_env_set_mapsize (environment, map_size));
			release_assert (status3 == 0);
			// It seems if there's ever more threads than mdb_env_set_maxreaders has read slots available, we get failures on transaction creation unless MDB_NOTLS is specified
			// This can happen if something like 256 io_threads are specified in the node config
			// MDB_NORDAHEAD will allow platforms that support it to load the DB in memory as needed.
			// MDB_NOMEMINIT prevents zeroing malloc'ed pages. Can provide improvement for non-sensitive data but may make memory checkers noisy (e.g valgrind).
			auto environment_flags = MDB_NOSUBDIR | MDB_NOTLS | MDB_NORDAHEAD;
			if (options_a.config.sync == nano::lmdb_config::sync_strategy::nosync_safe)
			{
				environment_flags |= MDB_NOMETASYNC;
			}
			else if (options_a.config.sync == nano::lmdb_config::sync_strategy::nosync_unsafe)
			{
				environment_flags |= MDB_NOSYNC;
			}
			else if (options_a.config.sync == nano::lmdb_config::sync_strategy::nosync_unsafe_large_memory)
			{
				environment_flags |= MDB_NOSYNC | MDB_WRITEMAP | MDB_MAPASYNC;
			}

			if (!running_within_valgrind () && options_a.use_no_mem_init)
			{
				environment_flags |= MDB_NOMEMINIT;
			}
			auto status4 (mdb_env_open (environment, path_a.string ().c_str (), environment_flags, 00600));
			if (status4 != 0)
			{
				std::cerr << "Could not open lmdb environment: " << status4;
				char * error_str (mdb_strerror (status4));
				if (error_str)
				{
					std::cerr << ", " << error_str;
				}
				std::cerr << std::endl;
			}
			release_assert (status4 == 0);
			error_a = status4 != 0;
		}
		else
		{
			error_a = true;
			environment = nullptr;
		}
	}
	else
	{
		error_a = true;
		environment = nullptr;
	}
}

nano::mdb_env::~mdb_env ()
{
	if (environment != nullptr)
	{
		// Make sure the commits are flushed. This is a no-op unless MDB_NOSYNC is used.
		mdb_env_sync (environment, true);
		mdb_env_close (environment);
	}
}

nano::mdb_env::operator MDB_env * () const
{
	return environment;
}

std::unique_ptr<nano::read_transaction> nano::mdb_env::tx_begin_read (mdb_txn_callbacks mdb_txn_callbacks) const
{
	return std::make_unique<nano::read_mdb_txn> (next_txn_id++, environment, mdb_txn_callbacks);
}

std::unique_ptr<nano::write_transaction> nano::mdb_env::tx_begin_write (mdb_txn_callbacks mdb_txn_callbacks) const
{
	/*
	 * For IO threads, we do not want them to block on creating write transactions.
	 */
	debug_assert (nano::thread_role::get () != nano::thread_role::name::io);
	return std::make_unique<nano::write_mdb_txn> (next_txn_id++, environment, mdb_txn_callbacks);
}

MDB_txn * nano::mdb_env::tx (nano::transaction const & transaction_a) const
{
	return to_mdb_txn (transaction_a);
}

MDB_txn * nano::to_mdb_txn (nano::transaction const & transaction_a)
{
	return static_cast<MDB_txn *> (transaction_a.get_handle ());
}

void nano::assert_success (int const status)
{
	if (status != MDB_SUCCESS)
	{
		release_assert (false, mdb_strerror (status));
	}
}
