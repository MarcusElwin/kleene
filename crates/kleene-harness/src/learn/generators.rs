//! Task generators with code oracles. Every generator is a pure function of
//! its hardness dial and a seed, so a task can be regenerated and its
//! reference answer recomputed; the oracle is the [`Verify`] spec attached
//! to the task, never the model that solves it.

use super::verify::Verify;
use std::collections::{HashMap, VecDeque};

/// A generated task before it is stored.
#[derive(Debug, Clone, PartialEq)]
pub struct GeneratedTask {
    /// Generator name.
    pub generator: String,
    /// Task kind, for playbook lookup (`sat3`, `graph_path`, ...).
    pub kind: String,
    /// The task text the solver sees.
    pub task: String,
    /// Context loaded as `ctx(ordinal, text)`, if any.
    pub context: Option<String>,
    /// How the answer is checked.
    pub verify: Verify,
    /// The dial the task was generated at.
    pub dial: f64,
    /// Difficulty prior in rating points (1000 = even odds against a
    /// baseline solver).
    pub difficulty: f64,
}

/// Generators the harness knows.
pub const GENERATORS: &[&str] = &[
    "sat3",
    "graph",
    "puzzle",
    "corpus",
    "repo",
    "statements",
    "contracts",
    "logbook",
    "memo",
];

/// A small deterministic PRNG (xorshift64*), so tasks are reproducible.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    /// Seeded generator.
    pub fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    /// Next raw value.
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    /// Uniform in `0..n`.
    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 {
            0
        } else {
            self.next_u64() % n
        }
    }
    /// Uniform in `[0, 1)`.
    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// Generate one task. `dial` is the generator's hardness knob in `[0, 1]`;
/// `workspace` is needed by `repo`.
pub fn generate(
    generator: &str,
    dial: f64,
    seed: u64,
    workspace: &std::path::Path,
) -> Option<GeneratedTask> {
    let dial = dial.clamp(0.0, 1.0);
    let mut rng = Rng::new(seed);
    Some(match generator {
        "sat3" => sat3(dial, &mut rng),
        "graph" => graph(dial, &mut rng),
        "puzzle" => puzzle(dial, &mut rng),
        "corpus" => corpus(dial, &mut rng),
        "repo" => repo(dial, &mut rng, workspace)?,
        "statements" => statements(dial, &mut rng),
        "contracts" => contracts(dial, &mut rng),
        "logbook" => logbook(dial, &mut rng),
        "memo" => memo(dial, &mut rng),
        _ => return None,
    })
}

/// Finance: a synthetic income statement with planted figures across years
/// and a numerical question (growth, ratio or sum). The dial adds years,
/// line items and distractor notes.
fn statements(dial: f64, rng: &mut Rng) -> GeneratedTask {
    let years = 2 + (dial * 3.0).round() as usize;
    let items = [
        "Revenue",
        "Cost of revenue",
        "Operating expenses",
        "Interest expense",
        "Income tax",
        "Depreciation",
    ];
    let n_items = 3 + (dial * 3.0).round() as usize;
    let mut table: Vec<(String, Vec<i64>)> = vec![];
    for item in items.iter().take(n_items) {
        let base = 500 + rng.below(9000) as i64;
        let vals: Vec<i64> = (0..years)
            .map(|y| base + (y as i64) * (rng.below(400) as i64 - 100))
            .collect();
        table.push((item.to_string(), vals));
    }
    let mut lines = vec![format!(
        "Consolidated statement of operations (in thousands). Fiscal years: {}.",
        (0..years)
            .map(|y| (2020 + y).to_string())
            .collect::<Vec<_>>()
            .join(", ")
    )];
    for (item, vals) in &table {
        lines.push(format!(
            "{item}: {}",
            vals.iter()
                .enumerate()
                .map(|(y, v)| format!("FY{}: {}", 2020 + y, v))
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }
    let distractors = (dial * 6.0).round() as usize;
    for i in 0..distractors {
        lines.push(format!(
            "Note {}: figures are unaudited; segment {} reclassified in FY{}.",
            i + 1,
            i % 3 + 1,
            2020 + rng.below(years as u64) as usize
        ));
    }
    let (question, value, tolerance) = match rng.below(3) {
        0 => {
            let (item, vals) = &table[0];
            let a = vals[years - 2] as f64;
            let b = vals[years - 1] as f64;
            let growth = (b - a) / a * 100.0;
            (
                format!(
                    "By what percentage did {item} change from FY{} to FY{}? Answer with FINAL over one row with column pct (a number, one decimal is fine).",
                    2020 + years - 2,
                    2020 + years - 1
                ),
                growth,
                0.15,
            )
        }
        1 => {
            let (a_item, a_vals) = &table[0];
            let (b_item, b_vals) = &table[1];
            let ratio = a_vals[years - 1] as f64 / b_vals[years - 1] as f64;
            (
                format!(
                    "What is the ratio of {a_item} to {b_item} in FY{}? Answer with FINAL over one row with column ratio (two decimals).",
                    2020 + years - 1
                ),
                ratio,
                0.015,
            )
        }
        _ => {
            let total: i64 = table.iter().map(|(_, v)| v[years - 1]).sum();
            (
                format!(
                    "What is the sum of every line item in FY{}? Answer with FINAL over one row with column total (in thousands).",
                    2020 + years - 1
                ),
                total as f64,
                0.5,
            )
        }
    };
    GeneratedTask {
        generator: "statements".into(),
        kind: "finance_statement".into(),
        task: format!("ctx holds a financial statement, one line per row. {question}"),
        context: Some(lines.join("\n\n")),
        verify: Verify::Number { value, tolerance },
        dial,
        difficulty: difficulty_prior(dial),
    }
}

/// Legal: a synthetic contract with planted clause categories among
/// boilerplate; the task is to list which categories are present. The dial
/// adds length, paraphrase and near-miss distractors.
fn contracts(dial: f64, rng: &mut Rng) -> GeneratedTask {
    let categories: [(&str, [&str; 2]); 6] = [
        (
            "non-compete",
            [
                "The Supplier shall not engage in any business competing with the Customer for a period of two years.",
                "During the Restricted Period the Supplier agrees not to compete, directly or indirectly, with the Customer.",
            ],
        ),
        (
            "termination for convenience",
            [
                "Either party may terminate this Agreement for any reason upon ninety days' written notice.",
                "This Agreement may be terminated by either party without cause on ninety days' notice.",
            ],
        ),
        (
            "cap on liability",
            [
                "In no event shall either party's aggregate liability exceed the fees paid in the preceding twelve months.",
                "Each party's total liability under this Agreement is limited to the amounts paid in the prior year.",
            ],
        ),
        (
            "governing law",
            [
                "This Agreement shall be governed by the laws of the State of Delaware.",
                "The laws of Delaware govern this Agreement and any dispute arising from it.",
            ],
        ),
        (
            "audit rights",
            [
                "The Customer may audit the Supplier's relevant records once per year on reasonable notice.",
                "Upon reasonable notice the Customer shall have the right to inspect the Supplier's records annually.",
            ],
        ),
        (
            "exclusivity",
            [
                "The Customer shall purchase the Products exclusively from the Supplier during the Term.",
                "During the Term the Supplier is the Customer's sole source for the Products.",
            ],
        ),
    ];
    let boilerplate = [
        "The parties agree that headings are for convenience only.",
        "Notices shall be delivered in writing to the addresses set out in Schedule 1.",
        "This Agreement constitutes the entire agreement between the parties.",
        "If any provision is held invalid, the remainder shall continue in effect.",
        "Each party shall bear its own costs in connection with this Agreement.",
        "The Supplier shall deliver the Products in accordance with Schedule 2.",
        "Nothing in this Agreement creates a partnership or agency between the parties.",
    ];
    let near_misses = [
        "The Supplier may compete freely with the Customer after the Term ends.",
        "This Agreement may not be terminated except for material breach.",
        "Liability for gross negligence is unlimited.",
    ];
    let n_present = 1 + rng.below(4) as usize;
    let mut present: Vec<usize> = vec![];
    while present.len() < n_present {
        let c = rng.below(categories.len() as u64) as usize;
        if !present.contains(&c) {
            present.push(c);
        }
    }
    let mut clauses: Vec<String> = vec![];
    let length = 6 + (dial * 18.0).round() as usize;
    for i in 0..length {
        clauses.push(format!(
            "{}. {}",
            i + 1,
            boilerplate[rng.below(boilerplate.len() as u64) as usize]
        ));
    }
    for &c in &present {
        let variant = if rng.unit() < dial { 1 } else { 0 };
        let pos = rng.below(clauses.len() as u64) as usize;
        clauses.insert(pos, format!("{}a. {}", pos + 1, categories[c].1[variant]));
    }
    if dial > 0.4 {
        let pos = rng.below(clauses.len() as u64) as usize;
        clauses.insert(
            pos,
            format!(
                "{}b. {}",
                pos + 1,
                near_misses[rng.below(near_misses.len() as u64) as usize]
            ),
        );
    }
    let mut rows: Vec<Vec<String>> = present
        .iter()
        .map(|&c| vec![categories[c].0.to_string()])
        .collect();
    rows.sort();
    GeneratedTask {
        generator: "contracts".into(),
        kind: "legal_clause_categories".into(),
        task: format!(
            "ctx holds a contract, one clause per row. Which of these clause categories does it contain: {}? \
Answer with FINAL over one row per present category with column category (exactly the category name as written here).",
            categories
                .iter()
                .map(|(n, _)| n.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
        context: Some(clauses.join("\n\n")),
        verify: Verify::Exact { rows },
        dial,
        difficulty: difficulty_prior(dial),
    }
}

/// A long work log: hundreds to thousands of dated entries in several
/// phrasings, with entries about the same projects that carry numbers
/// that are not hours, and (above dial 0.3) corrections that amend an
/// earlier entry. Five question shapes, chosen by the seed. The dial sets
/// the entry count (300 at 0, 1800 at 1), the span of dates, and the
/// correction and distractor rates; at dial 0.9 a context is about 150,000
/// characters, so it cannot be answered by reading it once.
fn logbook(dial: f64, rng: &mut Rng) -> GeneratedTask {
    let projects = ["Heron", "Kestrel", "Osprey", "Plover", "Sandpiper", "Tern"];
    let people = [
        "Amara", "Bao", "Chidi", "Dana", "Eitan", "Farah", "Gita", "Hugo",
    ];
    let fillers = [
        "The coffee machine is broken again.",
        "Parking closes at 22:00.",
        "Design review moved rooms.",
        "All-hands cancelled next week.",
        "Remember to submit expense reports by Friday.",
        "The build server was slow all afternoon.",
    ];
    let n_projects = 3 + (dial * 3.0).round() as usize;
    let n_people = 4 + (dial * 4.0).round() as usize;
    let entries = 300 + (dial * 1500.0).round() as usize;
    let span_days = 60 + (dial * 120.0).round() as usize;
    let month_name = |m: usize| {
        [
            "January", "February", "March", "April", "May", "June", "July",
        ][m - 1]
    };
    // Day d (0-based) of 2026 from 1 January; months with their lengths.
    let date_of = |d: usize| -> (usize, usize) {
        let lens = [31, 28, 31, 30, 31, 30, 31];
        let (mut m, mut rem) = (1, d);
        for len in lens {
            if rem < len {
                break;
            }
            rem -= len;
            m += 1;
        }
        (m, rem + 1)
    };
    // Every hours entry: (date index, person, project, hours), in log order.
    let mut log: Vec<(usize, usize, usize, i64)> = vec![];
    let mut lines: Vec<String> = vec![];
    let mut day = 0usize;
    for _ in 0..entries {
        // Days advance at most one at a time so the log stays dated in order.
        if rng.unit() < span_days as f64 / entries as f64 {
            day = (day + 1).min(span_days - 1);
        }
        let (m, dd) = date_of(day);
        let date = format!("2026-{m:02}-{dd:02}");
        let p = rng.below(n_projects as u64) as usize;
        let who = rng.below(n_people as u64) as usize;
        let roll = rng.unit();
        let distractor_rate = 0.08 + 0.12 * dial;
        let correction_rate = if dial > 0.3 { 0.01 + 0.03 * dial } else { 0.0 };
        if roll < correction_rate && !log.is_empty() {
            // Amend an earlier entry: the log line names it by date, person
            // and project, and gives the new hours.
            let i = rng.below(log.len() as u64) as usize;
            let (d0, who0, p0, h0) = log[i];
            let mut h1 = 1 + rng.below(9) as i64;
            if h1 == h0 {
                h1 = if h0 < 9 { h0 + 1 } else { h0 - 1 };
            }
            log[i].3 = h1;
            let (m0, dd0) = date_of(d0);
            lines.push(format!(
                "{date}. Correction: the entry of 2026-{m0:02}-{dd0:02} for {} on project {} should read {h1} hours, not {h0}.",
                people[who0], projects[p0]
            ));
            continue;
        }
        if roll < correction_rate + distractor_rate {
            let n = 2 + rng.below(12) as i64;
            let which = rng.below(4);
            lines.push(match which {
                0 => format!(
                    "{date}. Project {}: planning session with {n} attendees; {} took notes.",
                    projects[p], people[who]
                ),
                1 => format!(
                    "{date}. {} booked room {n} for the {} sync.",
                    people[who], projects[p]
                ),
                2 => format!(
                    "{date}. Project {} budget review: {n} open action items, owner {}.",
                    projects[p], people[who]
                ),
                _ => format!(
                    "{date}. {} estimated the {} backlog at {n} days of work.",
                    people[who], projects[p]
                ),
            });
            continue;
        }
        let h = 1 + rng.below(9) as i64;
        log.push((day, who, p, h));
        let filler = if rng.unit() < 0.2 + 0.4 * dial {
            format!(" {}", fillers[rng.below(fillers.len() as u64) as usize])
        } else {
            String::new()
        };
        let template = rng.below(4);
        lines.push(match template {
            0 => format!(
                "{date}. Project {}: {} logged {h} hours.{filler}",
                projects[p], people[who]
            ),
            1 => format!(
                "{date}. {} spent {h} hours on {} today.{filler}",
                people[who], projects[p]
            ),
            2 => format!(
                "{date}. Time entry, {}, project {}: {h}h.{filler}",
                people[who], projects[p]
            ),
            _ => format!(
                "{date}. {} worked {h} hours on project {}.{filler}",
                people[who], projects[p]
            ),
        });
    }
    let question = rng.below(5);
    let target_project = rng.below(n_projects as u64) as usize;
    let months_seen: Vec<usize> = {
        let mut v: Vec<usize> = log.iter().map(|e| date_of(e.0).0).collect();
        v.sort();
        v.dedup();
        v
    };
    let target_month = months_seen[rng.below(months_seen.len() as u64) as usize];
    let mut sum: HashMap<usize, i64> = HashMap::new();
    let (kind, task, rows): (&str, String, Vec<Vec<String>>) = match question {
        0 => {
            for e in &log {
                *sum.entry(e.2).or_default() += e.3;
            }
            (
                "logbook_hours_by_project",
                "ctx holds a work log, one entry per row. Entries that record time say how many hours a person logged on a project, in a few phrasings; other entries mention projects and numbers that are not hours (attendees, room numbers, action items, estimates). A later entry beginning 'Correction:' replaces the hours of the earlier entry it names (same date, person and project). Answer with FINAL over one row per project: columns project (the project name) and total_hours (an integer, corrections applied).".into(),
                sum.iter().map(|(p, h)| vec![projects[*p].to_string(), h.to_string()]).collect(),
            )
        }
        1 => {
            for e in log.iter().filter(|e| e.2 == target_project) {
                *sum.entry(e.1).or_default() += e.3;
            }
            (
                "logbook_hours_by_person",
                format!("ctx holds a work log, one entry per row. Entries that record time say how many hours a person logged on a project, in a few phrasings; other entries mention projects and numbers that are not hours. A later entry beginning 'Correction:' replaces the hours of the earlier entry it names. For project {} only, answer with FINAL over one row per person who logged time on it: columns person (the name) and total_hours (an integer, corrections applied).", projects[target_project]),
                sum.iter().map(|(w, h)| vec![people[*w].to_string(), h.to_string()]).collect(),
            )
        }
        2 => {
            for e in &log {
                *sum.entry(e.1).or_default() += e.3;
            }
            let mut best: Vec<(i64, String)> = sum
                .iter()
                .map(|(w, h)| (*h, people[*w].to_string()))
                .collect();
            best.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
            let (h, w) = best[0].clone();
            (
                "logbook_top_person",
                "ctx holds a work log, one entry per row. Entries that record time say how many hours a person logged on a project, in a few phrasings; other entries mention people and numbers that are not hours. A later entry beginning 'Correction:' replaces the hours of the earlier entry it names. Who logged the most hours in total across every project? Answer with FINAL over one row: columns person (the name) and total_hours (an integer, corrections applied). Break a tie by the name that sorts first.".into(),
                vec![vec![w, h.to_string()]],
            )
        }
        3 => {
            for e in log.iter().filter(|e| date_of(e.0).0 == target_month) {
                *sum.entry(e.2).or_default() += e.3;
            }
            (
                "logbook_hours_by_project_month",
                format!("ctx holds a work log, one entry per row, each starting with its date (YYYY-MM-DD). Entries that record time say how many hours a person logged on a project, in a few phrasings; other entries mention projects and numbers that are not hours. A later entry beginning 'Correction:' replaces the hours of the earlier entry it names; the correction counts for the month of the entry it corrects. For {} 2026 only, answer with FINAL over one row per project with time logged that month: columns project (the project name) and total_hours (an integer).", month_name(target_month)),
                sum.iter().map(|(p, h)| vec![projects[*p].to_string(), h.to_string()]).collect(),
            )
        }
        _ => {
            let mut who: Vec<usize> = log
                .iter()
                .filter(|e| e.2 == target_project)
                .map(|e| e.1)
                .collect();
            who.sort();
            who.dedup();
            (
                "logbook_people_on_project",
                format!("ctx holds a work log, one entry per row. Entries that record time say how many hours a person logged on a project, in a few phrasings; other entries mention people and projects without recording time (attendees, room bookings, reviews, estimates) and do not count. How many distinct people logged time on project {}? Answer with FINAL over one row with column people (an integer).", projects[target_project]),
                vec![vec![who.len().to_string()]],
            )
        }
    };
    let mut rows = rows;
    rows.sort();
    GeneratedTask {
        generator: "logbook".into(),
        kind: kind.into(),
        task,
        context: Some(lines.join("\n\n")),
        verify: Verify::Exact { rows },
        dial,
        difficulty: difficulty_prior(0.5 + 0.5 * dial),
    }
}

/// A rubric-judged memo: a synthetic services agreement with planted
/// commercial terms among boilerplate, a rejected-draft near miss, and
/// (above dial 0.4) an amendment that supersedes one of the terms. The
/// task asks for a short written memo answering three questions about the
/// terms; the oracle is a `judge` model with a rubric that names each
/// required fact and forbids the superseded or rejected values, plus a
/// reference memo. The dial sets the contract length and whether an
/// amendment is present.
fn memo(dial: f64, rng: &mut Rng) -> GeneratedTask {
    let customers = [
        "Alder Logistics",
        "Brightwater Foods",
        "Cobalt Analytics",
        "Dunmore Health",
    ];
    let suppliers = ["Ferrule Systems", "Greyline Services", "Halyard Software"];
    let states = ["Delaware", "New York", "California", "Texas", "Washington"];
    let customer = customers[rng.below(customers.len() as u64) as usize];
    let supplier = suppliers[rng.below(suppliers.len() as u64) as usize];
    let state = states[rng.below(states.len() as u64) as usize];
    let convenience_notice = [30, 45, 60, 90, 120][rng.below(5) as usize];
    let cure_days = [10, 15, 30, 45][rng.below(4) as usize];
    let cap_months = [6, 12, 24][rng.below(3) as usize];
    let net_days = [15, 30, 45, 60][rng.below(4) as usize];
    let late_pct = [1.0, 1.5, 2.0][rng.below(3) as usize];
    let term_years = 1 + rng.below(4) as usize;
    let renewal_notice = [30, 60, 90][rng.below(3) as usize];
    let rejected_cap = if cap_months == 12 { 24 } else { 12 };
    // Facts: (key, clause text, memo sentence, rubric line).
    let mut facts: Vec<(&str, String, String, String)> = vec![
        (
            "convenience",
            format!("Either party may terminate this Agreement for convenience on {convenience_notice} days' written notice to the other party."),
            format!("Either party may terminate for convenience on {convenience_notice} days' written notice."),
            format!("states that either party may terminate for convenience on {convenience_notice} days' written notice (the number of days must be {convenience_notice})"),
        ),
        (
            "breach",
            format!("Either party may terminate this Agreement for material breach if the breach is not cured within {cure_days} days of written notice describing it."),
            format!("Termination for material breach requires written notice and a {cure_days}-day cure period."),
            format!("states that termination for breach requires a cure period of {cure_days} days after written notice"),
        ),
        (
            "cap",
            format!("Each party's aggregate liability under this Agreement shall not exceed the fees paid by the Customer in the {cap_months} months preceding the claim."),
            format!("Each party's aggregate liability is capped at the fees paid in the {cap_months} months before the claim."),
            format!("states that liability is capped at the fees paid in the {cap_months} months preceding the claim (not {rejected_cap} months)"),
        ),
        (
            "payment",
            format!("The Customer shall pay each invoice within {net_days} days of receipt; overdue amounts bear interest at {late_pct}% per month."),
            format!("Invoices are due within {net_days} days of receipt, and overdue amounts bear interest at {late_pct}% per month."),
            format!("states that invoices are due {net_days} days from receipt and that late interest is {late_pct}% per month"),
        ),
        (
            "law",
            format!("This Agreement is governed by the laws of the State of {state}, without regard to its conflict of laws rules."),
            format!("The Agreement is governed by {state} law."),
            format!("states that the governing law is that of {state}"),
        ),
        (
            "term",
            format!("The initial term is {term_years} year{} from the Effective Date and renews automatically for successive one-year periods unless either party gives {renewal_notice} days' notice of non-renewal before the end of the current term.", if term_years == 1 { "" } else { "s" }),
            format!("The initial term is {term_years} year{}, renewing automatically for one-year periods unless a party gives {renewal_notice} days' notice of non-renewal.", if term_years == 1 { "" } else { "s" }),
            format!("states an initial term of {term_years} year{} with automatic one-year renewals and a {renewal_notice}-day non-renewal notice", if term_years == 1 { "" } else { "s" }),
        ),
    ];
    let boilerplate = [
        "The parties agree that headings are for convenience only.",
        "Notices shall be delivered in writing to the addresses set out in Schedule 1.",
        "This Agreement constitutes the entire agreement between the parties and supersedes all prior discussions.",
        "If any provision is held invalid, the remainder shall continue in effect.",
        "Each party shall bear its own costs in connection with this Agreement.",
        "The Supplier shall deliver the Services in accordance with Schedule 2.",
        "Nothing in this Agreement creates a partnership or agency between the parties.",
        "Each party shall keep the other's Confidential Information in confidence and use it only for the purposes of this Agreement.",
        "The Supplier shall maintain insurance customary for providers of similar services.",
        "Neither party is liable for delay caused by events beyond its reasonable control.",
        "This Agreement may be executed in counterparts, each of which is an original.",
        "The Customer shall provide the access and information the Supplier reasonably needs to perform the Services.",
    ];
    // An amendment supersedes one fact above dial 0.4: the memo must give
    // the amended value and must not present the original as current.
    let amended = if dial > 0.4 {
        Some(rng.below(3) as usize)
    } else {
        None
    };
    let mut amendment_clause = None;
    if let Some(i) = amended {
        let (key, clause, sentence, rubric) = match i {
            0 => {
                let n2 = if convenience_notice == 90 {
                    180
                } else {
                    convenience_notice * 2
                };
                (
                    "convenience",
                    format!("Amendment No. 1. Section {{sec}} is deleted and replaced as follows: either party may terminate this Agreement for convenience on {n2} days' written notice."),
                    format!("Either party may terminate for convenience on {n2} days' written notice (as amended by Amendment No. 1; the original {convenience_notice}-day notice no longer applies)."),
                    format!("states that either party may terminate for convenience on {n2} days' written notice, the period set by Amendment No. 1, and does not present the original {convenience_notice} days as the current notice period"),
                )
            }
            1 => {
                let m2 = cap_months * 2;
                (
                    "cap",
                    format!("Amendment No. 1. Section {{sec}} is deleted and replaced as follows: each party's aggregate liability shall not exceed the fees paid by the Customer in the {m2} months preceding the claim."),
                    format!("Each party's aggregate liability is capped at the fees paid in the {m2} months before the claim (as amended by Amendment No. 1; the original {cap_months}-month cap no longer applies)."),
                    format!("states that liability is capped at the fees paid in the {m2} months preceding the claim, the cap set by Amendment No. 1, and does not present the original {cap_months} months as the current cap"),
                )
            }
            _ => {
                let d2 = net_days + 15;
                (
                    "payment",
                    format!("Amendment No. 1. Section {{sec}} is deleted and replaced as follows: the Customer shall pay each invoice within {d2} days of receipt; overdue amounts bear interest at {late_pct}% per month."),
                    format!("Invoices are due within {d2} days of receipt (as amended by Amendment No. 1; the original {net_days}-day term no longer applies), and overdue amounts bear interest at {late_pct}% per month."),
                    format!("states that invoices are due {d2} days from receipt, the term set by Amendment No. 1, and does not present the original {net_days} days as the current payment term; late interest is {late_pct}% per month"),
                )
            }
        };
        let f = facts.iter_mut().find(|f| f.0 == key).expect("fact");
        f.2 = sentence;
        f.3 = rubric;
        amendment_clause = Some(clause);
    }
    // Lay the contract out: boilerplate with the facts at random positions,
    // numbered as sections; the near miss and the amendment go last.
    let length = 12 + (dial * 24.0).round() as usize;
    let mut clauses: Vec<String> = (0..length)
        .map(|_| boilerplate[rng.below(boilerplate.len() as u64) as usize].to_string())
        .collect();
    let mut positions: Vec<usize> = vec![];
    for f in &facts {
        let pos = rng.below(clauses.len() as u64) as usize;
        clauses.insert(pos, f.1.clone());
        for p in positions.iter_mut() {
            if *p >= pos {
                *p += 1;
            }
        }
        positions.push(pos);
    }
    let mut numbered: Vec<String> = clauses
        .iter()
        .enumerate()
        .map(|(i, c)| format!("Section {}. {c}", i + 1))
        .collect();
    numbered.insert(
        0,
        format!(
            "MASTER SERVICES AGREEMENT between {customer} (the \"Customer\") and {supplier} (the \"Supplier\"), effective 1 March 2026 (the \"Effective Date\")."
        ),
    );
    numbered.push(format!(
        "Schedule 3 (negotiation history, not part of the operative terms). The Customer proposed a liability cap of the fees paid in the {rejected_cap} months preceding the claim; the proposal was rejected and does not form part of this Agreement."
    ));
    if let Some(clause) = amendment_clause {
        let idx = amended.expect("amended index");
        let key = ["convenience", "cap", "payment"][idx];
        let fi = facts.iter().position(|f| f.0 == key).expect("fact");
        let sec = positions[fi] + 1;
        numbered.push(format!(
            "{} Dated 15 June 2026 and signed by both parties.",
            clause.replace("{sec}", &sec.to_string())
        ));
    }
    // Ask three of the six facts.
    let mut asked: Vec<usize> = vec![];
    while asked.len() < 3 {
        let i = rng.below(facts.len() as u64) as usize;
        if !asked.contains(&i) {
            asked.push(i);
        }
    }
    asked.sort();
    let questions: Vec<&str> = asked
        .iter()
        .map(|&i| match facts[i].0 {
            "convenience" => "how either party can terminate the agreement for convenience, and on how much notice",
            "breach" => "what happens if a party is in material breach, including any cure period",
            "cap" => "what the current cap on each party's liability is",
            "payment" => "when invoices are due and what interest overdue amounts bear",
            "law" => "which law governs the agreement",
            _ => "how long the initial term is and how renewal and non-renewal work",
        })
        .collect();
    let reference = asked
        .iter()
        .map(|&i| facts[i].2.clone())
        .collect::<Vec<_>>()
        .join(" ");
    let rubric = format!(
        "The answer is a memo about the agreement between {customer} and {supplier}. PASS only if every line below holds; FAIL if any does not.\n- {}\n- The memo does not attribute to the agreement any term it does not contain (a term from the rejected proposal in Schedule 3, or an invented number, is a FAIL).\n- Wording may differ from the reference; only the facts matter.",
        asked
            .iter()
            .map(|&i| facts[i].3.clone())
            .collect::<Vec<_>>()
            .join("\n- ")
    );
    let task = format!(
        "ctx holds a services agreement, one section per row, including its schedules and any amendment. Read it as a lawyer would: a later amendment replaces the section it names, and a negotiation schedule is not an operative term. Write a short memo for the Customer that answers, with the specific numbers and periods from the agreement: (1) {}; (2) {}; (3) {}. Answer with FINAL over one row with a single column memo holding the memo text.",
        questions[0], questions[1], questions[2]
    );
    GeneratedTask {
        generator: "memo".into(),
        kind: "memo_contract_terms".into(),
        task,
        context: Some(numbered.join("\n\n")),
        verify: Verify::Judge {
            rubric,
            reference: Some(reference),
        },
        dial,
        difficulty: difficulty_prior(0.4 + 0.6 * dial),
    }
}

fn difficulty_prior(dial: f64) -> f64 {
    // 0 -> 800 (easy), 1 -> 1600 (hard).
    800.0 + 800.0 * dial
}

/// Random 3-SAT at a clause/variable ratio near the phase transition; the
/// dial sets the variable count (4..=12) and the ratio (3.0..=4.6).
fn sat3(dial: f64, rng: &mut Rng) -> GeneratedTask {
    let n = 4 + (dial * 8.0).round() as usize;
    let ratio = 3.0 + 1.6 * dial;
    let m = ((n as f64) * ratio).round() as usize;
    let mut clauses: Vec<[i32; 3]> = vec![];
    for _ in 0..m {
        let mut lits = [0i32; 3];
        let mut used = vec![];
        for l in lits.iter_mut() {
            let mut v;
            loop {
                v = 1 + rng.below(n as u64) as i32;
                if !used.contains(&v) {
                    break;
                }
            }
            used.push(v);
            *l = if rng.below(2) == 0 { v } else { -v };
        }
        clauses.push(lits);
    }
    let rendered: Vec<String> = clauses
        .iter()
        .map(|c| format!("({} OR {} OR {})", lit(c[0]), lit(c[1]), lit(c[2])))
        .collect();
    let satisfiable = super::verify::sat_solve(n, &clauses).is_some();
    GeneratedTask {
        generator: "sat3".into(),
        kind: "sat3".into(),
        task: format!(
            "Decide whether this 3-SAT formula over variables x1..x{n} is satisfiable. Clauses (one per row of ctx too): {}. \
Answer with FINAL over one row: column verdict = 'SAT' or 'UNSAT', and if SAT a column assignment listing the true variables separated by spaces (e.g. 'x1 x3').",
            rendered.join(" AND ")
        ),
        context: Some(rendered.join("\n\n")),
        verify: Verify::Sat {
            vars: n,
            clauses: clauses.clone(),
            satisfiable,
        },
        dial,
        difficulty: difficulty_prior(dial),
    }
}

fn lit(l: i32) -> String {
    if l < 0 {
        format!("NOT x{}", -l)
    } else {
        format!("x{l}")
    }
}

/// Shortest path in a random directed graph; the dial sets the node count
/// and edge density.
fn graph(dial: f64, rng: &mut Rng) -> GeneratedTask {
    let n = 5 + (dial * 15.0).round() as usize;
    let density = 0.15 + 0.2 * dial;
    let mut edges: Vec<(usize, usize)> = vec![];
    for a in 0..n {
        for b in 0..n {
            if a != b && rng.unit() < density {
                edges.push((a, b));
            }
        }
    }
    // Guarantee a spine so the graph is connected enough to be interesting.
    for a in 0..n.saturating_sub(1) {
        if rng.unit() < 0.7 && !edges.contains(&(a, a + 1)) {
            edges.push((a, a + 1));
        }
    }
    let from = 0;
    let to = n - 1;
    let dist = bfs(n, &edges, from, to);
    let lines: Vec<String> = edges.iter().map(|(a, b)| format!("n{a} -> n{b}")).collect();
    GeneratedTask {
        generator: "graph".into(),
        kind: "graph_path".into(),
        task: format!(
            "A directed graph is given in ctx, one edge per row as 'nA -> nB'. \
What is the length (number of edges) of the shortest path from n{from} to n{to}? \
Answer with FINAL over one row with column hops (an integer), or hops = -1 if there is no path."
        ),
        context: Some(lines.join("\n\n")),
        verify: Verify::Exact {
            rows: vec![vec![dist.map(|d| d.to_string()).unwrap_or("-1".into())]],
        },
        dial,
        difficulty: difficulty_prior(dial),
    }
}

fn bfs(n: usize, edges: &[(usize, usize)], from: usize, to: usize) -> Option<usize> {
    let mut adj: HashMap<usize, Vec<usize>> = HashMap::new();
    for (a, b) in edges {
        adj.entry(*a).or_default().push(*b);
    }
    let mut dist = vec![usize::MAX; n];
    dist[from] = 0;
    let mut q = VecDeque::from([from]);
    while let Some(u) = q.pop_front() {
        if u == to {
            return Some(dist[u]);
        }
        for &v in adj.get(&u).map(|v| v.as_slice()).unwrap_or(&[]) {
            if dist[v] == usize::MAX {
                dist[v] = dist[u] + 1;
                q.push_back(v);
            }
        }
    }
    None
}

/// A procedural arithmetic puzzle over a list of numbers in ctx: sum the
/// numbers divisible by k. The dial sets the list length and the divisor.
fn puzzle(dial: f64, rng: &mut Rng) -> GeneratedTask {
    let len = 8 + (dial * 40.0).round() as usize;
    let k = 2 + rng.below(2 + (dial * 5.0).round() as u64) as i64;
    let nums: Vec<i64> = (0..len).map(|_| 1 + rng.below(200) as i64).collect();
    let total: i64 = nums.iter().filter(|x| *x % k == 0).sum();
    GeneratedTask {
        generator: "puzzle".into(),
        kind: "puzzle_divisible_sum".into(),
        task: format!(
            "ctx holds {len} integers, one per row. Answer with FINAL over one row with column total = the sum of the numbers divisible by {k} (0 if none)."
        ),
        context: Some(
            nums.iter()
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join("\n\n"),
        ),
        verify: Verify::Exact {
            rows: vec![vec![total.to_string()]],
        },
        dial,
        difficulty: difficulty_prior(dial * 0.6),
    }
}

/// OOLONG-like: dated meeting notes about projects, with a per-project
/// aggregate to compute. The dial sets the note count and distractor rate.
fn corpus(dial: f64, rng: &mut Rng) -> GeneratedTask {
    let projects = ["Heron", "Kestrel", "Osprey", "Plover", "Sandpiper", "Tern"];
    let people = ["Amara", "Bao", "Chidi", "Dana", "Eitan", "Farah"];
    let fillers = [
        "The coffee machine is broken again.",
        "Parking closes at 22:00.",
        "Design review moved rooms.",
        "All-hands cancelled next week.",
    ];
    let n_projects = 2 + (dial * 4.0).round() as usize;
    let notes = 12 + (dial * 60.0).round() as usize;
    let mut hours: HashMap<&str, i64> = HashMap::new();
    let mut lines = vec![];
    for i in 0..notes {
        let p = projects[rng.below(n_projects as u64) as usize];
        let who = people[rng.below(people.len() as u64) as usize];
        let h = 1 + rng.below(9) as i64;
        *hours.entry(p).or_default() += h;
        let filler = if rng.unit() < 0.3 + 0.5 * dial {
            format!(" {}", fillers[rng.below(fillers.len() as u64) as usize])
        } else {
            String::new()
        };
        lines.push(format!(
            "Meeting notes 2026-03-{:02}. Project {p}: {who} logged {h} hours.{filler}",
            1 + i % 28
        ));
    }
    let mut rows: Vec<Vec<String>> = hours
        .iter()
        .map(|(p, h)| vec![p.to_string(), h.to_string()])
        .collect();
    rows.sort();
    GeneratedTask {
        generator: "corpus".into(),
        kind: "corpus_hours_by_project".into(),
        task: "ctx holds meeting notes, one per row, each mentioning a project and hours logged. \
Answer with FINAL over one row per project: columns project (the project name) and total_hours (an integer)."
            .into(),
        context: Some(lines.join("\n\n")),
        verify: Verify::Exact { rows },
        dial,
        difficulty: difficulty_prior(dial),
    }
}

/// A question about the workspace checkout answered by walking it: how many
/// files with a given extension live under a directory. The dial picks
/// deeper directories.
fn repo(dial: f64, rng: &mut Rng, workspace: &std::path::Path) -> Option<GeneratedTask> {
    let mut dirs: Vec<std::path::PathBuf> = vec![];
    collect_dirs(
        workspace,
        workspace,
        0,
        1 + (dial * 3.0).round() as usize,
        &mut dirs,
    );
    if dirs.is_empty() {
        return None;
    }
    let dir = dirs[rng.below(dirs.len() as u64) as usize].clone();
    let exts = ["rs", "md", "toml", "sql", "txt", "yml"];
    let ext = exts[rng.below(exts.len() as u64) as usize];
    let count = count_files(&workspace.join(&dir), ext);
    let rel = dir.display().to_string();
    let rel = if rel.is_empty() { ".".to_string() } else { rel };
    Some(GeneratedTask {
        generator: "repo".into(),
        kind: "repo_count_files".into(),
        task: format!(
            "In this workspace, how many files with the extension .{ext} are there under the directory '{rel}' (recursively)? \
Use the files tool. Answer with FINAL over one row with column count (an integer)."
        ),
        context: None,
        verify: Verify::Exact {
            rows: vec![vec![count.to_string()]],
        },
        dial,
        difficulty: difficulty_prior(dial),
    })
}

fn collect_dirs(
    root: &std::path::Path,
    dir: &std::path::Path,
    depth: usize,
    max_depth: usize,
    out: &mut Vec<std::path::PathBuf>,
) {
    if depth > max_depth {
        return;
    }
    if let Ok(rel) = dir.strip_prefix(root) {
        out.push(rel.to_path_buf());
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || name == "target" || name == "node_modules" {
            continue;
        }
        if e.path().is_dir() {
            collect_dirs(root, &e.path(), depth + 1, max_depth, out);
        }
    }
}

fn count_files(dir: &std::path::Path, ext: &str) -> usize {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut n = 0;
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || name == "target" || name == "node_modules" {
            continue;
        }
        if p.is_dir() {
            n += count_files(&p, ext);
        } else if p.extension().is_some_and(|x| x == ext) {
            n += 1;
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generators_are_deterministic_and_self_consistent() {
        let ws = std::env::temp_dir();
        for g in GENERATORS {
            if *g == "repo" {
                continue;
            }
            let a = generate(g, 0.5, 7, &ws).unwrap();
            let b = generate(g, 0.5, 7, &ws).unwrap();
            assert_eq!(a, b, "{g} must be deterministic");
            let c = generate(g, 0.5, 8, &ws).unwrap();
            assert_ne!(
                a.task.clone() + &a.context.clone().unwrap_or_default(),
                c.task + &c.context.unwrap_or_default(),
                "{g} must vary with the seed"
            );
            assert!(a.difficulty >= 800.0 && a.difficulty <= 1600.0);
        }
    }

    #[test]
    fn dial_makes_tasks_bigger() {
        let ws = std::env::temp_dir();
        let easy = generate("puzzle", 0.0, 1, &ws).unwrap();
        let hard = generate("puzzle", 1.0, 1, &ws).unwrap();
        assert!(hard.context.unwrap().lines().count() > easy.context.unwrap().lines().count());
        let easy = generate("sat3", 0.0, 1, &ws).unwrap();
        let hard = generate("sat3", 1.0, 1, &ws).unwrap();
        assert!(hard.context.unwrap().lines().count() > easy.context.unwrap().lines().count());
    }

    /// `5h.`: digits then the short unit.
    fn is_hours_short(w: &str) -> bool {
        w.strip_suffix("h.")
            .is_some_and(|d| !d.is_empty() && d.chars().all(|c| c.is_ascii_digit()))
    }

    /// Recount a logbook context the way a careful reader would, applying
    /// corrections, to check the generator's answer for the per-project
    /// question against its own text.
    fn recount_logbook(ctx: &str) -> HashMap<String, i64> {
        let mut entries: Vec<(String, String, String, i64)> = vec![];
        for line in ctx.split("\n\n") {
            let (date, rest) = line.split_once(". ").unwrap();
            if let Some(c) = rest.strip_prefix("Correction: the entry of ") {
                // "<date> for <who> on project <p> should read <h1> hours, not <h0>."
                let (d0, tail) = c.split_once(" for ").unwrap();
                let (who, tail) = tail.split_once(" on project ").unwrap();
                let (p, tail) = tail.split_once(" should read ").unwrap();
                let (h1, tail) = tail.split_once(" hours, not ").unwrap();
                let h0: i64 = tail.trim_end_matches('.').parse().unwrap();
                let e = entries
                    .iter_mut()
                    .find(|e| e.0 == d0 && e.1 == who && e.2 == p && e.3 == h0)
                    .expect("a correction names an entry");
                e.3 = h1.parse().unwrap();
                continue;
            }
            let words: Vec<&str> = rest.split_whitespace().collect();
            let hours = words
                .iter()
                .position(|w| *w == "hours" || *w == "hours." || is_hours_short(w));
            let Some(hi) = hours else { continue };
            let h: i64 = words[if is_hours_short(words[hi]) {
                hi
            } else {
                hi - 1
            }]
            .trim_end_matches("h.")
            .parse()
            .unwrap_or_else(|_| panic!("no hours in {line}"));
            let cap = |w: &str| w.trim_matches(|c: char| !c.is_alphanumeric()).to_string();
            let (who, p) = if rest.starts_with("Project ") {
                (cap(words[2]), cap(words[1]))
            } else if rest.starts_with("Time entry") {
                (cap(words[2]), cap(words[4]))
            } else if words[1] == "spent" {
                (cap(words[0]), cap(words[5]))
            } else {
                (cap(words[0]), cap(words[6]))
            };
            entries.push((date.to_string(), who, p, h));
        }
        let mut sum = HashMap::new();
        for e in entries {
            *sum.entry(e.2).or_insert(0) += e.3;
        }
        sum
    }

    #[test]
    fn logbook_answers_match_a_recount_of_their_own_text() {
        let ws = std::env::temp_dir();
        let mut checked = 0;
        for seed in 1..40u64 {
            for dial in [0.0, 0.9] {
                let t = generate("logbook", dial, seed, &ws).unwrap();
                if t.kind != "logbook_hours_by_project" {
                    continue;
                }
                let recount = recount_logbook(t.context.as_deref().unwrap());
                let Verify::Exact { rows } = &t.verify else {
                    panic!()
                };
                let mut want: Vec<(String, i64)> = recount.into_iter().collect();
                want.sort();
                let got: Vec<(String, i64)> = rows
                    .iter()
                    .map(|r| (r[0].clone(), r[1].parse().unwrap()))
                    .collect();
                assert_eq!(got, want, "seed {seed} dial {dial}");
                checked += 1;
            }
        }
        assert!(checked >= 4, "{checked}");
        let hard = generate("logbook", 0.9, 1, &ws).unwrap();
        assert!(hard.context.as_ref().unwrap().len() > 80_000);
        assert!(hard.context.as_ref().unwrap().contains("Correction:"));
        let easy = generate("logbook", 0.0, 1, &ws).unwrap();
        assert!(!easy.context.as_ref().unwrap().contains("Correction:"));
    }

    #[test]
    fn memo_rubrics_name_the_amended_terms_and_carry_a_reference() {
        let ws = std::env::temp_dir();
        let mut amended = 0;
        for seed in 1..20u64 {
            let t = generate("memo", 0.8, seed, &ws).unwrap();
            let Verify::Judge { rubric, reference } = &t.verify else {
                panic!()
            };
            let ctx = t.context.as_deref().unwrap();
            assert!(ctx.contains("Amendment No. 1"), "{ctx}");
            assert!(ctx.contains("Schedule 3"));
            assert_eq!(rubric.matches("\n- ").count(), 5, "{rubric}");
            assert!(reference.as_ref().unwrap().len() > 60);
            if rubric.contains("Amendment No. 1") {
                amended += 1;
                assert!(reference.as_ref().unwrap().contains("as amended"));
            }
        }
        assert!(amended >= 5, "{amended}");
        let plain = generate("memo", 0.2, 1, &ws).unwrap();
        assert!(!plain.context.unwrap().contains("Amendment"));
    }

    #[test]
    fn bfs_finds_shortest_paths() {
        assert_eq!(bfs(4, &[(0, 1), (1, 2), (2, 3), (0, 3)], 0, 3), Some(1));
        assert_eq!(bfs(3, &[(0, 1)], 0, 2), None);
    }
}
