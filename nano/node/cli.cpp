#include <nano/lib/cli.hpp>
#include <nano/lib/tlsconfig.hpp>
#include <nano/lib/tomlconfig.hpp>
#include <nano/node/cli.hpp>
#include <nano/node/common.hpp>
#include <nano/node/daemonconfig.hpp>
#include <nano/node/node.hpp>

#include <boost/format.hpp>

namespace
{
void reset_confirmation_heights (nano::store::write_transaction const & transaction, nano::ledger_constants & constants, nano::store::component & store);
}

std::string nano::error_cli_messages::message (int ev) const
{
	switch (static_cast<nano::error_cli> (ev))
	{
		case nano::error_cli::generic:
			return "Unknown error";
		case nano::error_cli::parse_error:
			return "Coud not parse command line";
		case nano::error_cli::invalid_arguments:
			return "Invalid arguments";
		case nano::error_cli::unknown_command:
			return "Unknown command";
		case nano::error_cli::database_write_error:
			return "Database write error";
		case nano::error_cli::reading_config:
			return "Config file read error";
		case nano::error_cli::ambiguous_pruning_voting_options:
			return "Flag --enable_pruning and enable_voting in node config cannot be used together";
	}

	return "Invalid error code";
}

void nano::add_node_options (boost::program_options::options_description & description_a)
{
	// clang-format off
	description_a.add_options ()
	("initialize", "Initialize the data folder, if it is not already initialised. This command is meant to be run when the data folder is empty, to populate it with the genesis block.")
	("account_create", "Insert next deterministic key in to <wallet>")
	("account_get", "Get account number for the <key>")
	("account_key", "Get the public key for <account>")
	("vacuum", "Compact database. If data_path is missing, the database in data directory is compacted.")
	("snapshot", "Compact database and create snapshot, functions similar to vacuum but does not replace the existing database")
	("data_path", boost::program_options::value<std::string> (), "Use the supplied path as the data directory")
	("network", boost::program_options::value<std::string> (), "Use the supplied network (live, test, beta or dev)")
	("clear_send_ids", "Remove all send IDs from the database (dangerous: not intended for production use)")
	("online_weight_clear", "Clear online weight history records")
	("peer_clear", "Clear online peers database dump")
	("unchecked_clear", "Clear unchecked blocks")
	("confirmation_height_clear", "Clear confirmation height. Requires an <account> option that can be 'all' to clear all accounts")
	("final_vote_clear", "Clear final votes")
	("rebuild_database", "Rebuild LMDB database with vacuum for best compaction")
	("diagnostics", "Run internal diagnostics")
	("generate_config", boost::program_options::value<std::string> (), "Write configuration to stdout, populated with defaults suitable for this system. Pass the configuration type node, rpc or tls. See also use_defaults.")
	("key_create", "Generates a adhoc random keypair and prints it to stdout")
	("key_expand", "Derive public key and account number from <key>")
	("wallet_add_adhoc", "Insert <key> in to <wallet>")
	("wallet_create", "Creates a new wallet and prints the ID")
	("wallet_change_seed", "Changes seed for <wallet> to <key>")
	("wallet_decrypt_unsafe", "Decrypts <wallet> using <password>, !!THIS WILL PRINT YOUR PRIVATE KEY TO STDOUT!!")
	("wallet_destroy", "Destroys <wallet> and all keys it contains")
	("wallet_import", "Imports keys in <file> using <password> in to <wallet>")
	("wallet_list", "Dumps wallet IDs and public keys")
	("wallet_remove", "Remove <account> from <wallet>")
	("wallet_representative_get", "Prints default representative for <wallet>")
	("wallet_representative_set", "Set <account> as default representative for <wallet>")
	("all", "Only valid with --final_vote_clear")
	("account", boost::program_options::value<std::string> (), "Defines <account> for other commands")
	("root", boost::program_options::value<std::string> (), "Defines <root> for other commands")
	("file", boost::program_options::value<std::string> (), "Defines <file> for other commands")
	("key", boost::program_options::value<std::string> (), "Defines the <key> for other commands, hex")
	("seed", boost::program_options::value<std::string> (), "Defines the <seed> for other commands, hex")
	("password", boost::program_options::value<std::string> (), "Defines <password> for other commands")
	("wallet", boost::program_options::value<std::string> (), "Defines <wallet> for other commands")
	("force", boost::program_options::value<bool>(), "Bool to force command if allowed")
	("use_defaults", "If present, the generate_config command will generate uncommented entries");
	// clang-format on
}

void nano::add_node_flag_options (boost::program_options::options_description & description_a)
{
	// clang-format off
	description_a.add_options()
		("disable_add_initial_peers", "Disable contacting the peer in the peers table at startup")
		("disable_backup", "Disable wallet automatic backups")
		("disable_lazy_bootstrap", "Disables lazy bootstrap")
		("disable_legacy_bootstrap", "Disables legacy bootstrap")
		("disable_wallet_bootstrap", "Disables wallet lazy bootstrap")
		("disable_ongoing_bootstrap", "Disable ongoing bootstrap")
		("disable_ascending_bootstrap", "Disable ascending bootstrap")
		("disable_rep_crawler", "Disable rep crawler")
		("disable_request_loop", "Disable request loop")
		("disable_bootstrap_listener", "Disables bootstrap processing for TCP listener (not including realtime network TCP connections)")
		("disable_unchecked_cleanup", "Disables periodic cleanup of old records from unchecked table")
		("disable_unchecked_drop", "Disables drop of unchecked table at startup")
		("disable_providing_telemetry_metrics", "Disable using any node information in the telemetry_ack messages.")
		("disable_block_processor_unchecked_deletion", "Disable deletion of unchecked blocks after processing")
		("enable_pruning", "Enable experimental ledger pruning")
		("allow_bootstrap_peers_duplicates", "Allow multiple connections to same peer in bootstrap attempts")
		("fast_bootstrap", "Increase bootstrap speed for high end nodes with higher limits")
		("block_processor_batch_size", boost::program_options::value<std::size_t>(), "Increase block processor transaction batch write size, default 0 (limited by config block_processor_batch_max_time), 256k for fast_bootstrap")
		("block_processor_full_size", boost::program_options::value<std::size_t>(), "Increase block processor allowed blocks queue size before dropping live network packets and holding bootstrap download, default 65536, 1 million for fast_bootstrap")
		("block_processor_verification_size", boost::program_options::value<std::size_t>(), "Increase batch signature verification size in block processor, default 0 (limited by config signature_checker_threads), unlimited for fast_bootstrap")
		("inactive_votes_cache_size", boost::program_options::value<std::size_t>(), "Increase cached votes without active elections size, default 16384")
		("vote_processor_capacity", boost::program_options::value<std::size_t>(), "Vote processor queue size before dropping votes, default 144k")
		;
	// clang-format on
}

std::error_code nano::update_flags (nano::node_flags & flags_a, boost::program_options::variables_map const & vm)
{
	std::error_code ec;
	flags_a.set_disable_add_initial_peers (vm.count ("disable_add_initial_peers") > 0);
	flags_a.set_disable_backup (vm.count ("disable_backup") > 0);
	flags_a.set_disable_lazy_bootstrap (vm.count ("disable_lazy_bootstrap") > 0);
	flags_a.set_disable_legacy_bootstrap (vm.count ("disable_legacy_bootstrap") > 0);
	flags_a.set_disable_wallet_bootstrap (vm.count ("disable_wallet_bootstrap") > 0);
	flags_a.set_disable_ongoing_bootstrap (vm.count ("disable_ongoing_bootstrap") > 0);
	flags_a.set_disable_ascending_bootstrap (vm.count ("disable_ascending_bootstrap") > 0);
	flags_a.set_disable_rep_crawler (vm.count ("disable_rep_crawler") > 0);
	flags_a.set_disable_request_loop (vm.count ("disable_request_loop") > 0);
	if (!flags_a.inactive_node ())
	{
		flags_a.set_disable_bootstrap_listener (vm.count ("disable_bootstrap_listener") > 0);
	}
	flags_a.set_disable_providing_telemetry_metrics (vm.count ("disable_providing_telemetry_metrics") > 0);
	flags_a.set_disable_unchecked_cleanup (vm.count ("disable_unchecked_cleanup") > 0);
	flags_a.set_disable_unchecked_drop (vm.count ("disable_unchecked_drop") > 0);
	flags_a.set_disable_block_processor_unchecked_deletion (vm.count ("disable_block_processor_unchecked_deletion") > 0);
	flags_a.set_enable_pruning (vm.count ("enable_pruning") > 0);
	flags_a.set_allow_bootstrap_peers_duplicates (vm.count ("allow_bootstrap_peers_duplicates") > 0);
	flags_a.set_fast_bootstrap (vm.count ("fast_bootstrap") > 0);
	if (flags_a.fast_bootstrap ())
	{
		flags_a.set_disable_block_processor_unchecked_deletion (true);
		flags_a.set_block_processor_batch_size (256 * 1024);
		flags_a.set_block_processor_full_size (1024 * 1024);
		flags_a.set_block_processor_verification_size (std::numeric_limits<std::size_t>::max ());
	}
	auto block_processor_batch_size_it = vm.find ("block_processor_batch_size");
	if (block_processor_batch_size_it != vm.end ())
	{
		flags_a.set_block_processor_batch_size (block_processor_batch_size_it->second.as<std::size_t> ());
	}
	auto block_processor_full_size_it = vm.find ("block_processor_full_size");
	if (block_processor_full_size_it != vm.end ())
	{
		flags_a.set_block_processor_full_size (block_processor_full_size_it->second.as<std::size_t> ());
	}
	auto block_processor_verification_size_it = vm.find ("block_processor_verification_size");
	if (block_processor_verification_size_it != vm.end ())
	{
		flags_a.set_block_processor_verification_size (block_processor_verification_size_it->second.as<std::size_t> ());
	}
	auto vote_processor_capacity_it = vm.find ("vote_processor_capacity");
	if (vote_processor_capacity_it != vm.end ())
	{
		flags_a.set_vote_processor_capacity (vote_processor_capacity_it->second.as<std::size_t> ());
	}
	// Config overriding
	auto config (vm.find ("config"));
	if (config != vm.end ())
	{
		flags_a.set_config_overrides (nano::config_overrides (config->second.as<std::vector<nano::config_key_value_pair>> ()));
	}
	auto rpcconfig (vm.find ("rpcconfig"));
	if (rpcconfig != vm.end ())
	{
		flags_a.set_rpc_overrides (nano::config_overrides (rpcconfig->second.as<std::vector<nano::config_key_value_pair>> ()));
	}
	return ec;
}

std::error_code nano::flags_config_conflicts (nano::node_flags const & flags_a, nano::node_config const & config_a)
{
	std::error_code ec;
	if (flags_a.enable_pruning () && config_a.enable_voting)
	{
		ec = nano::error_cli::ambiguous_pruning_voting_options;
	}
	return ec;
}

namespace
{
void database_write_lock_error (std::error_code & ec)
{
	std::cerr << "Write database error, this cannot be run while the node is already running\n";
	ec = nano::error_cli::database_write_error;
}

bool copy_database (std::filesystem::path const & data_path, boost::program_options::variables_map const & vm, std::filesystem::path const & output_path, std::error_code & ec)
{
	bool success = false;
	bool needs_to_write = vm.count ("unchecked_clear") || vm.count ("clear_send_ids") || vm.count ("online_weight_clear") || vm.count ("peer_clear") || vm.count ("confirmation_height_clear") || vm.count ("final_vote_clear") || vm.count ("rebuild_database");

	auto node_flags = nano::inactive_node_flag_defaults ();
	node_flags.set_read_only (!needs_to_write);
	nano::update_flags (node_flags, vm);
	nano::inactive_node node (data_path, node_flags);
	if (!node.node->init_error ())
	{
		auto & store (node.node->store);
		if (vm.count ("unchecked_clear"))
		{
			node.node->unchecked.clear ();
		}
		if (vm.count ("clear_send_ids"))
		{
			auto tx{ node.node->wallets.tx_begin_write () };
			node.node->wallets.clear_send_ids (*tx);
		}
		if (vm.count ("online_weight_clear"))
		{
			auto tx{ store.tx_begin_write () };
			node.node->store.online_weight ().clear (*tx);
		}
		if (vm.count ("peer_clear"))
		{
			auto tx{ store.tx_begin_write () };
			node.node->store.peer ().clear (*tx);
		}
		if (vm.count ("confirmation_height_clear"))
		{
			auto tx{ store.tx_begin_write () };
			reset_confirmation_heights (*tx, node.node->network_params.ledger, store);
		}
		if (vm.count ("final_vote_clear"))
		{
			auto tx{ store.tx_begin_write () };
			node.node->store.final_vote ().clear (*tx);
		}
		if (vm.count ("rebuild_database"))
		{
			auto tx{ store.tx_begin_write () };
			node.node->store.rebuild_db (*tx);
		}

		success = node.node->copy_with_compaction (output_path);
	}
	else
	{
		database_write_lock_error (ec);
	}
	return success;
}
}

namespace
{
void set_cli_wallets_error (nano::wallets_error error, std::error_code & ec)
{
	switch (error)
	{
		case nano::wallets_error::none:
			break;
		case nano::wallets_error::invalid_password:
			std::cerr << "Invalid password\n";
			ec = nano::error_cli::invalid_arguments;
			break;
		case nano::wallets_error::wallet_not_found:
			std::cerr << "Wallet doesn't exist\n";
			ec = nano::error_cli::invalid_arguments;
			break;
		default:
			std::cerr << "Unknown error\n";
			ec = nano::error_cli::generic;
			break;
	}
}
}

std::error_code nano::handle_node_options (boost::program_options::variables_map const & vm)
{
	std::error_code ec;
	std::filesystem::path data_path = vm.count ("data_path") ? std::filesystem::path (vm["data_path"].as<std::string> ()) : nano::working_path ();

	if (vm.count ("initialize"))
	{
		auto node_flags = nano::inactive_node_flag_defaults ();
		node_flags.set_read_only (false);
		nano::update_flags (node_flags, vm);
		nano::inactive_node node (data_path, node_flags);
	}
	else if (vm.count ("account_create"))
	{
		if (vm.count ("wallet") == 1)
		{
			nano::wallet_id wallet_id;
			if (!wallet_id.decode_hex (vm["wallet"].as<std::string> ()))
			{
				std::string password;
				if (vm.count ("password") > 0)
				{
					password = vm["password"].as<std::string> ();
				}
				auto inactive_node = nano::default_inactive_node (data_path, vm);
				auto & wallets = inactive_node->node->wallets;
				auto error = wallets.enter_password (wallet_id, password);
				if (error == nano::wallets_error::none)
				{
					auto pub{ wallets.deterministic_insert (wallet_id) };
					std::cout << boost::str (boost::format ("Account: %1%\n") % pub.to_account ());
				}
				set_cli_wallets_error (error, ec);
			}
			else
			{
				std::cerr << "Invalid wallet id\n";
				ec = nano::error_cli::invalid_arguments;
			}
		}
		else
		{
			std::cerr << "account_create command requires one <wallet> option and optionally one <password> option\n";
			ec = nano::error_cli::invalid_arguments;
		}
	}
	else if (vm.count ("account_get") > 0)
	{
		if (vm.count ("key") == 1)
		{
			nano::account pub;
			pub.decode_hex (vm["key"].as<std::string> ());
			std::cout << "Account: " << pub.to_account () << std::endl;
		}
		else
		{
			std::cerr << "account comand requires one <key> option\n";
			ec = nano::error_cli::invalid_arguments;
		}
	}
	else if (vm.count ("account_key") > 0)
	{
		if (vm.count ("account") == 1)
		{
			nano::account account;
			account.decode_account (vm["account"].as<std::string> ());
			std::cout << "Hex: " << account.to_string () << std::endl;
		}
		else
		{
			std::cerr << "account_key command requires one <account> option\n";
			ec = nano::error_cli::invalid_arguments;
		}
	}
	else if (vm.count ("vacuum") > 0)
	{
		try
		{
			if (!ec)
			{
				std::cout << "Vacuuming database copy in ";
				std::filesystem::path source_path;
				std::filesystem::path backup_path;
				std::filesystem::path vacuum_path;
				source_path = data_path / "data.ldb";
				backup_path = data_path / "backup.vacuum.ldb";
				vacuum_path = data_path / "vacuumed.ldb";
				std::cout << data_path << "\n";
				std::cout << "This may take a while..." << std::endl;

				bool success = copy_database (data_path, vm, vacuum_path, ec);
				if (success)
				{
					// Note that these throw on failure
					std::cout << "Finalizing" << std::endl;
					std::filesystem::remove (backup_path);
					std::filesystem::rename (source_path, backup_path);
					std::filesystem::rename (vacuum_path, source_path);
					std::cout << "Vacuum completed" << std::endl;
				}
				else
				{
					std::cerr << "Vacuum failed (copying returned false)" << std::endl;
				}
			}
			else
			{
				std::cerr << "Vacuum failed. RocksDB is enabled but the node has not been built with RocksDB support" << std::endl;
			}
		}
		catch (std::filesystem::filesystem_error const & ex)
		{
			std::cerr << "Vacuum failed during a file operation: " << ex.what () << std::endl;
		}
		catch (...)
		{
			std::cerr << "Vacuum failed (unknown reason)" << std::endl;
		}
	}
	else if (vm.count ("snapshot"))
	{
		try
		{
			if (!ec)
			{
				std::filesystem::path source_path;
				std::filesystem::path snapshot_path;
				source_path = data_path / "data.ldb";
				snapshot_path = data_path / "snapshot.ldb";

				std::cout << "Database snapshot of " << source_path << " to " << snapshot_path << " in progress" << std::endl;
				std::cout << "This may take a while..." << std::endl;

				bool success = copy_database (data_path, vm, snapshot_path, ec);
				if (success)
				{
					std::cout << "Snapshot completed, This can be found at " << snapshot_path << std::endl;
				}
				else
				{
					std::cerr << "Snapshot failed (copying returned false)" << std::endl;
				}
			}
			else
			{
				std::cerr << "Snapshot failed. RocksDB is enabled but the node has not been built with RocksDB support" << std::endl;
			}
		}
		catch (std::filesystem::filesystem_error const & ex)
		{
			std::cerr << "Snapshot failed during a file operation: " << ex.what () << std::endl;
		}
		catch (...)
		{
			std::cerr << "Snapshot failed (unknown reason)" << std::endl;
		}
	}
	else if (vm.count ("unchecked_clear"))
	{
		std::filesystem::path data_path = vm.count ("data_path") ? std::filesystem::path (vm["data_path"].as<std::string> ()) : nano::working_path ();
		auto node_flags = nano::inactive_node_flag_defaults ();
		node_flags.set_read_only (false);
		nano::update_flags (node_flags, vm);
		nano::inactive_node node (data_path, node_flags);
		if (!node.node->init_error ())
		{
			auto transaction (node.node->store.tx_begin_write ());
			node.node->unchecked.clear ();
			std::cout << "Unchecked blocks deleted" << std::endl;
		}
		else
		{
			database_write_lock_error (ec);
		}
	}
	else if (vm.count ("clear_send_ids"))
	{
		std::filesystem::path data_path = vm.count ("data_path") ? std::filesystem::path (vm["data_path"].as<std::string> ()) : nano::working_path ();
		auto node_flags = nano::inactive_node_flag_defaults ();
		node_flags.set_read_only (false);
		nano::update_flags (node_flags, vm);
		nano::inactive_node node (data_path, node_flags);
		if (!node.node->init_error ())
		{
			auto transaction (node.node->wallets.tx_begin_write ());
			node.node->wallets.clear_send_ids (*transaction);
			std::cout << "Send IDs deleted" << std::endl;
		}
		else
		{
			database_write_lock_error (ec);
		}
	}
	else if (vm.count ("online_weight_clear"))
	{
		std::filesystem::path data_path = vm.count ("data_path") ? std::filesystem::path (vm["data_path"].as<std::string> ()) : nano::working_path ();
		auto node_flags = nano::inactive_node_flag_defaults ();
		node_flags.set_read_only (false);
		nano::update_flags (node_flags, vm);
		nano::inactive_node node (data_path, node_flags);
		if (!node.node->init_error ())
		{
			auto transaction (node.node->store.tx_begin_write ());
			node.node->store.online_weight ().clear (*transaction);
			std::cout << "Online weight records are removed" << std::endl;
		}
		else
		{
			database_write_lock_error (ec);
		}
	}
	else if (vm.count ("peer_clear"))
	{
		std::filesystem::path data_path = vm.count ("data_path") ? std::filesystem::path (vm["data_path"].as<std::string> ()) : nano::working_path ();
		auto node_flags = nano::inactive_node_flag_defaults ();
		node_flags.set_read_only (false);
		nano::update_flags (node_flags, vm);
		nano::inactive_node node (data_path, node_flags);
		if (!node.node->init_error ())
		{
			auto transaction (node.node->store.tx_begin_write ());
			node.node->store.peer ().clear (*transaction);
			std::cout << "Database peers are removed" << std::endl;
		}
		else
		{
			database_write_lock_error (ec);
		}
	}
	else if (vm.count ("confirmation_height_clear"))
	{
		std::filesystem::path data_path = vm.count ("data_path") ? std::filesystem::path (vm["data_path"].as<std::string> ()) : nano::working_path ();
		auto node_flags = nano::inactive_node_flag_defaults ();
		node_flags.set_read_only (false);
		nano::update_flags (node_flags, vm);
		nano::inactive_node node (data_path, node_flags);
		if (!node.node->init_error ())
		{
			if (vm.count ("account") == 1)
			{
				auto account_str = vm["account"].as<std::string> ();
				nano::account account;
				if (!account.decode_account (account_str))
				{
					nano::confirmation_height_info confirmation_height_info;
					auto error = false;
					{
						auto tx{ node.node->store.tx_begin_read () };
						error = node.node->store.confirmation_height ().get (*tx, account, confirmation_height_info);
					}
					if (!error)
					{
						auto transaction (node.node->store.tx_begin_write ());
						auto conf_height_reset_num = 0;
						if (account == node.node->network_params.ledger.genesis->account ())
						{
							conf_height_reset_num = 1;
							node.node->store.confirmation_height ().put (*transaction, account, { confirmation_height_info.height (), node.node->network_params.ledger.genesis->hash () });
						}
						else
						{
							node.node->store.confirmation_height ().clear (*transaction, account);
						}

						std::cout << "Confirmation height of account " << account_str << " is set to " << conf_height_reset_num << std::endl;
					}
					else
					{
						std::cerr << "Could not find account" << std::endl;
						ec = nano::error_cli::generic;
					}
				}
				else if (account_str == "all")
				{
					auto transaction (node.node->store.tx_begin_write ());
					reset_confirmation_heights (*transaction, node.node->network_params.ledger, node.node->store);
					std::cout << "Confirmation heights of all accounts (except genesis which is set to 1) are set to 0" << std::endl;
				}
				else
				{
					std::cerr << "Specify either valid account id or 'all'\n";
					ec = nano::error_cli::invalid_arguments;
				}
			}
			else
			{
				std::cerr << "confirmation_height_clear command requires one <account> option that may contain an account or the value 'all'\n";
				ec = nano::error_cli::invalid_arguments;
			}
		}
		else
		{
			database_write_lock_error (ec);
		}
	}
	else if (vm.count ("final_vote_clear"))
	{
		std::filesystem::path data_path = vm.count ("data_path") ? std::filesystem::path (vm["data_path"].as<std::string> ()) : nano::working_path ();
		auto node_flags = nano::inactive_node_flag_defaults ();
		node_flags.set_read_only (false);
		nano::update_flags (node_flags, vm);
		nano::inactive_node node (data_path, node_flags);
		if (!node.node->init_error ())
		{
			if (auto root_it = vm.find ("root"); root_it != vm.cend ())
			{
				auto root_str = root_it->second.as<std::string> ();
				auto transaction (node.node->store.tx_begin_write ());
				nano::root root;
				if (!root.decode_hex (root_str))
				{
					node.node->store.final_vote ().clear (*transaction, root);
					std::cout << "Successfully cleared final votes" << std::endl;
				}
				else
				{
					std::cerr << "Invalid root" << std::endl;
					ec = nano::error_cli::invalid_arguments;
				}
			}
			else if (vm.count ("all"))
			{
				auto tx{ node.node->store.tx_begin_write () };
				node.node->store.final_vote ().clear (*tx);
				std::cout << "All final votes are cleared" << std::endl;
			}
			else
			{
				std::cerr << "Either specify a single --root to clear or --all to clear all final votes (not recommended)" << std::endl;
			}
		}
		else
		{
			database_write_lock_error (ec);
		}
	}
	else if (vm.count ("generate_config"))
	{
		auto type = vm["generate_config"].as<std::string> ();
		nano::tomlconfig toml;
		bool valid_type = false;
		if (type == "node")
		{
			valid_type = true;
			nano::network_params network_params{ nano::network_constants::active_network () };
			nano::daemon_config config{ data_path, network_params };
			// set the peering port to the default value so that it is printed in the example toml file
			config.node.peering_port = network_params.network.default_node_port;
			config.serialize_toml (toml);
		}
		else if (type == "rpc")
		{
			valid_type = true;
			nano::rpc_config config{ nano::dev::network_params.network };
			config.serialize_toml (toml);
		}
		else if (type == "tls")
		{
			valid_type = true;
			nano::tls_config config;
			config.serialize_toml (toml);
		}
		else
		{
			std::cerr << "Invalid configuration type " << type << ". Must be node or rpc." << std::endl;
		}

		if (valid_type)
		{
			std::cout << "# This is an example configuration file for Nano. Visit https://docs.nano.org/running-a-node/configuration/ for more information.\n#\n"
					  << "# Fields may need to be defined in the context of a [category] above them.\n"
					  << "# The desired configuration changes should be placed in config-" << type << ".toml in the node data path.\n"
					  << "# To change a value from its default, uncomment (erasing #) the corresponding field.\n"
					  << "# It is not recommended to uncomment every field, as the default value for important fields may change in the future. Only change what you need.\n"
					  << "# Additional information for notable configuration options is available in https://docs.nano.org/running-a-node/configuration/#notable-configuration-options\n";

			if (vm.count ("use_defaults"))
			{
				std::cout << toml.to_string () << std::endl;
			}
			else
			{
				std::cout << toml.to_string_commented_entries () << std::endl;
			}
		}
	}
	else if (vm.count ("diagnostics"))
	{
		auto inactive_node = nano::default_inactive_node (data_path, vm);
		std::cout << "Testing hash function" << std::endl;
		nano::raw_key key;
		key.clear ();
		nano::send_block send (0, 0, 0, key, 0, 0);
		std::cout << "Testing key derivation function" << std::endl;
		nano::raw_key junk1;
		junk1.clear ();
		nano::uint256_union junk2 (0);
		nano::kdf kdf{ inactive_node->node->config->network_params.kdf_work };
		kdf.phs (junk1, "", junk2);
		std::cout << "Testing time retrieval latency... " << std::flush;
		nano::timer<std::chrono::nanoseconds> timer (nano::timer_state::started);
		auto const iters = 2'000'000;
		for (auto i (0); i < iters; ++i)
		{
			(void)std::chrono::steady_clock::now ();
		}
		std::cout << timer.stop ().count () / iters << " " << timer.unit () << std::endl;
		std::cout << "Dumping OpenCL information" << std::endl;
		bool error (false);
		nano::opencl_environment environment (error);
		if (!error)
		{
			environment.dump (std::cout);
			std::stringstream stream;
			environment.dump (stream);
			inactive_node->node->logger->always_log (stream.str ());
		}
		else
		{
			std::cerr << "Error initializing OpenCL" << std::endl;
			ec = nano::error_cli::generic;
		}
	}
	else if (vm.count ("key_create"))
	{
		nano::keypair pair;
		std::cout << "Private: " << pair.prv.to_string () << std::endl
				  << "Public: " << pair.pub.to_string () << std::endl
				  << "Account: " << pair.pub.to_account () << std::endl;
	}
	else if (vm.count ("key_expand"))
	{
		if (vm.count ("key") == 1)
		{
			nano::raw_key prv;
			prv.decode_hex (vm["key"].as<std::string> ());
			nano::public_key pub (nano::pub_key (prv));
			std::cout << "Private: " << prv.to_string () << std::endl
					  << "Public: " << pub.to_string () << std::endl
					  << "Account: " << pub.to_account () << std::endl;
		}
		else
		{
			std::cerr << "key_expand command requires one <key> option\n";
			ec = nano::error_cli::invalid_arguments;
		}
	}
	else if (vm.count ("wallet_add_adhoc"))
	{
		if (vm.count ("wallet") == 1 && vm.count ("key") == 1)
		{
			nano::wallet_id wallet_id;
			if (!wallet_id.decode_hex (vm["wallet"].as<std::string> ()))
			{
				std::string password;
				if (vm.count ("password") > 0)
				{
					password = vm["password"].as<std::string> ();
				}
				auto inactive_node = nano::default_inactive_node (data_path, vm);
				auto & wallets{ inactive_node->node->wallets };
				auto error = wallets.enter_password (wallet_id, password);
				if (error == nano::wallets_error::none)
				{
					nano::raw_key key;
					if (!key.decode_hex (vm["key"].as<std::string> ()))
					{
						wallets.insert_adhoc (wallet_id, key);
					}
					else
					{
						std::cerr << "Invalid key\n";
						ec = nano::error_cli::invalid_arguments;
					}
				}
				else
				{
					set_cli_wallets_error (error, ec);
				}
			}
			else
			{
				std::cerr << "Invalid wallet id\n";
				ec = nano::error_cli::invalid_arguments;
			}
		}
		else
		{
			std::cerr << "wallet_add command requires one <wallet> option and one <key> option and optionally one <password> option\n";
			ec = nano::error_cli::invalid_arguments;
		}
	}
	else if (vm.count ("wallet_change_seed"))
	{
		if (vm.count ("wallet") == 1 && (vm.count ("seed") == 1 || vm.count ("key") == 1))
		{
			nano::wallet_id wallet_id;
			if (!wallet_id.decode_hex (vm["wallet"].as<std::string> ()))
			{
				std::string password;
				if (vm.count ("password") > 0)
				{
					password = vm["password"].as<std::string> ();
				}
				auto inactive_node = nano::default_inactive_node (data_path, vm);
				auto & wallets{ inactive_node->node->wallets };
				auto error = wallets.enter_password (wallet_id, password);
				if (error == nano::wallets_error::none)
				{
					nano::raw_key seed;
					if (vm.count ("seed"))
					{
						if (seed.decode_hex (vm["seed"].as<std::string> ()))
						{
							std::cerr << "Invalid seed\n";
							ec = nano::error_cli::invalid_arguments;
						}
					}
					else if (seed.decode_hex (vm["key"].as<std::string> ()))
					{
						std::cerr << "Invalid key seed\n";
						ec = nano::error_cli::invalid_arguments;
					}
					if (!ec)
					{
						std::cout << "Changing seed and caching work. Please wait..." << std::endl;
						nano::account first_account;
						uint32_t restored_count;
						(void)inactive_node->node->wallets.change_seed (wallet_id, seed, 0, first_account, restored_count);
					}
				}
				else
				{
					set_cli_wallets_error (error, ec);
				}
			}
			else
			{
				std::cerr << "Invalid wallet id\n";
				ec = nano::error_cli::invalid_arguments;
			}
		}
		else
		{
			std::cerr << "wallet_change_seed command requires one <wallet> option and one <seed> option and optionally one <password> option\n";
			ec = nano::error_cli::invalid_arguments;
		}
	}
	else if (vm.count ("wallet_create"))
	{
		nano::raw_key seed_key;
		if (vm.count ("seed") == 1)
		{
			if (seed_key.decode_hex (vm["seed"].as<std::string> ()))
			{
				std::cerr << "Invalid seed\n";
				ec = nano::error_cli::invalid_arguments;
			}
		}
		else if (vm.count ("seed") > 1)
		{
			std::cerr << "wallet_create command allows one optional <seed> parameter\n";
			ec = nano::error_cli::invalid_arguments;
		}
		else if (vm.count ("key") == 1)
		{
			if (seed_key.decode_hex (vm["key"].as<std::string> ()))
			{
				std::cerr << "Invalid seed key\n";
				ec = nano::error_cli::invalid_arguments;
			}
		}
		else if (vm.count ("key") > 1)
		{
			std::cerr << "wallet_create command allows one optional <key> seed parameter\n";
			ec = nano::error_cli::invalid_arguments;
		}
		if (!ec)
		{
			auto inactive_node = nano::default_inactive_node (data_path, vm);
			auto wallet_key = nano::random_wallet_id ();
			inactive_node->node->wallets.create (wallet_key);
			if (vm.count ("password") > 0)
			{
				std::string password (vm["password"].as<std::string> ());
				if (inactive_node->node->wallets.rekey(wallet_key, password) != nano::wallets_error::none)
				{
					std::cerr << "Password change error\n";
					ec = nano::error_cli::invalid_arguments;
				}
			}
			if (vm.count ("seed") || vm.count ("key"))
			{
				nano::account first_account;
				uint32_t restored_count;
				(void)inactive_node->node->wallets.change_seed(wallet_key, seed_key, 0, first_account, restored_count);
			}
			std::cout << wallet_key.to_string () << std::endl;
		}
	}
	else if (vm.count ("wallet_decrypt_unsafe"))
	{
		if (vm.count ("wallet") == 1)
		{
			std::string password;
			if (vm.count ("password") == 1)
			{
				password = vm["password"].as<std::string> ();
			}
			nano::wallet_id wallet_id;
			if (!wallet_id.decode_hex (vm["wallet"].as<std::string> ()))
			{
				auto inactive_node = nano::default_inactive_node (data_path, vm);
				auto node = inactive_node->node;
				auto & wallets = node->wallets;
				auto error = wallets.enter_password (wallet_id, password);
				if (error == nano::wallets_error::none)
				{
					nano::raw_key seed;
					auto transaction (wallets.tx_begin_write ());
					wallets.get_seed (seed, *transaction, wallet_id);
					std::cout << boost::str (boost::format ("Seed: %1%\n") % seed.to_string ());
					auto keys{ wallets.decrypt (*transaction, wallet_id) };
					for (auto key : keys)
					{
						std::cout << boost::str (boost::format ("Pub: %1% Prv: %2%\n") % key.first.to_account () % key.second.to_string ());
						if (nano::pub_key (key.second) != key.first)
						{
							std::cerr << boost::str (boost::format ("Invalid private key %1%\n") % key.second.to_string ());
						}
					}
				}
				else
				{
					set_cli_wallets_error (error, ec);
				}
			}
			else
			{
				std::cerr << "Invalid wallet id\n";
				ec = nano::error_cli::invalid_arguments;
			}
		}
		else
		{
			std::cerr << "wallet_decrypt_unsafe requires one <wallet> option\n";
			ec = nano::error_cli::invalid_arguments;
		}
	}
	else if (vm.count ("wallet_destroy"))
	{
		if (vm.count ("wallet") == 1)
		{
			nano::wallet_id wallet_id;
			if (!wallet_id.decode_hex (vm["wallet"].as<std::string> ()))
			{
				auto inactive_node = nano::default_inactive_node (data_path, vm);
				auto node = inactive_node->node;
				if (node->wallets.wallet_exists (wallet_id))
				{
					node->wallets.destroy (wallet_id);
				}
				else
				{
					std::cerr << "Wallet doesn't exist\n";
					ec = nano::error_cli::invalid_arguments;
				}
			}
			else
			{
				std::cerr << "Invalid wallet id\n";
				ec = nano::error_cli::invalid_arguments;
			}
		}
		else
		{
			std::cerr << "wallet_destroy requires one <wallet> option\n";
			ec = nano::error_cli::invalid_arguments;
		}
	}
	else if (vm.count ("wallet_import"))
	{
		if (vm.count ("file") == 1)
		{
			std::string filename (vm["file"].as<std::string> ());
			std::ifstream stream;
			stream.open (filename.c_str ());
			if (!stream.fail ())
			{
				std::stringstream contents;
				contents << stream.rdbuf ();
				std::string password;
				if (vm.count ("password") == 1)
				{
					password = vm["password"].as<std::string> ();
				}
				bool forced (false);
				if (vm.count ("force") == 1)
				{
					forced = vm["force"].as<bool> ();
				}
				if (vm.count ("wallet") == 1)
				{
					nano::wallet_id wallet_id;
					if (!wallet_id.decode_hex (vm["wallet"].as<std::string> ()))
					{
						auto inactive_node = nano::default_inactive_node (data_path, vm);
						auto node = inactive_node->node;
						auto & wallets = inactive_node->node->wallets;
						if (wallets.wallet_exists (wallet_id))
						{
							bool valid = wallets.ensure_wallet_is_unlocked (wallet_id, password);
							if (valid)
							{
								if (wallets.import (wallet_id, contents.str (), password))
								{
									std::cerr << "Unable to import wallet\n";
									ec = nano::error_cli::invalid_arguments;
								}
								else
								{
									std::cout << "Import completed\n";
								}
							}
							else
							{
								std::cerr << boost::str (boost::format ("Invalid password for wallet %1%\nNew wallet should have empty (default) password or passwords for new wallet & json file should match\n") % wallet_id.to_string ());
								ec = nano::error_cli::invalid_arguments;
							}
						}
						else
						{
							if (!forced)
							{
								std::cerr << "Wallet doesn't exist\n";
								ec = nano::error_cli::invalid_arguments;
							}
							else
							{
								bool error (true);
								{
									auto lock{ node->wallets.mutex.lock () };
									auto transaction (node->wallets.tx_begin_write ());
									nano::wallet wallet (error, *transaction, node->wallets, wallet_id.to_string (), contents.str ());
								}
								if (error)
								{
									std::cerr << "Unable to import wallet\n";
									ec = nano::error_cli::invalid_arguments;
								}
								else
								{
									node->wallets.reload ();
									auto lock{ node->wallets.mutex.lock () };
									release_assert (node->wallets.wallet_exists (wallet_id));
									std::cout << "Import completed\n";
								}
							}
						}
					}
					else
					{
						std::cerr << "Invalid wallet id\n";
						ec = nano::error_cli::invalid_arguments;
					}
				}
				else
				{
					std::cerr << "wallet_import requires one <wallet> option\n";
					ec = nano::error_cli::invalid_arguments;
				}
			}
			else
			{
				std::cerr << "Unable to open <file>\n";
				ec = nano::error_cli::invalid_arguments;
			}
		}
		else
		{
			std::cerr << "wallet_import requires one <file> option\n";
			ec = nano::error_cli::invalid_arguments;
		}
	}
	else if (vm.count ("wallet_list"))
	{
		auto inactive_node = nano::default_inactive_node (data_path, vm);
		auto node = inactive_node->node;
		auto wallet_ids{ node->wallets.get_wallet_ids () };
		for (auto wallet_id : wallet_ids)
		{
			std::cout << boost::str (boost::format ("Wallet ID: %1%\n") % wallet_id.to_string ());
			auto accounts{ node->wallets.get_accounts (wallet_id) };
			for (auto account : accounts)
			{
				std::cout << account.to_account () << '\n';
			}
		}
	}
	else if (vm.count ("wallet_remove"))
	{
		if (vm.count ("wallet") == 1 && vm.count ("account") == 1)
		{
			auto inactive_node = nano::default_inactive_node (data_path, vm);
			auto node = inactive_node->node;
			nano::wallet_id wallet_id;
			if (!wallet_id.decode_hex (vm["wallet"].as<std::string> ()))
			{
				nano::account account_id;
				if (!account_id.decode_account (vm["account"].as<std::string> ()))
				{
					auto error = node->wallets.remove_account (wallet_id, account_id);
					switch (error)
					{
						case nano::wallets_error::none:
							break;
						case nano::wallets_error::account_not_found:
							std::cerr << "Account not found in wallet\n";
							ec = nano::error_cli::invalid_arguments;
							break;
						case nano::wallets_error::wallet_not_found:
							std::cerr << "Wallet not found\n";
							ec = nano::error_cli::invalid_arguments;
							break;
						default:
							std::cerr << "Unknown error in remove_account\n";
							ec = nano::error_cli::generic;
							break;
					}
				}
				else
				{
					std::cerr << "Invalid account id\n";
					ec = nano::error_cli::invalid_arguments;
				}
			}
			else
			{
				std::cerr << "Invalid wallet id\n";
				ec = nano::error_cli::invalid_arguments;
			}
		}
		else
		{
			std::cerr << "wallet_remove command requires one <wallet> and one <account> option\n";
			ec = nano::error_cli::invalid_arguments;
		}
	}
	else if (vm.count ("wallet_representative_get"))
	{
		if (vm.count ("wallet") == 1)
		{
			nano::wallet_id wallet_id;
			if (!wallet_id.decode_hex (vm["wallet"].as<std::string> ()))
			{
				auto inactive_node = nano::default_inactive_node (data_path, vm);
				auto node = inactive_node->node;
				if (node->wallets.wallet_exists (wallet_id))
				{
					auto transaction (node->wallets.tx_begin_read ());
					auto representative{ node->wallets.get_representative (*transaction, wallet_id) };
					std::cout << boost::str (boost::format ("Representative: %1%\n") % representative.to_account ());
				}
				else
				{
					std::cerr << "Wallet not found\n";
					ec = nano::error_cli::invalid_arguments;
				}
			}
			else
			{
				std::cerr << "Invalid wallet id\n";
				ec = nano::error_cli::invalid_arguments;
			}
		}
		else
		{
			std::cerr << "wallet_representative_get requires one <wallet> option\n";
			ec = nano::error_cli::invalid_arguments;
		}
	}
	else if (vm.count ("wallet_representative_set"))
	{
		if (vm.count ("wallet") == 1)
		{
			if (vm.count ("account") == 1)
			{
				nano::wallet_id wallet_id;
				if (!wallet_id.decode_hex (vm["wallet"].as<std::string> ()))
				{
					nano::account account;
					if (!account.decode_account (vm["account"].as<std::string> ()))
					{
						auto inactive_node = nano::default_inactive_node (data_path, vm);
						auto node = inactive_node->node;
						if (node->wallets.wallet_exists (wallet_id))
						{
							node->wallets.set_representative (wallet_id, account);
						}
						else
						{
							std::cerr << "Wallet not found\n";
							ec = nano::error_cli::invalid_arguments;
						}
					}
					else
					{
						std::cerr << "Invalid account\n";
						ec = nano::error_cli::invalid_arguments;
					}
				}
				else
				{
					std::cerr << "Invalid wallet id\n";
					ec = nano::error_cli::invalid_arguments;
				}
			}
			else
			{
				std::cerr << "wallet_representative_set requires one <account> option\n";
				ec = nano::error_cli::invalid_arguments;
			}
		}
		else
		{
			std::cerr << "wallet_representative_set requires one <wallet> option\n";
			ec = nano::error_cli::invalid_arguments;
		}
	}
	else
	{
		ec = nano::error_cli::unknown_command;
	}

	return ec;
}

std::unique_ptr<nano::inactive_node> nano::default_inactive_node (std::filesystem::path const & path_a, boost::program_options::variables_map const & vm_a)
{
	auto node_flags = nano::inactive_node_flag_defaults ();
	nano::update_flags (node_flags, vm_a);
	return std::make_unique<nano::inactive_node> (path_a, node_flags);
}

namespace
{
void reset_confirmation_heights (nano::store::write_transaction const & transaction, nano::ledger_constants & constants, nano::store::component & store)
{
	// First do a clean sweep
	store.confirmation_height ().clear (transaction);

	// Then make sure the confirmation height of the genesis account open block is 1
	store.confirmation_height ().put (transaction, constants.genesis->account (), { 1, constants.genesis->hash () });
}
}
