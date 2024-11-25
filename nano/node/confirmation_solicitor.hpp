#pragma once

#include <nano/node/network.hpp>
#include <nano/node/repcrawler.hpp>

#include <unordered_map>
#include <thread>
#include <deque>

namespace nano
{
class election;
class node;
class node_config;

/** This class accepts elections that need further votes before they can be confirmed and bundles them in to single confirm_req packets */
class confirmation_solicitor final
{
public:
    confirmation_solicitor (nano::rep_crawler &,nano::network &, nano::node_config const &);
    ~confirmation_solicitor();
    
    /** Start the solicitor processing thread */
    void start();
    
    /** Stop the solicitor processing thread */
    void stop();
    
    /** Add an election that needs to be confirmed. Returns false if successfully added */
    bool add (nano::election const &);
    
    /** Add an election with limited scope for checking if already confirmed */
    bool request_confirmation_check(nano::election const &);

    /** Broadcast the winner of an election if the broadcast limit has not been reached. Returns false if the broadcast was performed */
    bool broadcast (nano::election const &);

    /** Prepare object for batching election confirmation requests*/
    void prepare (std::vector<nano::representative> const &);   
    
    /** Dispatch bundled requests to each channel*/
    void flush ();

private:
    enum class request_type
    {
        regular,
        check
    };

    struct pending_request
    {
        pending_request(request_type type_a, nano::election const & election_a) :
            type(type_a),
            election(election_a)
        {
        }
        
        request_type type;
        std::reference_wrapper<nano::election const> election;
    };

    /** Main processing loop */
    void run();
    
    
 
    
    /** Process a regular confirmation request */
    bool add_impl (nano::election const &);
    
    /** Process a confirmation check request */
    bool request_confirmation_check_impl(nano::election const &);
    
  

    /** Global maximum amount of block broadcasts */
    std::size_t const max_block_broadcasts;
    /** Maximum amount of requests to be sent per election, bypassed if an existing vote is for a different hash*/
    std::size_t const max_election_requests;
    /** Maximum amount of directed broadcasts to be sent per election */
    std::size_t const max_election_broadcasts;
    
    nano::network & network;
    nano::node_config const & config;
    nano::rep_crawler & rep_crawler;
    
    // Thread management
    std::deque<pending_request> pending_requests;
    nano::mutex mutex;
    nano::condition_variable condition;
    std::atomic<bool> stopped{ false };
    std::thread thread;

    // Request tracking
    unsigned rebroadcasted{ 0 };
    std::vector<nano::representative> representatives_requests;
    std::vector<nano::representative> representatives_broadcasts;
    using vector_root_hashes = std::vector<std::pair<nano::block_hash, nano::root>>;
    std::unordered_map<std::shared_ptr<nano::transport::channel>, vector_root_hashes> requests;
    bool prepared{ false };
};
}