#include <nano/node/vote_signature_spacing.hpp>

nano::vote_signature_spacing::vote_signature_spacing (std::chrono::milliseconds const & delay_a, nano::logger & logger_a) :
	delay{ delay_a },
	logger{ logger_a }
{
}

bool nano::vote_signature_spacing::votable (nano::signature const & signature_a) const
{
	trim ();
	auto existing = recent.find (signature_a);
	bool result = existing == recent.end () || existing->second < std::chrono::steady_clock::now () - delay;

	if (existing != recent.end ())
	{
		auto time_since = std::chrono::duration_cast<std::chrono::milliseconds> (std::chrono::steady_clock::now () - existing->second).count ();
		logger.debug (nano::log::type::vote_rebroadcaster, "Signature exists - Time since last: {}ms, Required delay: {}ms", time_since, delay.count ());
	}

	logger.debug (nano::log::type::vote_rebroadcaster, "Vote spacing check - Votable: {}", result ? "true" : "false");

	return result;
}

void nano::vote_signature_spacing::flag (nano::signature const & signature_a)
{
	trim ();
	auto now = std::chrono::steady_clock::now ();
	auto [it, inserted] = recent.insert_or_assign (signature_a, now);

	logger.debug (nano::log::type::vote_rebroadcaster, "Vote spacing flag - Operation: {}, Size: {}", inserted ? "inserted" : "updated", recent.size ());
}

std::size_t nano::vote_signature_spacing::size () const
{
	return recent.size ();
}

void nano::vote_signature_spacing::trim () const
{
	auto now = std::chrono::steady_clock::now ();
	auto it = recent.begin ();
	std::size_t erased = 0;

	while (it != recent.end ())
	{
		if (it->second < now - delay)
		{
			it = recent.erase (it);
			erased++;
		}
		else
		{
			++it;
		}
	}

	if (erased > 0)
	{
		logger.debug (nano::log::type::vote_rebroadcaster, "Vote spacing trim - Removed: {}, New size: {}", erased, recent.size ());
	}
} 