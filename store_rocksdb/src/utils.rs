use store_traits::ledger::RangeBounds;

pub(crate) fn value_in_range<T>(value: &T, range: &RangeBounds<T>) -> bool
where
    T: Ord,
{
    use std::ops::Bound;
    let start_ok = match &range.start {
        Bound::Included(start) => value >= start,
        Bound::Excluded(start) => value > start,
        Bound::Unbounded => true,
    };
    let end_ok = match &range.end {
        Bound::Included(end) => value <= end,
        Bound::Excluded(end) => value < end,
        Bound::Unbounded => true,
    };
    start_ok && end_ok
}
