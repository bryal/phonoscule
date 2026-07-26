//! Reordering a play queue.
//!
//! Works on the album keys alone -- one per queue slot, equal keys meaning the same album -- and
//! returns a permutation of the slots, so a player applies it to whatever its own queue items are.

/// What a shuffle permutes: single tracks, or whole albums, each album's tracks staying together and
/// in their queue order while the albums land in random order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grouping {
    Tracks,
    Albums,
}

/// How much of the queue a shuffle reorders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// The playing track, or its whole album, moves to the front and everything else shuffles in
    /// behind it -- so nothing lands unreachably behind the cursor and playback carries on.
    Others,
    /// Everything shuffles, the playing track included.
    All,
}

/// The order a shuffle puts the queue in, as a permutation of its slots: `albums` is the album key of
/// each slot, `current` the slot playing. Empty in, empty out.
///
/// A permutation rather than the reordered queue, because a queue may hold the same track twice --
/// an album queued twice over -- and following the playing slot by identity would then be ambiguous.
/// The caller permutes its own items and finds `current`'s new position by looking for its index.
pub fn shuffle(albums: &[u64], current: usize, grouping: Grouping, scope: Scope, seed: u64) -> Vec<usize> {
    if albums.is_empty() {
        return vec![];
    }
    let mut groups: Vec<Vec<usize>> = match grouping {
        Grouping::Tracks => (0..albums.len()).map(|ix| vec![ix]).collect(),
        Grouping::Albums => {
            let mut groups: Vec<(u64, Vec<usize>)> = Vec::new();
            for (ix, &album) in albums.iter().enumerate() {
                match groups.iter_mut().find(|(key, _)| *key == album) {
                    Some((_, slots)) => slots.push(ix),
                    None => groups.push((album, vec![ix])),
                }
            }
            groups.into_iter().map(|(_, slots)| slots).collect()
        }
    };
    let mut next = random(seed);
    match scope {
        Scope::All => fisher_yates(&mut groups, &mut next),
        Scope::Others => {
            // The playing group goes to the front; only what follows is shuffled.
            let playing = groups.iter().position(|group| group.contains(&current)).unwrap_or(0);
            groups.swap(0, playing);
            fisher_yates(&mut groups[1..], &mut next);
        }
    }
    groups.into_iter().flatten().collect()
}

/// A seed for [`shuffle`], off the clock. Its own function so the shuffle itself stays pure, and so a
/// caller with a better source of entropy can pass that instead.
pub fn seed() -> u64 {
    let since_epoch = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
    since_epoch.map_or(GOLDEN, |d| d.as_nanos() as u64)
}

/// The odd 64-bit constant SplitMix64 walks its state by: 2^64 divided by the golden ratio.
const GOLDEN: u64 = 0x9E37_79B9_7F4A_7C15;

/// SplitMix64: enough of a generator to shuffle a play queue with, and short enough not to warrant a
/// dependency.
fn random(seed: u64) -> impl FnMut() -> u64 {
    let mut state = seed;
    move || {
        state = state.wrapping_add(GOLDEN);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

fn fisher_yates<T>(items: &mut [T], next: &mut impl FnMut() -> u64) {
    for i in (1..items.len()).rev() {
        // The modulo bias is immaterial at queue lengths.
        items.swap(i, (next() % (i as u64 + 1)) as usize);
    }
}

#[cfg(test)]
mod test {
    use super::*;

    /// Two albums: slots 0-2 are one, slots 3-4 the other.
    const ALBUMS: [u64; 5] = [1, 1, 1, 2, 2];

    /// Whatever the shuffle, every slot is present exactly once -- it is a permutation, and a queue
    /// that lost or duplicated a track would be a bug the caller could not detect.
    #[test]
    fn every_shuffle_is_a_permutation() {
        for grouping in [Grouping::Tracks, Grouping::Albums] {
            for scope in [Scope::All, Scope::Others] {
                for seed in 0..32 {
                    let mut order = shuffle(&ALBUMS, 1, grouping, scope, seed);
                    order.sort_unstable();
                    assert_eq!(order, [0, 1, 2, 3, 4], "{grouping:?} {scope:?} seed {seed}");
                }
            }
        }
    }

    /// Grouping by album keeps each album's tracks together and in their queue order; only the albums
    /// move.
    #[test]
    fn shuffling_albums_keeps_their_tracks_together() {
        for seed in 0..32 {
            let order = shuffle(&ALBUMS, 0, Grouping::Albums, Scope::All, seed);
            let keys: Vec<u64> = order.iter().map(|&ix| ALBUMS[ix]).collect();
            let first = keys[0];
            let split = keys.iter().position(|&k| k != first).expect("both albums are present");
            assert!(keys[split..].iter().all(|&k| k != first), "an album's tracks are contiguous: {keys:?}");
            let slots: Vec<usize> = order.iter().copied().filter(|&ix| ALBUMS[ix] == 1).collect();
            assert_eq!(slots, [0, 1, 2], "and stay in their queue order");
        }
    }

    /// Shuffling the others leaves the playing slot at the front, so playback carries on and nothing
    /// ends up behind the cursor.
    #[test]
    fn shuffling_the_others_leaves_the_playing_track_in_front() {
        for seed in 0..32 {
            let order = shuffle(&ALBUMS, 3, Grouping::Tracks, Scope::Others, seed);
            assert_eq!(order[0], 3, "the playing slot leads");
        }
        // Grouped by album, the playing album leads whole, in its own order.
        for seed in 0..32 {
            let order = shuffle(&ALBUMS, 4, Grouping::Albums, Scope::Others, seed);
            assert_eq!(&order[..2], [3, 4], "the playing album leads, its tracks in order");
        }
    }

    /// The same seed shuffles the same way, which is what makes the above worth asserting.
    #[test]
    fn a_seed_settles_the_order() {
        let once = shuffle(&ALBUMS, 0, Grouping::Tracks, Scope::All, 12345);
        let again = shuffle(&ALBUMS, 0, Grouping::Tracks, Scope::All, 12345);
        assert_eq!(once, again);
        assert_ne!(once, shuffle(&ALBUMS, 0, Grouping::Tracks, Scope::All, 54321), "a different seed differs");
    }

    #[test]
    fn an_empty_queue_shuffles_to_nothing() {
        assert!(shuffle(&[], 0, Grouping::Albums, Scope::All, 1).is_empty());
    }
}
