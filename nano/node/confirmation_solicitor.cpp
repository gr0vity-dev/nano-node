#include <nano/lib/blocks.hpp>
#include <nano/node/confirmation_solicitor.hpp>
#include <nano/node/election.hpp>
#include <nano/node/nodeconfig.hpp>
#include <nano/node/repcrawler.hpp>


using namespace std::chrono_literals;

nano::confirmation_solicitor::confirmation_solicitor (nano::rep_crawler & rep_crawler_a ,nano::network & network_a, nano::node_config const & config_a) :
    max_block_broadcasts (config_a.network_params.network.is_dev_network () ? 4 : 30),
    max_election_requests (50),
    max_election_broadcasts (std::max<std::size_t> (network_a.fanout () / 2, 1)),
	rep_crawler{rep_crawler_a},
    network (network_a),
    config (config_a)
{
}

nano::confirmation_solicitor::~confirmation_solicitor()
{
    // Thread must be stopped before destruction
    debug_assert(!thread.joinable());
}

void nano::confirmation_solicitor::start()
{
    debug_assert(!thread.joinable());
    {
        nano::lock_guard<nano::mutex> lock(mutex);
        stopped = false;
    }
    thread = std::thread([this]() {
        nano::thread_role::set(nano::thread_role::name::request_loop);
        // run();
    });
}

void nano::confirmation_solicitor::stop()
{
    {
        nano::lock_guard<nano::mutex> lock(mutex);
        stopped = true;
    }
    condition.notify_all();
    nano::join_or_pass(thread);
}

bool nano::confirmation_solicitor::add(nano::election const & election_a)
{
    nano::lock_guard<nano::mutex> lock(mutex);
    if (!stopped)
    {
        pending_requests.emplace_back(request_type::regular, election_a);
        condition.notify_all();
    }
    return stopped; // Returns true if error (stopped)
}

bool nano::confirmation_solicitor::request_confirmation_check(nano::election const & election_a)
{
    nano::lock_guard<nano::mutex> lock(mutex);
    if (!stopped)
    {
        pending_requests.emplace_back(request_type::check, election_a);
        condition.notify_all();
    }
    return stopped; // Returns true if error (stopped)
}

void nano::confirmation_solicitor::run()
{
    nano::unique_lock<nano::mutex> lock(mutex);
    while (!stopped)
    {
        if (pending_requests.empty())
        {
            condition.wait(lock);
            continue;
        }

        // Get current batch of requests
        std::vector<pending_request> current_batch;
        current_batch.reserve(pending_requests.size());
        current_batch.insert(current_batch.end(), pending_requests.begin(), pending_requests.end());
        pending_requests.clear();
        
        // Process batch without holding the mutex
        lock.unlock();
        
        if (!stopped)
        {
            // Prepare representatives list
            prepare(rep_crawler.principal_representatives(std::numeric_limits<std::size_t>::max()));

            // Process each request
            for (auto const & request : current_batch)
            {
                if (stopped)
                {
                    break;
                }

                switch (request.type)
                {
                    case request_type::regular:
                        add_impl(request.election);
                        break;
                    case request_type::check:
                        request_confirmation_check_impl(request.election);
                        break;
                }
            }

            // Flush requests if not stopped
            if (!stopped)
            {
                flush();
            }
        }
        
        lock.lock();
    }
}

void nano::confirmation_solicitor::prepare (std::vector<nano::representative> const & representatives_a)
{
    debug_assert(!prepared);
    debug_assert(std::none_of(representatives_a.begin(), representatives_a.end(), 
                             [](auto const & rep) { return rep.channel == nullptr; }));

    requests.clear();
    rebroadcasted = 0;
    /** Two copies are required as representatives can be erased from \p representatives_requests */
    representatives_requests = representatives_a;
    representatives_broadcasts = representatives_a;
    prepared = true;
}

bool nano::confirmation_solicitor::broadcast (nano::election const & election_a)
{
    debug_assert(prepared);
    bool error(true);
    if (rebroadcasted++ < max_block_broadcasts)
    {
        auto const & hash(election_a.status.winner->hash());
        nano::publish winner{ config.network_params.network, election_a.status.winner };
        unsigned count = 0;
        
        // Directed broadcasting to principal representatives
        for (auto i(representatives_broadcasts.begin()), n(representatives_broadcasts.end()); 
             i != n && count < max_election_broadcasts; ++i)
        {
            auto existing(election_a.last_votes.find(i->account));
            bool const exists(existing != election_a.last_votes.end());
            bool const different(exists && existing->second.hash != hash);
            if (!exists || different)
            {
                i->channel->send(winner);
                count += different ? 0 : 1;
            }
        }
        
        // Random flood for block propagation
        network.flood_message(winner, nano::transport::buffer_drop_policy::limiter, 0.5f);
        error = false;
    }
    return error;
}

bool nano::confirmation_solicitor::add_impl(nano::election const & election_a)
{
    debug_assert(prepared);
    bool error(true);
    unsigned count = 0;
    auto const & hash(election_a.status.winner->hash());
    
    for (auto i(representatives_requests.begin()); i != representatives_requests.end() && count < max_election_requests;)
    {
        bool full_queue(false);
        auto rep(*i);
        auto existing(election_a.last_votes.find(rep.account));
        bool const exists(existing != election_a.last_votes.end());
        bool const is_final(exists && (!election_a.is_quorum.load() || 
                          existing->second.timestamp == std::numeric_limits<uint64_t>::max()));
        bool const different(exists && existing->second.hash != hash);
        
        if (!exists || !is_final || different)
        {
            auto & request_queue(requests[rep.channel]);
            if (!rep.channel->max())
            {
                request_queue.emplace_back(election_a.status.winner->hash(), 
                                         election_a.status.winner->root());
                count += different ? 0 : 1;
                error = false;
            }
            else
            {
                full_queue = true;
            }
        }
        
        i = !full_queue ? i + 1 : representatives_requests.erase(i);
    }
    return error;
}

bool nano::confirmation_solicitor::request_confirmation_check_impl(nano::election const & election_a)
{
    debug_assert(prepared);
    bool error(true);
    auto const & hash(election_a.status.winner->hash());
    
    // Only request from 2 representatives for minimal network impact
    constexpr unsigned check_requests = 2;
    
    // Create a temporary copy of the representatives list that we can shuffle
    std::vector<nano::representative> random_reps(representatives_requests);
    std::random_shuffle(random_reps.begin(), random_reps.end());
    
    unsigned count = 0;
    for (auto i(random_reps.begin()); i != random_reps.end() && count < check_requests;)
    {
        bool full_queue(false);
        auto rep(*i);
        auto existing(election_a.last_votes.find(rep.account));
        bool const exists(existing != election_a.last_votes.end());
        bool const is_final(exists && (!election_a.is_quorum.load() || 
                          existing->second.timestamp == std::numeric_limits<uint64_t>::max()));
        bool const different(exists && existing->second.hash != hash);

        if (!exists || !is_final || different)
        {
            auto & request_queue(requests[rep.channel]);
            if (!rep.channel->max())
            {
                request_queue.emplace_back(election_a.status.winner->hash(), 
                                         election_a.status.winner->root());
                count += different ? 0 : 1;
                error = false;       
            }
            else
            {
                full_queue = true;
            }
        }

        i = !full_queue ? i + 1 : random_reps.erase(i);
    }
    return error;
}

void nano::confirmation_solicitor::flush()
{
    debug_assert(prepared);
    for (auto const & request_queue : requests)
    {
        auto const & channel(request_queue.first);
        std::vector<std::pair<nano::block_hash, nano::root>> roots_hashes_l;
        
        for (auto const & root_hash : request_queue.second)
        {
            roots_hashes_l.push_back(root_hash);
            if (roots_hashes_l.size() == nano::network::confirm_req_hashes_max)
            {
                nano::confirm_req req{ config.network_params.network, roots_hashes_l };
                channel->send(req);
                roots_hashes_l.clear();
            }
        }
        
        if (!roots_hashes_l.empty())
        {
            nano::confirm_req req{ config.network_params.network, roots_hashes_l };
            channel->send(req);
        }
    }
    prepared = false;
}