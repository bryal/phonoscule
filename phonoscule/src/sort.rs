//! Album ordering: a small, serializable [`SortOrder`] yielding a total [`SortOrder::cmp`] over
//! [`Album`]s, for a browser to arrange them by and to persist as a user's choice.

use crate::library::Album;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;

/// A sort direction, shown in the UI as ascending ↑ / descending ↓.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Dir {
    Asc,
    Desc,
}

impl Dir {
    /// Orients an ascending comparison to this direction.
    fn apply(self, ord: Ordering) -> Ordering {
        match self {
            Dir::Asc => ord,
            Dir::Desc => ord.reverse(),
        }
    }

    fn arrow(self) -> &'static str {
        match self {
            Dir::Asc => "↑",
            Dir::Desc => "↓",
        }
    }
}

/// The album attribute a sort orders by within a group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SortField {
    Name,
    Year,
}

/// An optional artist grouping (the groups ordered by artist name in the given direction), then the
/// field within each group. [`SortOrder::ALL`] is every combination, for a picker to offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SortOrder {
    /// `Some(dir)` groups albums by artist and orders the groups by artist name in `dir`; `None`
    /// sorts the whole library by the field alone.
    pub group_by_artist: Option<Dir>,
    pub field: SortField,
    pub field_dir: Dir,
}

impl Default for SortOrder {
    fn default() -> Self {
        // "Name ↑": the whole library by album name, ascending (A–Z).
        SortOrder { group_by_artist: None, field: SortField::Name, field_dir: Dir::Asc }
    }
}

impl SortOrder {
    /// Every order, in a sensible display order: the ungrouped fields first, then the
    /// artist-grouped ones; ascending above descending throughout (the artist grouping's direction
    /// leads, then the field's), so the default "Name ↑" heads the list.
    pub const ALL: [SortOrder; 12] = {
        use Dir::{Asc, Desc};
        use SortField::{Name, Year};
        [
            SortOrder { group_by_artist: None, field: Name, field_dir: Asc },
            SortOrder { group_by_artist: None, field: Name, field_dir: Desc },
            SortOrder { group_by_artist: None, field: Year, field_dir: Asc },
            SortOrder { group_by_artist: None, field: Year, field_dir: Desc },
            SortOrder { group_by_artist: Some(Asc), field: Name, field_dir: Asc },
            SortOrder { group_by_artist: Some(Asc), field: Name, field_dir: Desc },
            SortOrder { group_by_artist: Some(Desc), field: Name, field_dir: Asc },
            SortOrder { group_by_artist: Some(Desc), field: Name, field_dir: Desc },
            SortOrder { group_by_artist: Some(Asc), field: Year, field_dir: Asc },
            SortOrder { group_by_artist: Some(Asc), field: Year, field_dir: Desc },
            SortOrder { group_by_artist: Some(Desc), field: Year, field_dir: Asc },
            SortOrder { group_by_artist: Some(Desc), field: Year, field_dir: Desc },
        ]
    };

    /// The chip/menu label, e.g. "Name ↓" or "Artist ↓, Year ↑".
    pub fn label(self) -> String {
        let field = match self.field {
            SortField::Name => "Name",
            SortField::Year => "Year",
        };
        match self.group_by_artist {
            Some(dir) => format!("Artist {}, {} {}", dir.arrow(), field, self.field_dir.arrow()),
            None => format!("{} {}", field, self.field_dir.arrow()),
        }
    }

    /// Orders two albums by this sort. Artist grouping (when enabled) is the primary key; then the
    /// field; then a fixed name/id tiebreak, so equal keys keep a stable, repeatable order. A
    /// missing year sorts as the smallest value (last under a descending year sort).
    pub fn cmp(self, a: &Album, b: &Album) -> Ordering {
        let group = match self.group_by_artist {
            Some(dir) => dir.apply(a.artist.to_lowercase().cmp(&b.artist.to_lowercase())),
            None => Ordering::Equal,
        };
        let field = match self.field {
            SortField::Name => a.title.to_lowercase().cmp(&b.title.to_lowercase()),
            SortField::Year => a.year.cmp(&b.year),
        };
        group
            .then_with(|| self.field_dir.apply(field))
            .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
            .then_with(|| a.id.cmp(&b.id))
    }
}

#[cfg(test)]
mod test {
    use super::*;

    fn album(artist: &str, title: &str, year: Option<u32>) -> Album {
        Album {
            // Irrelevant to these assertions (the id only breaks ties between identical titles).
            id: 0,
            title: title.into(),
            artist: artist.into(),
            genre: String::new(),
            year,
            cover_id: None,
            cover: None,
            accent: None,
            tracks: vec![],
        }
    }

    /// Sorting a small library by each order gives the expected album sequence.
    #[test]
    fn sort_orders_albums() {
        use Dir::{Asc, Desc};
        use SortField::{Name, Year};
        let sorted = |sort: SortOrder, albums: &[Album]| -> Vec<String> {
            let mut ixs: Vec<usize> = (0..albums.len()).collect();
            ixs.sort_by(|&a, &b| sort.cmp(&albums[a], &albums[b]));
            ixs.into_iter().map(|i| albums[i].title.clone()).collect()
        };
        // Two artists; "Zoo" has an undated album to exercise the missing-year end.
        let albums = [
            album("Az", "Beta", Some(2001)),
            album("Az", "Alpha", Some(2010)),
            album("Zoo", "Gamma", Some(2005)),
            album("Zoo", "Delta", None),
        ];

        let name_desc = SortOrder { group_by_artist: None, field: Name, field_dir: Desc };
        assert_eq!(sorted(name_desc, &albums), ["Gamma", "Delta", "Beta", "Alpha"]);

        let name_asc = SortOrder { group_by_artist: None, field: Name, field_dir: Asc };
        assert_eq!(sorted(name_asc, &albums), ["Alpha", "Beta", "Delta", "Gamma"]);
        assert_eq!(name_asc, SortOrder::default(), "Name ↑ (ascending) is the default");

        let year_desc = SortOrder { group_by_artist: None, field: Year, field_dir: Desc };
        assert_eq!(sorted(year_desc, &albums), ["Alpha", "Gamma", "Beta", "Delta"], "the undated album sorts last");

        let year_asc = SortOrder { group_by_artist: None, field: Year, field_dir: Asc };
        assert_eq!(sorted(year_asc, &albums), ["Delta", "Beta", "Gamma", "Alpha"], "and first, ascending");

        // Group by artist descending (Zoo before Az), albums by name ascending within each group.
        let grouped = SortOrder { group_by_artist: Some(Desc), field: Name, field_dir: Asc };
        assert_eq!(sorted(grouped, &albums), ["Delta", "Gamma", "Alpha", "Beta"]);
    }

    /// The offered orders are the twelve distinct combinations, with the expected labels.
    #[test]
    fn sort_options_are_labelled() {
        assert_eq!(SortOrder::ALL.len(), 12);
        let labels: Vec<String> = SortOrder::ALL.iter().map(|s| s.label()).collect();
        let unique: std::collections::HashSet<&String> = labels.iter().collect();
        assert_eq!(unique.len(), 12, "every option is distinct");
        assert_eq!(SortOrder::ALL[0].label(), "Name ↑", "the default heads the list");
        assert!(labels.contains(&"Artist ↓, Name ↓".to_string()));
        assert!(labels.contains(&"Artist ↑, Year ↑".to_string()));
    }
}
