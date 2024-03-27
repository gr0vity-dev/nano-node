#include <nano/lib/interval.hpp>
#include <nano/node/messages.hpp>
#include <nano/node/node.hpp>
#include <nano/node/transport/tcp.hpp>
#include <nano/node/transport/tcp_listener.hpp>
#include <nano/node/transport/tcp_server.hpp>

#include <memory>

using namespace std::chrono_literals;

/*
 * tcp_listener
 */

nano::transport::tcp_listener::tcp_listener (uint16_t port_a, nano::node & node_a, std::size_t max_inbound_connections) :
	node{ node_a },
	stats{ node_a.stats },
	logger{ node_a.logger },
	port{ port_a },
	max_inbound_connections{ max_inbound_connections },
	acceptor{ node_a.io_ctx },
	local{ boost::asio::ip::tcp::endpoint{ boost::asio::ip::address_v6::any (), port_a } }
{
	connection_accepted.add ([this] (auto const & socket, auto const & server) {
		node.observers.socket_accepted.notify (*socket);
	});
}

nano::transport::tcp_listener::~tcp_listener ()
{
	// Thread should be stopped before destruction
	debug_assert (!cleanup_thread.joinable ());
}

void nano::transport::tcp_listener::start (std::function<bool (std::shared_ptr<nano::transport::socket> const &, boost::system::error_code const &)> callback_a)
{
	debug_assert (!cleanup_thread.joinable ());

	try
	{
		acceptor.open (local.protocol ());
		acceptor.set_option (boost::asio::ip::tcp::acceptor::reuse_address (true));
		acceptor.bind (local);
		acceptor.listen (boost::asio::socket_base::max_listen_connections);

		logger.info (nano::log::type::tcp_listener, "Listening for incoming connections on: {}", fmt::streamed (acceptor.local_endpoint ()));
	}
	catch (boost::system::system_error const & ex)
	{
		logger.critical (nano::log::type::tcp_listener, "Error while binding for incoming TCP: {} (port: {})", ex.what (), port);
		throw std::runtime_error (ex.code ().message ());
	}

	cleanup_thread = std::thread ([this] {
		nano::thread_role::set (nano::thread_role::name::tcp_listener);
		run_cleanup ();
	});

	try
	{
		logger.debug (nano::log::type::tcp_listener, "Starting acceptor thread");
		start_accept ();
		logger.debug (nano::log::type::tcp_listener, "Stopped acceptor thread");
	}
	catch (std::exception const & ex)
	{
		logger.critical (nano::log::type::tcp_listener, "Error: {}", ex.what ());
		release_assert (false); // Should be handled earlier
	}
	catch (...)
	{
		logger.critical (nano::log::type::tcp_listener, "Unknown error");
		release_assert (false); // Should be handled earlier
	}
}

void nano::transport::tcp_listener::start_accept ()
{
	if (!acceptor.is_open () || stopped)
	{
		logger.debug (nano::log::type::tcp_listener, "Acceptor is not open or listening has been stopped.");
		return;
	}

	// Need a way to limit max connections
	// wait_available_slots();  // socket.max_connections  AND socket.disconnection_of_silent_connections  FAIL

	auto socket = std::make_shared<boost::asio::ip::tcp::socket> (acceptor.get_executor ());

	acceptor.async_accept (*socket, [this, socket] (const boost::system::error_code & ec) {
		if (!ec && !stopped)
		{
			auto remote_endpoint = socket->remote_endpoint ();
			// Perform limit checks here before proceeding
			if (check_limits (remote_endpoint.address ()) != accept_result::accepted)
			{
				stats.inc (nano::stat::type::tcp_listener, nano::stat::detail::accept_failure, nano::stat::dir::in);
				socket->close ();
				start_accept (); // Continue to accept next incoming connection
				return;
			}

			stats.inc (nano::stat::type::tcp_listener, nano::stat::detail::accept_success, nano::stat::dir::in);
			logger.debug (nano::log::type::tcp_listener, "Accepted incoming connection from: {}", fmt::streamed (remote_endpoint));

			// If not over limit, proceed with establishing this connection
			auto socket_l = std::make_shared<nano::transport::socket> (std::move (*socket), remote_endpoint, socket->local_endpoint (), node, nano::transport::socket_endpoint::server);
			auto server_l = std::make_shared<nano::transport::tcp_server> (socket_l, node.shared (), true);

			{
				nano::lock_guard<nano::mutex> guard{ mutex };
				connections.insert (entry{ remote_endpoint, socket_l });
			}
			socket_l->set_timeout (node.network_params.network.idle_timeout);
			socket_l->start ();
			server_l->start ();
			connection_accepted.notify (socket_l, server_l);

			logger.debug (nano::log::type::tcp_listener, "Connection added to the container.");
		}
		else if (ec != boost::asio::error::operation_aborted)
		{
			logger.error (nano::log::type::tcp_listener, "Error on accept: {}", ec.message ());
		}
		// Always ready to accept the next connection
		start_accept ();
	});
}

void nano::transport::tcp_listener::stop ()
{
	logger.info (nano::log::type::tcp_listener, "Stopping listeninig for incoming connections and closing all sockets...");
	{
		nano::lock_guard<nano::mutex> lock{ mutex };
		stopped = true;
	}
	condition.notify_all ();
	acceptor.close ();

	if (cleanup_thread.joinable ())
	{
		cleanup_thread.join ();
	}

	decltype (connections) connections_l;
	{
		nano::lock_guard<nano::mutex> lock{ mutex };
		connections_l.swap (connections);
	}

	for (auto & connection : connections_l)
	{
		if (auto socket = connection.socket.lock ())
		{
			socket->close ();
		}
		if (auto server = connection.server.lock ())
		{
			server->stop ();
		}
		logger.debug (nano::log::type::tcp_listener, "tcp_listener::stop () for... server->stop ();");
	}
	logger.debug (nano::log::type::tcp_listener, "tcp_listener::stop () All connections closed");
}

void nano::transport::tcp_listener::run_cleanup ()
{
	nano::unique_lock<nano::mutex> lock{ mutex };
	while (!stopped)
	{
		stats.inc (nano::stat::type::tcp_listener, nano::stat::detail::cleanup);
		cleanup ();
		condition.wait_for (lock, 1s, [this] () { return stopped.load (); });
	}
}

void nano::transport::tcp_listener::cleanup ()
{
	debug_assert (!mutex.try_lock ());

	erase_if (connections, [this] (auto const & connection) {
		if (connection.socket.expired () && connection.server.expired ())
		{
			logger.debug (nano::log::type::tcp_listener, "Evicting dead connection: {}", fmt::streamed (connection.endpoint));
			return true;
		}
		else
		{
			return false;
		}
	});
}

void nano::transport::tcp_listener::wait_available_slots ()

auto should_wait
= [this] {
	  nano::lock_guard<nano::mutex> lock{ mutex };
	  return connections.size () >= max_inbound_connections;
  };
nano::interval log_interval;
while (!stopped && should_wait ())
{
	if (log_interval.elapsed (node.network_params.network.is_dev_network () ? 1s : 15s))
	{
		logger.warn (nano::log::type::tcp_listener, "Waiting for available slots to accept new connections (current: {} / max: {})",
		connection_count (), max_inbound_connections);
	}
	std::this_thread::sleep_for (100ms);
}

auto nano::transport::tcp_listener::check_limits (boost::asio::ip::address const & ip) -> accept_result
{
	nano::lock_guard<nano::mutex> lock{ mutex };

	cleanup ();

	debug_assert (connections.size () <= max_inbound_connections); // Should be checked earlier (wait_available_slots)

	if (!node.flags.disable_max_peers_per_ip)
	{
		if (count_per_ip (ip) >= node.network_params.network.max_peers_per_ip)
		{
			stats.inc (nano::stat::type::tcp_listener, nano::stat::detail::max_per_ip, nano::stat::dir::in);
			logger.debug (nano::log::type::tcp_listener, "Max connections per IP reached ({}), unable to open new connection", ip.to_string ());

			return accept_result::too_many_per_ip;
		}
	}

	// If the address is IPv4 we don't check for a network limit, since its address space isn't big as IPv6/64.
	if (!node.flags.disable_max_peers_per_subnetwork && !nano::transport::is_ipv4_or_v4_mapped_address (ip))
	{
		if (count_per_subnetwork (ip) >= node.network_params.network.max_peers_per_subnetwork)
		{
			stats.inc (nano::stat::type::tcp_listener, nano::stat::detail::max_per_subnetwork, nano::stat::dir::in);
			logger.debug (nano::log::type::tcp_listener, "Max connections per subnetwork reached ({}), unable to open new connection", ip.to_string ());

			return accept_result::too_many_per_subnetwork;
		}
	}

	if (node.network.excluded_peers.check (ip)) // true => error
	{
		stats.inc (nano::stat::type::tcp_listener, nano::stat::detail::excluded, nano::stat::dir::in);
		logger.debug (nano::log::type::tcp_listener, "Rejected connection from excluded peer: {}", ip.to_string ());

		return accept_result::excluded;
	}

	return accept_result::accepted;
}

size_t nano::transport::tcp_listener::connection_count () const
{
	nano::lock_guard<nano::mutex> lock{ mutex };
	return connections.size ();
}

size_t nano::transport::tcp_listener::realtime_count () const
{
	nano::lock_guard<nano::mutex> lock{ mutex };

	return std::count_if (connections.begin (), connections.end (), [] (auto const & connection) {
		if (auto socket = connection.socket.lock ())
		{
			return socket->is_realtime_connection ();
		}
		return false;
	});
}

size_t nano::transport::tcp_listener::bootstrap_count () const
{
	nano::lock_guard<nano::mutex> lock{ mutex };

	return std::count_if (connections.begin (), connections.end (), [] (auto const & connection) {
		if (auto socket = connection.socket.lock ())
		{
			return socket->is_bootstrap_connection ();
		}
		return false;
	});
}

size_t nano::transport::tcp_listener::count_per_ip (boost::asio::ip::address const & ip) const
{
	debug_assert (!mutex.try_lock ());

	return std::count_if (connections.begin (), connections.end (), [&ip] (auto const & connection) {
		return nano::transport::is_same_ip (connection.address (), ip);
	});
}

size_t nano::transport::tcp_listener::count_per_subnetwork (boost::asio::ip::address const & ip) const
{
	debug_assert (!mutex.try_lock ());

	return std::count_if (connections.begin (), connections.end (), [this, &ip] (auto const & connection) {
		return nano::transport::is_same_subnetwork (connection.address (), ip);
	});
}

boost::asio::ip::tcp::endpoint nano::transport::tcp_listener::endpoint () const
{
	if (!stopped)
	{
		return { boost::asio::ip::address_v6::loopback (), acceptor.local_endpoint ().port () };
	}
	else
	{
		return { boost::asio::ip::address_v6::loopback (), 0 };
	}
}

std::unique_ptr<nano::container_info_component> nano::transport::tcp_listener::collect_container_info (std::string const & name)
{
	auto composite = std::make_unique<container_info_composite> (name);
	composite->add_component (std::make_unique<container_info_leaf> (container_info{ "connections", connection_count (), sizeof (decltype (connections)::value_type) }));
	return composite;
}