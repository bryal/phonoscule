//! A bounded least-recently-used cache, keyed by cover id.
//!
//! What the covers are kept in (see the covers module), and the reason this player's memory does not
//! grow with the size of the library. Beyond the bound there is a *pinned* set the caller names on
//! every insert: the covers it is about to need, which are never the ones evicted however long ago
//! they were last looked at.

use std::collections::{HashMap, HashSet};

/// A cache of at most `capacity` entries. Full, it evicts the entry looked at longest ago that the
/// caller has not pinned.
pub struct Lru<T> {
    /// `id -> (value, the tick it was last used at)`. A counter, not a clock: this is driven from
    /// pure state transitions with no time to read.
    entries: HashMap<u64, (T, u64)>,
    /// Ids whose load is in flight, so asking twice does not load twice.
    pending: HashSet<u64>,
    capacity: usize,
    tick: u64,
}

impl<T> Lru<T> {
    pub fn new(capacity: usize) -> Self {
        Lru { entries: HashMap::new(), pending: HashSet::new(), capacity: capacity.max(1), tick: 0 }
    }

    /// The entry for `id`, marking it as just used. `None` if it isn't held.
    pub fn get(&mut self, id: u64) -> Option<&mut T> {
        self.tick += 1;
        let tick = self.tick;
        let (value, used) = self.entries.get_mut(&id)?;
        *used = tick;
        Some(value)
    }

    /// Marks `id` as being loaded, reporting whether that is news -- so a caller can ask on every
    /// frame and start the load only once. Cleared by [`insert`](Self::insert) or [`give_up`].
    ///
    /// [`give_up`]: Self::give_up
    pub fn start_loading(&mut self, id: u64) -> bool {
        !self.entries.contains_key(&id) && self.pending.insert(id)
    }

    /// Takes in a loaded entry, evicting the least recently used unpinned one if that puts the cache
    /// over its bound.
    pub fn insert(&mut self, id: u64, value: T, pinned: &HashSet<u64>) {
        self.pending.remove(&id);
        self.tick += 1;
        self.entries.insert(id, (value, self.tick));
        while self.entries.len() > self.capacity {
            // The bound is small and inserts are infrequent, so scanning for the oldest beats
            // maintaining an ordered index beside the map.
            let oldest = self
                .entries
                .iter()
                .filter(|(id, _)| !pinned.contains(*id))
                .min_by_key(|(_, (_, used))| *used)
                .map(|(id, _)| *id);
            // Everything left is pinned: the caller pinned more than fits, and keeping what it asked
            // for beats honouring a bound it has overrun.
            let Some(oldest) = oldest else { break };
            self.entries.remove(&oldest);
        }
    }

    /// Drops everything held. Loads in flight are left to land and be judged on arrival.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Drops an entry that is no longer any use, so it can be loaded afresh.
    pub fn forget(&mut self, id: u64) {
        self.entries.remove(&id);
    }

    /// Abandons a load that failed, so a later attempt is not deduplicated away forever.
    pub fn give_up(&mut self, id: u64) {
        self.pending.remove(&id);
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(test)]
mod test {
    use super::*;

    fn pinned(ids: &[u64]) -> HashSet<u64> {
        ids.iter().copied().collect()
    }

    /// A full cache evicts what was looked at longest ago -- and a `get` makes an entry the freshest,
    /// so what is being looked at survives while colder entries go.
    #[test]
    fn evicts_the_least_recently_used() {
        let mut cache = Lru::new(4);
        for id in 0..4 {
            cache.insert(id, id, &pinned(&[]));
        }
        assert!(cache.get(0).is_some(), "refresh the oldest");
        cache.insert(4, 4, &pinned(&[]));

        assert_eq!(cache.len(), 4);
        assert!(cache.get(0).is_some(), "the refreshed entry survives");
        assert!(cache.get(1).is_none(), "the now-oldest entry is evicted");
        assert!(cache.get(4).is_some(), "the newcomer is held");
    }

    /// A pinned entry is not evicted however long ago it was looked at: it is what the caller is
    /// about to need.
    #[test]
    fn pinned_entries_survive_eviction() {
        let mut cache = Lru::new(3);
        cache.insert(0, 0, &pinned(&[]));
        cache.insert(1, 1, &pinned(&[]));
        cache.insert(2, 2, &pinned(&[]));

        // 0 is the oldest, but pinned, so 1 goes instead.
        cache.insert(3, 3, &pinned(&[0]));
        assert!(cache.get(0).is_some(), "the pinned entry survives");
        assert!(cache.get(1).is_none(), "the oldest unpinned entry went");
        assert_eq!(cache.len(), 3);
    }

    /// Pinning more than fits keeps what was asked for rather than the bound.
    #[test]
    fn pinning_more_than_fits_overruns_the_bound() {
        let mut cache = Lru::new(2);
        let keep = pinned(&[0, 1, 2]);
        for id in 0..3 {
            cache.insert(id, id, &keep);
        }
        assert_eq!(cache.len(), 3, "nothing could be evicted");
    }

    /// A load is started once however often it is asked for, and a failed one can be retried.
    #[test]
    fn loads_are_started_once() {
        let mut cache = Lru::new(4);
        assert!(cache.start_loading(7), "the first ask starts a load");
        assert!(!cache.start_loading(7), "a load already in flight is not started again");

        cache.give_up(7);
        assert!(cache.start_loading(7), "a failed load can be retried");

        cache.insert(7, 7, &pinned(&[]));
        assert!(!cache.start_loading(7), "an entry already held needs no load");
    }
}
