// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Losses a suite asks an engine for on its desktops' streams
//! ([`vnilink::probes::Drops`]): at each [`DropPoint`], a seeded share of
//! what passes, or exactly the nth. One generator per point, so what one
//! point loses never depends on how much passed another, and a seed loses
//! the same things on every run that sends the same things.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub use vnilink::probes::DropPoint;

use crate::{lock, Engine};

/// What a [`SeededDrops`] loses at one point.
enum Plan {
    /// About `percent` of what passes, chosen by a deterministic xorshift64*.
    Share { state: u64, percent: u64 },
    /// Exactly these, counting from 1 in the order they pass.
    Nth(Vec<usize>),
}

/// At each point, what passed and what was lost.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tally {
    pub seen: usize,
    pub lost: usize,
}

/// A plan of losses, built point by point and installed on an engine with
/// [`SeededDrops::install`]. A point it does not name loses nothing.
#[derive(Default)]
pub struct SeededDrops {
    plans: Mutex<HashMap<DropPoint, Plan>>,
    tallies: Mutex<HashMap<DropPoint, Tally>>,
}

impl SeededDrops {
    pub fn new() -> Self {
        Self::default()
    }

    /// Loses about `percent` of what passes `point`, chosen by `seed`.
    pub fn share(self, point: DropPoint, seed: u64, percent: u64) -> Self {
        lock(&self.plans).insert(
            point,
            Plan::Share {
                state: seed.max(1),
                percent: percent.min(100),
            },
        );
        self
    }

    /// Loses exactly the `nths` things that pass `point`, the first being 1.
    pub fn nth(self, point: DropPoint, nths: &[usize]) -> Self {
        lock(&self.plans).insert(point, Plan::Nth(nths.to_vec()));
        self
    }

    /// Puts the plan on `engine`, which loses from now on as it says; the
    /// plan stays readable for its tallies.
    pub fn install(self, engine: &Engine) -> Arc<Self> {
        let plan = Arc::new(self);
        engine
            .core
            .set_drops(Some(Arc::clone(&plan) as Arc<dyn vnilink::probes::Drops>));
        plan
    }

    /// What passed `point` and what of it was lost, so far.
    pub fn tally(&self, point: DropPoint) -> Tally {
        lock(&self.tallies).get(&point).copied().unwrap_or_default()
    }
}

impl vnilink::probes::Drops for SeededDrops {
    fn drops(&self, point: DropPoint) -> bool {
        let mut tallies = lock(&self.tallies);
        let tally = tallies.entry(point).or_default();
        tally.seen += 1;
        let lose = match lock(&self.plans).get_mut(&point) {
            None => false,
            Some(Plan::Nth(nths)) => nths.contains(&tally.seen),
            Some(Plan::Share { state, percent }) => {
                *state ^= *state >> 12;
                *state ^= *state << 25;
                *state ^= *state >> 27;
                state.wrapping_mul(0x2545_F491_4F6C_DD1D) % 100 < *percent
            }
        };
        if lose {
            tally.lost += 1;
        }
        lose
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vnilink::probes::Drops;

    #[test]
    fn a_seed_loses_the_same_things_and_points_do_not_share_a_generator() {
        let run = |interleave: bool| {
            let drops =
                SeededDrops::new()
                    .share(DropPoint::Datagram, 7, 30)
                    .share(DropPoint::Inbox, 7, 30);
            let mut lost = Vec::new();
            for _ in 0..200 {
                if interleave {
                    drops.drops(DropPoint::Inbox);
                }
                lost.push(drops.drops(DropPoint::Datagram));
            }
            (lost, drops.tally(DropPoint::Datagram))
        };
        let (alone, tally) = run(false);
        assert_eq!(
            run(true).0,
            alone,
            "what one point loses does not depend on another point's traffic"
        );
        assert!(
            (40..=80).contains(&tally.lost) && tally.seen == 200,
            "about 30% of 200 is lost: {tally:?}"
        );
    }

    #[test]
    fn nth_loses_exactly_those_and_unnamed_points_lose_nothing() {
        let drops = SeededDrops::new().nth(DropPoint::Outbox, &[2, 4]);
        let lost: Vec<bool> = (0..5).map(|_| drops.drops(DropPoint::Outbox)).collect();
        assert_eq!(
            lost,
            [false, true, false, true, false],
            "the second and fourth are lost"
        );
        assert!(
            !(0..10).any(|_| drops.drops(DropPoint::Gate)),
            "a point the plan does not name loses nothing"
        );
        assert_eq!(
            drops.tally(DropPoint::Gate),
            Tally { seen: 10, lost: 0 },
            "what passed an unnamed point is still counted"
        );
    }
}
