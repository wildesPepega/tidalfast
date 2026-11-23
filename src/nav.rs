//! Which track comes after / before the current one: one place for the rules,
//! so skipping, auto-advance and preloading can never disagree.

use crate::config::RepeatMode;

/// Where playback stands in the queue.
pub struct Nav<'a> {
    pub len: usize,
    /// Queue index of the current track.
    pub index: usize,
    /// The play order and the position in it, when shuffle is on (and the order matches the queue).
    pub shuffle: Option<(&'a [usize], usize)>,
    pub repeat: RepeatMode,
}

/// A track to move to: its queue index and, in shuffle mode, its position in the play order.
pub type Step = (usize, Option<usize>);

impl Nav<'_> {
    /// The track after the current one, wrapping round only with "repeat all".
    /// (Repeating a single track is the caller's business.)
    pub fn next(&self) -> Option<Step> {
        if self.len == 0 {
            return None;
        }
        match self.shuffle {
            Some((order, pos)) => {
                if pos + 1 < order.len() {
                    Some((order[pos + 1], Some(pos + 1)))
                } else if self.repeat == RepeatMode::All && !order.is_empty() {
                    Some((order[0], Some(0)))
                } else {
                    None
                }
            }
            None => {
                if self.index + 1 < self.len {
                    Some((self.index + 1, None))
                } else if self.repeat == RepeatMode::All {
                    Some((0, None))
                } else {
                    None
                }
            }
        }
    }

    /// The track before the current one; `None` at the very beginning (unless "repeat all" wraps).
    pub fn previous(&self) -> Option<Step> {
        if self.len == 0 {
            return None;
        }
        match self.shuffle {
            Some((order, pos)) => {
                if pos > 0 {
                    Some((order[pos - 1], Some(pos - 1)))
                } else if self.repeat == RepeatMode::All && !order.is_empty() {
                    let last = order.len() - 1;
                    Some((order[last], Some(last)))
                } else {
                    None
                }
            }
            None => {
                if self.index > 0 {
                    Some((self.index - 1, None))
                } else if self.repeat == RepeatMode::All {
                    Some((self.len - 1, None))
                } else {
                    None
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(len: usize, index: usize, repeat: RepeatMode) -> Option<Step> {
        Nav { len, index, shuffle: None, repeat }.next()
    }

    #[test]
    fn walks_forward_and_stops_or_wraps_at_the_end() {
        assert_eq!(plain(3, 0, RepeatMode::Off), Some((1, None)));
        assert_eq!(plain(3, 2, RepeatMode::Off), None);
        assert_eq!(plain(3, 2, RepeatMode::All), Some((0, None)));
        assert_eq!(plain(0, 0, RepeatMode::All), None);
        assert_eq!(plain(1, 0, RepeatMode::Off), None);
    }

    #[test]
    fn walks_backwards_and_stops_or_wraps_at_the_start() {
        let prev = |index, repeat| Nav { len: 3, index, shuffle: None, repeat }.previous();
        assert_eq!(prev(2, RepeatMode::Off), Some((1, None)));
        assert_eq!(prev(0, RepeatMode::Off), None);
        assert_eq!(prev(0, RepeatMode::All), Some((2, None)));
    }

    #[test]
    fn follows_the_shuffle_order() {
        let order = [2, 0, 3, 1];
        let nav = |pos, repeat| Nav { len: 4, index: order[pos], shuffle: Some((&order, pos)), repeat };
        assert_eq!(nav(0, RepeatMode::Off).next(), Some((0, Some(1))));
        assert_eq!(nav(2, RepeatMode::Off).next(), Some((1, Some(3))));
        assert_eq!(nav(3, RepeatMode::Off).next(), None);
        assert_eq!(nav(3, RepeatMode::All).next(), Some((2, Some(0))));
        assert_eq!(nav(1, RepeatMode::Off).previous(), Some((2, Some(0))));
        assert_eq!(nav(0, RepeatMode::Off).previous(), None);
        assert_eq!(nav(0, RepeatMode::All).previous(), Some((1, Some(3))));
    }
}
