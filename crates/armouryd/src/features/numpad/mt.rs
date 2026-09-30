//! Multitouch protocol B → the first finger's Down / Move / Up (other fingers ignored).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Raw { Slot(i32), TrackingId(i32), X(i32), Y(i32), Syn }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Touch { Down { x: i32, y: i32 }, Move { x: i32, y: i32 }, Up }

pub struct MtDecoder {
    slot: usize,
    /// (tracking id, x, y) per slot; tracking id < 0 = empty.
    slots: [(i32, i32, i32); 10],
    /// The slot holding the first finger, and whether its Down was already sent.
    first: Option<usize>,
    down_sent: bool,
    last: (i32, i32),
}

impl Default for MtDecoder {
    /// Every slot starts empty (tracking id -1).
    fn default() -> Self { Self { slot: 0, slots: [(-1, 0, 0); 10], first: None, down_sent: false, last: (0, 0) } }
}

impl MtDecoder {
    pub fn feed(&mut self, r: Raw) -> Option<Touch> {
        match r {
            Raw::Slot(s) => { self.slot = (s.max(0) as usize).min(9); None }
            Raw::TrackingId(id) => {
                self.slots[self.slot].0 = id;
                if id >= 0 && self.first.is_none() { self.first = Some(self.slot); self.down_sent = false; }
                None
            }
            Raw::X(x) => { self.slots[self.slot].1 = x; None }
            Raw::Y(y) => { self.slots[self.slot].2 = y; None }
            Raw::Syn => {
                let f = self.first?;
                let (id, x, y) = self.slots[f];
                if id < 0 {
                    self.first = None;
                    return self.down_sent.then_some(Touch::Up);
                }
                if !self.down_sent { self.down_sent = true; self.last = (x, y); return Some(Touch::Down { x, y }); }
                if (x, y) != self.last { self.last = (x, y); return Some(Touch::Move { x, y }); }
                None
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use Raw::*;
    fn run(d: &mut MtDecoder, rs: &[Raw]) -> Vec<Touch> { rs.iter().filter_map(|r| d.feed(*r)).collect() }

    #[test]
    fn one_finger_down_move_up() {
        let mut d = MtDecoder::default();
        assert_eq!(run(&mut d, &[Slot(0), TrackingId(7), X(100), Y(200), Syn]), [Touch::Down { x: 100, y: 200 }]);
        assert_eq!(run(&mut d, &[X(150), Syn]), [Touch::Move { x: 150, y: 200 }]);
        assert_eq!(run(&mut d, &[TrackingId(-1), Syn]), [Touch::Up]);
    }

    #[test]
    fn second_finger_is_ignored() {
        let mut d = MtDecoder::default();
        run(&mut d, &[Slot(0), TrackingId(1), X(100), Y(100), Syn]);
        // second finger lands and lifts: no events for it, first finger unaffected
        assert!(run(&mut d, &[Slot(1), TrackingId(2), X(900), Y(900), Syn]).is_empty());
        assert!(run(&mut d, &[Slot(1), TrackingId(-1), Syn]).is_empty());
        assert_eq!(run(&mut d, &[Slot(0), TrackingId(-1), Syn]), [Touch::Up]);
        // after the first lifts, a new finger becomes the first
        assert_eq!(run(&mut d, &[Slot(1), TrackingId(3), X(5), Y(6), Syn]), [Touch::Down { x: 5, y: 6 }]);
    }
}
