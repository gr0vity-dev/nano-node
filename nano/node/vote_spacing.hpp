#pragma once

#include <nano/lib/numbers.hpp>

#include <boost/multi_index/hashed_index.hpp>
#include <boost/multi_index/member.hpp>
#include <boost/multi_index/ordered_index.hpp>
#include <boost/multi_index_container.hpp>

#include <chrono>

namespace mi = boost::multi_index;

namespace nano
{
class vote_spacing final
{
public:
	vote_spacing (std::chrono::milliseconds const & delay = std::chrono::milliseconds{ 5 * 60 * 1000 });
	bool votable (nano::root const & root_a, nano::block_hash const & hash_a) const;
	void flag (nano::root const & root_a, nano::block_hash const & hash_a);
	std::size_t size () const;
	void trim ();

private:
	class entry
	{
	public:
		nano::root root;
		std::chrono::steady_clock::time_point time;
		nano::block_hash hash;
		entry (nano::root const & root_a, std::chrono::steady_clock::time_point const & time_a, nano::block_hash const & hash_a) :
			root (root_a),
			time (time_a),
			hash (hash_a)
		{
		}
	};
	
	std::chrono::milliseconds const delay;
	// clang-format off
	class tag_root {};
	class tag_time {};
	boost::multi_index_container<entry,
	mi::indexed_by<
		mi::ordered_non_unique<mi::tag<tag_root>,
			mi::member<entry, nano::root, &entry::root>>,
		mi::ordered_non_unique<mi::tag<tag_time>,
			mi::member<entry, std::chrono::steady_clock::time_point, &entry::time>>>>
	recent;
	// clang-format on
};
}
