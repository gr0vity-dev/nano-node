#pragma once

#include <nano/lib/numbers.hpp>
#include <nano/lib/logging.hpp>

#include <chrono>
#include <map>

namespace nano
{
class vote_signature_spacing final
{
public:
	vote_signature_spacing (std::chrono::milliseconds const & delay_a, nano::logger & logger_a);
	bool votable (nano::signature const & signature_a) const;
	void flag (nano::signature const & signature_a);
	std::size_t size () const;

private:
	void trim () const;
	std::chrono::milliseconds delay;
	mutable std::map<nano::signature, std::chrono::steady_clock::time_point> recent;
	nano::logger & logger;
};
} 