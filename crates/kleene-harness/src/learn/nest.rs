//! The `nest` generator: consolidation over a reporting tree whose depth is
//! the dial. Every other generator grows wider as the dial rises; this one
//! grows *deeper*, so a pack built from it at several dials shows how the
//! harness behaves as the recursion a task calls for passes the default
//! depth of two.
//!
//! The context is one report per unit of a group, in no particular order.
//! A report names the unit, the unit it reports to, its direct spend and its
//! sub-units; some say a sub-unit was divested and is excluded with
//! everything under it, and `Correction:` rows restate a unit's spend. The
//! consolidated spend of a unit is its direct spend plus the consolidated
//! spend of its included sub-units, which is a recursion whose depth is the
//! tree's: a session that delegates one sub-unit to one child session
//! reaches depth [`levels`]`(dial)`. Nothing forces that shape (a flat
//! recursive CTE answers too), so a run records the depth the model chose
//! next to whether it was right.

use super::generators::{difficulty_prior, GeneratedTask, Rng};
use super::verify::Verify;

/// How many levels of sub-units lie below the top unit at a dial: 1 at 0,
/// 6 at 1, so dials 0.4, 0.6, 0.8 and 1.0 give depths 3, 4, 5 and 6. The
/// natural delegating solution reaches session depth `levels`.
pub fn levels(dial: f64) -> u32 {
    1 + (dial.clamp(0.0, 1.0) * 5.0).round() as u32
}

/// One unit of the generated group.
#[derive(Debug, Clone)]
struct Unit {
    name: String,
    level: u32,
    parent: Option<usize>,
    children: Vec<usize>,
    /// Direct spend as first reported.
    reported: i64,
    /// Direct spend after a correction, if the log carries one.
    corrected: Option<i64>,
    /// The child this unit's report excludes, if any.
    excluded: Option<usize>,
}

impl Unit {
    fn spend(&self) -> i64 {
        self.corrected.unwrap_or(self.reported)
    }
}

/// The group: units in creation order, root first.
struct Group {
    units: Vec<Unit>,
}

impl Group {
    /// Children that count: every child but the excluded one.
    fn included(&self, i: usize) -> impl Iterator<Item = usize> + '_ {
        let u = &self.units[i];
        u.children
            .iter()
            .copied()
            .filter(move |c| Some(*c) != u.excluded)
    }

    fn consolidated(&self, i: usize) -> i64 {
        self.units[i].spend() + self.included(i).map(|c| self.consolidated(c)).sum::<i64>()
    }

    fn count(&self, i: usize) -> i64 {
        1 + self.included(i).map(|c| self.count(c)).sum::<i64>()
    }

    fn depth(&self, i: usize) -> u32 {
        self.included(i)
            .map(|c| 1 + self.depth(c))
            .max()
            .unwrap_or(0)
    }
}

const WORDS: &[&str] = &[
    "Alder", "Birch", "Cedar", "Dogwood", "Elm", "Fir", "Ginkgo", "Hazel", "Juniper", "Larch",
    "Maple", "Oak", "Pine", "Rowan", "Spruce", "Willow", "Aspen", "Beech", "Cypress", "Hornbeam",
    "Linden", "Magnolia", "Poplar", "Sequoia", "Sycamore", "Yew", "Acacia", "Baobab", "Chestnut",
    "Ebony", "Hawthorn", "Laurel", "Mulberry", "Olive", "Walnut", "Tamarack",
];

fn build(dial: f64, rng: &mut Rng) -> Group {
    let k = levels(dial);
    let mut units: Vec<Unit> = vec![];
    let mut names = std::collections::HashSet::new();
    let mut fresh_name = |rng: &mut Rng| loop {
        let w = WORDS[rng.below(WORDS.len() as u64) as usize];
        let n = 10 + rng.below(90);
        let name = format!("{w}-{n}");
        if names.insert(name.clone()) {
            return name;
        }
    };
    let new_unit = |rng: &mut Rng, name: String, level: u32, parent: Option<usize>| Unit {
        name,
        level,
        parent,
        children: vec![],
        reported: 40 + rng.below(951) as i64,
        corrected: None,
        excluded: None,
    };
    let root_name = fresh_name(rng);
    units.push(new_unit(rng, root_name, 0, None));
    // Breadth-first: a unit below the deepest level gets two or three
    // sub-units; from level 2 down a branch other than the first may stop
    // early, so the tree is ragged but its first path always reaches `k`.
    let mut frontier = std::collections::VecDeque::from([0usize]);
    while let Some(i) = frontier.pop_front() {
        let level = units[i].level;
        if level >= k {
            continue;
        }
        if level >= 2 && !is_spine(&units, i) && rng.unit() < 0.2 {
            continue;
        }
        let n = if rng.unit() < 0.3 { 3 } else { 2 };
        for _ in 0..n {
            let name = fresh_name(rng);
            let c = units.len();
            units.push(new_unit(rng, name, level + 1, Some(i)));
            units[i].children.push(c);
            frontier.push_back(c);
        }
        // One divested sub-unit, never the first (the spine stays whole).
        if units[i].children.len() >= 2 && rng.unit() < 0.25 {
            let j = 1 + rng.below(units[i].children.len() as u64 - 1) as usize;
            units[i].excluded = Some(units[i].children[j]);
        }
    }
    // Corrections: one per corrected unit.
    for u in units.iter_mut() {
        if rng.unit() < 0.2 {
            let mut v = 40 + rng.below(951) as i64;
            if v == u.reported {
                v += 1;
            }
            u.corrected = Some(v);
        }
    }
    Group { units }
}

/// Whether `i` is on the first-child path from the root.
fn is_spine(units: &[Unit], mut i: usize) -> bool {
    while let Some(p) = units[i].parent {
        if units[p].children.first() != Some(&i) {
            return false;
        }
        i = p;
    }
    true
}

const DISTRACTORS: &[&str] = &[
    "Headcount at year end: {n}.",
    "Budget request for next year: {n} kEUR, not yet approved.",
    "The unit moved offices in March.",
    "Open audit findings: {n}.",
    "Travel was frozen for the fourth quarter.",
    "Capital commitments outstanding: {n} kEUR, outside this year's spend.",
];

const PREAMBLE: &str = "ctx holds the annual reports of a group, one unit per row, in no particular order. Each report names the unit, the unit it reports to, its direct spend this year in kEUR and its sub-units, and may say that one sub-unit was divested and is excluded from consolidation, which removes that sub-unit and everything under it. A row beginning 'Correction:' replaces the direct spend of the unit it names. Reports also mention figures that are not spend (headcount, budget requests, commitments, findings). The consolidated spend of a unit is its own direct spend plus the consolidated spend of every sub-unit that is not excluded, all the way down.";

/// Generate a task at `dial` from `rng`.
pub fn nest(dial: f64, rng: &mut Rng) -> GeneratedTask {
    let g = build(dial, rng);
    let root = &g.units[0];
    let mut rows: Vec<String> = vec![];
    for u in &g.units {
        let to = match u.parent {
            None => "the group board".to_string(),
            Some(p) => format!("unit {}", g.units[p].name),
        };
        let distractor = DISTRACTORS[rng.below(DISTRACTORS.len() as u64) as usize]
            .replace("{n}", &(5 + rng.below(400)).to_string());
        let subs = if u.children.is_empty() {
            "No sub-units.".to_string()
        } else {
            format!(
                "Sub-units: {}.",
                u.children
                    .iter()
                    .map(|c| g.units[*c].name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        let excluded = match u.excluded {
            Some(e) => format!(
                " Sub-unit {} was divested during the year and is excluded from consolidation, together with everything under it.",
                g.units[e].name
            ),
            None => String::new(),
        };
        rows.push(format!(
            "Unit {} reports to {to}. Direct spend this year: {} kEUR. {distractor} {subs}{excluded}",
            u.name, u.reported
        ));
        if let Some(c) = u.corrected {
            rows.push(format!(
                "Correction: unit {}'s direct spend this year should read {c} kEUR, not {}.",
                u.name, u.reported
            ));
        }
    }
    // Shuffle, so the hierarchy has to be followed by name.
    for i in (1..rows.len()).rev() {
        let j = rng.below(i as u64 + 1) as usize;
        rows.swap(i, j);
    }
    let kids: Vec<usize> = g.included(0).collect();
    let question = rng.below(5);
    let (kind, ask, verify): (&str, String, Verify) = match question {
        0 => (
            "nest_total",
            format!(
                "What is the consolidated spend of unit {}, the top of the group? Answer with FINAL over one row with column total (an integer, kEUR).",
                root.name
            ),
            Verify::Number {
                value: g.consolidated(0) as f64,
                tolerance: 0.5,
            },
        ),
        1 => {
            // A unit one level down, so the answer still needs most of the tree.
            let u = kids[rng.below(kids.len() as u64) as usize];
            (
                "nest_unit_total",
                format!(
                    "What is the consolidated spend of unit {}? Answer with FINAL over one row with column total (an integer, kEUR).",
                    g.units[u].name
                ),
                Verify::Number {
                    value: g.consolidated(u) as f64,
                    tolerance: 0.5,
                },
            )
        }
        2 => {
            let mut best: Vec<(i64, &str)> = kids
                .iter()
                .map(|c| (g.consolidated(*c), g.units[*c].name.as_str()))
                .collect();
            best.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(b.1)));
            (
                "nest_largest_unit",
                format!(
                    "Which sub-unit directly under unit {} (the top of the group), among those that are consolidated, has the largest consolidated spend? Answer with FINAL over one row: columns unit (its name) and total (an integer, kEUR). Break a tie by the name that sorts first.",
                    root.name
                ),
                Verify::Exact {
                    rows: vec![vec![best[0].1.to_string(), best[0].0.to_string()]],
                },
            )
        }
        3 => (
            "nest_units",
            format!(
                "How many units are consolidated into unit {}, the top of the group, counting it and leaving out every excluded sub-unit and everything under it? Answer with FINAL over one row with column units (an integer).",
                root.name
            ),
            Verify::Exact {
                rows: vec![vec![g.count(0).to_string()]],
            },
        ),
        _ => (
            "nest_levels",
            format!(
                "How many levels of sub-units lie below unit {}, the top of the group, once excluded sub-units and everything under them are left out? A sub-unit directly under it is on level 1. Answer with FINAL over one row with column levels (an integer).",
                root.name
            ),
            Verify::Exact {
                rows: vec![vec![g.depth(0).to_string()]],
            },
        ),
    };
    GeneratedTask {
        generator: "nest".into(),
        kind: kind.into(),
        task: format!("{PREAMBLE} {ask}"),
        context: Some(rows.join("\n\n")),
        verify,
        dial,
        difficulty: difficulty_prior(0.4 + 0.6 * dial),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_follow_the_dial() {
        assert_eq!(levels(0.0), 1);
        assert_eq!(levels(0.4), 3);
        assert_eq!(levels(0.6), 4);
        assert_eq!(levels(0.8), 5);
        assert_eq!(levels(1.0), 6);
    }

    #[test]
    fn the_spine_reaches_the_dialled_depth_and_exclusions_prune_whole_subtrees() {
        for seed in 1..40u64 {
            for dial in [0.4, 0.6, 0.8, 1.0] {
                let mut rng = Rng::new(seed);
                let g = build(dial, &mut rng);
                assert_eq!(g.depth(0), levels(dial), "seed {seed} dial {dial}");
                // Full tree depth equals the consolidated depth: the first
                // child is never excluded.
                let full = g.units.iter().map(|u| u.level).max().unwrap();
                assert_eq!(full, levels(dial));
                // An excluded unit and its descendants are not counted.
                let total_units = g.units.len() as i64;
                let excluded: i64 = g
                    .units
                    .iter()
                    .filter_map(|u| u.excluded)
                    .map(|e| {
                        // Size of e's subtree, exclusions below it or not.
                        fn size(g: &Group, i: usize) -> i64 {
                            1 + g.units[i].children.iter().map(|c| size(g, *c)).sum::<i64>()
                        }
                        size(&g, e)
                    })
                    .sum();
                // Nested exclusions can overlap, so this is a bound.
                assert!(g.count(0) >= total_units - excluded);
                assert!(g.count(0) <= total_units);
            }
        }
    }

    #[test]
    fn a_task_is_reproducible_and_its_context_names_every_unit_once() {
        let a =
            super::super::generators::generate("nest", 0.6, 7, std::path::Path::new(".")).unwrap();
        let b =
            super::super::generators::generate("nest", 0.6, 7, std::path::Path::new(".")).unwrap();
        assert_eq!(a, b);
        let ctx = a.context.unwrap();
        let reports: Vec<&str> = ctx
            .split("\n\n")
            .filter(|r| r.starts_with("Unit "))
            .collect();
        let corrections = ctx
            .split("\n\n")
            .filter(|r| r.starts_with("Correction:"))
            .count();
        assert!(reports.len() >= 8, "{}", reports.len());
        assert!(corrections <= reports.len());
        assert!(a.task.contains("consolidated"), "{}", a.task);
        assert!(a.kind.starts_with("nest_"));
    }

    #[test]
    fn every_kind_appears_across_seeds() {
        let mut kinds = std::collections::BTreeSet::new();
        for seed in 1..60u64 {
            let t =
                super::super::generators::generate("nest", 0.4, seed, std::path::Path::new("."))
                    .unwrap();
            kinds.insert(t.kind);
        }
        assert_eq!(
            kinds.into_iter().collect::<Vec<_>>(),
            [
                "nest_largest_unit",
                "nest_levels",
                "nest_total",
                "nest_unit_total",
                "nest_units"
            ]
        );
    }
}
