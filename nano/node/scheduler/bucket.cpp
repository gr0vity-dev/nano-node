#include <nano/lib/blocks.hpp>
#include <nano/node/scheduler/bucket.hpp>
#include <iostream> // Ensure this include is present to use std::cout

bool nano::scheduler::bucket::value_type::operator< (value_type const & other_a) const
{
	return time < other_a.time || (time == other_a.time && block->hash () < other_a.block->hash ());
}

bool nano::scheduler::bucket::value_type::operator== (value_type const & other_a) const
{
	return time == other_a.time && block->hash () == other_a.block->hash ();
}

nano::scheduler::bucket::bucket (size_t maximum) :
	maximum{ maximum }
{
	debug_assert (maximum > 0);
}

nano::scheduler::bucket::~bucket ()
{
}

std::shared_ptr<nano::block> nano::scheduler::bucket::top () const
{
	debug_assert (!queue.empty ());
	return queue.begin ()->block;
}

void nano::scheduler::bucket::pop ()
{
	debug_assert (!queue.empty ());
	queue.erase (queue.begin ());
}

void nano::scheduler::bucket::push (uint64_t time, std::shared_ptr<nano::block> block)
{
	queue.insert ({ time, block });
	if (queue.size () > maximum)
	{
		debug_assert (!queue.empty ());
		queue.erase (--queue.end ());
	}
}

size_t nano::scheduler::bucket::size () const
{
	return queue.size ();
}

bool nano::scheduler::bucket::empty () const
{
	return queue.empty ();
}


void nano::scheduler::bucket::modify_active_election_count(bool increment) {
    nano::lock_guard<nano::mutex> lock{ mutex };

    // Output the current count and maximum before adjustment
    std::cout << "Before Adjustment - Active Election Count: " << active_election_count << ", Maximum Active: " << maximum_active << std::endl;

    if (increment) {
        debug_assert(active_election_count < maximum_active); // Ensure no overflow
        active_election_count++;
    } else {
        debug_assert(active_election_count > 0);  // Ensure no underflow
        active_election_count--;
    }

    // Output the updated count and maximum after adjustment
    std::cout << "After Adjustment - Active Election Count: " << active_election_count << ", Maximum Active: " << maximum_active << std::endl;
}



bool nano::scheduler::bucket::vacancy () const
{
    nano::lock_guard<nano::mutex> lock{ mutex };
    return active_election_count < maximum_active;
}


void nano::scheduler::bucket::dump () const
{
	for (auto const & item : queue)
	{
		std::cerr << item.time << ' ' << item.block->hash ().to_string () << '\n';
	}
}
