//! The harder generators behind the `*-hard` packs: `tracker` (OOLONG-like
//! at scale, with action items and mixed time units), `ledger` (a segment
//! income statement with restatements and a discontinued segment) and
//! `agreement` (a long contract with amendments and near-miss clauses).
//!
//! The original `corpus`, `statements` and `contracts` generators top out at
//! a context that fits in one prompt, so every model solved their packs in
//! every mode and the packs compared cost only. These three are built so
//! that a context cannot be read once and answered: tens of thousands of
//! tokens, later entries that amend earlier ones, numbers in more than one
//! unit, and questions that join two kinds of entry. Like every generator
//! they are pure functions of the dial and the seed, so a lazy pack
//! regenerates the same task on load and the reference answer is computed
//! from the same state the text was written from.

use super::generators::{GeneratedTask, Rng};
use super::verify::Verify;
use std::collections::{HashMap, HashSet};

/// Difficulty prior in rating points for a dial, on the same scale as the
/// other generators (800 at 0, 1600 at 1), floored at the hard end because
/// these packs exist to be hard.
fn prior(dial: f64) -> f64 {
    800.0 + 800.0 * (0.6 + 0.4 * dial)
}

/// Day `d` (0-based from 1 January 2026) as `(month, day)`, January to
/// September.
fn date_of(d: usize) -> (usize, usize) {
    let lens = [31, 28, 31, 30, 31, 30, 31, 31, 30];
    let (mut m, mut rem) = (1, d);
    for len in lens {
        if rem < len {
            break;
        }
        rem -= len;
        m += 1;
    }
    (m, rem + 1)
}

fn month_name(m: usize) -> &'static str {
    [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
    ][m - 1]
}

fn date_str(d: usize) -> String {
    let (m, dd) = date_of(d);
    format!("2026-{m:02}-{dd:02}")
}

/// Minutes as the model should report hours: `12` or `12.5`.
fn hours_text(minutes: i64) -> String {
    if minutes % 60 == 0 {
        (minutes / 60).to_string()
    } else {
        format!("{}.{}", minutes / 60, (minutes % 60) * 10 / 60)
    }
}

/// Minutes written the way a log entry would: whole hours, decimal hours,
/// minutes, or hours and minutes, chosen by `style`.
fn duration_text(minutes: i64, style: u64) -> String {
    match style {
        0 if minutes % 60 == 0 => format!(
            "{} hour{}",
            minutes / 60,
            if minutes == 60 { "" } else { "s" }
        ),
        0 => format!("{} hours", hours_text(minutes)),
        1 => format!("{minutes} minutes"),
        _ if minutes % 60 == 0 => format!("{}h", minutes / 60),
        _ => format!("{}h {}m", minutes / 60, minutes % 60),
    }
}

/// OOLONG-like at scale: a stream of dated project notes where time is
/// logged in hours, decimal hours, minutes or `2h 30m`, action items are
/// opened on a project and closed later by id alone, decisions are
/// recorded per project, other entries carry numbers that are not time,
/// and `Correction:` entries amend an earlier time entry. Five question
/// shapes, chosen by the seed: hours per project, open action items per
/// project at the end of the log, the project with the most decisions in a
/// month, hours per person on one project in one month, and how many of a
/// project's action items were closed within a week. At dial 0.9 a context
/// is about 1,300 entries and 25,000 tokens.
pub(super) fn tracker(dial: f64, rng: &mut Rng) -> GeneratedTask {
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
        "Facilities will test the fire alarm on Thursday.",
    ];
    let item_texts = [
        "update the runbook",
        "renew the staging certificate",
        "write the migration plan",
        "review the vendor quote",
        "fix the flaky integration test",
        "draft the Q3 roadmap",
        "archive the old dashboards",
        "schedule the security review",
    ];
    let decision_texts = [
        "ship behind a feature flag",
        "postpone the launch by two weeks",
        "adopt the new logging format",
        "keep the current vendor for another year",
        "move the standup to 09:30",
        "deprecate the v1 endpoint",
    ];
    let n_projects = 4 + (dial * 2.0).round() as usize;
    let n_people = 5 + (dial * 3.0).round() as usize;
    let entries = 400 + (dial * 1000.0).round() as usize;
    let span_days = 90 + (dial * 150.0).round() as usize;
    // Time entries: (day, person, project, minutes), in log order.
    let mut log: Vec<(usize, usize, usize, i64)> = vec![];
    // Action items: id -> (project, opened day, closed day).
    let mut items: Vec<(u64, usize, usize, Option<usize>)> = vec![];
    // Decisions: (day, project).
    let mut decisions: Vec<(usize, usize)> = vec![];
    let mut lines: Vec<String> = vec![];
    let mut day = 0usize;
    let mut next_id = 1000 + rng.below(400);
    for _ in 0..entries {
        if rng.unit() < span_days as f64 / entries as f64 {
            day = (day + 1).min(span_days - 1);
        }
        let date = date_str(day);
        let p = rng.below(n_projects as u64) as usize;
        let who = rng.below(n_people as u64) as usize;
        let roll = rng.unit();
        let correction_rate = 0.02 + 0.04 * dial;
        let open_rate = 0.12;
        let close_rate = 0.10;
        let decision_rate = 0.08;
        let distractor_rate = 0.06 + 0.08 * dial;
        if roll < correction_rate && !log.is_empty() {
            let i = rng.below(log.len() as u64) as usize;
            let (d0, who0, p0, m0) = log[i];
            let mut m1 = 30 * (1 + rng.below(16) as i64);
            if m1 == m0 {
                m1 = if m0 < 480 { m0 + 30 } else { m0 - 30 };
            }
            log[i].3 = m1;
            lines.push(format!(
                "{date}. Correction: the {} entry for {} on project {} should read {}, not {}.",
                date_str(d0),
                people[who0],
                projects[p0],
                duration_text(m1, rng.below(3)),
                duration_text(m0, rng.below(3)),
            ));
            continue;
        }
        if roll < correction_rate + open_rate {
            let id = next_id;
            next_id += 1 + rng.below(3);
            items.push((id, p, day, None));
            lines.push(format!(
                "{date}. Action item AI-{id} opened on project {}, owner {}: {}.",
                projects[p],
                people[who],
                item_texts[rng.below(item_texts.len() as u64) as usize]
            ));
            continue;
        }
        if roll < correction_rate + open_rate + close_rate {
            let open: Vec<usize> = items
                .iter()
                .enumerate()
                .filter(|(_, it)| it.3.is_none())
                .map(|(i, _)| i)
                .collect();
            if let Some(&i) = open.get(rng.below(open.len() as u64) as usize) {
                items[i].3 = Some(day);
                lines.push(match rng.below(2) {
                    0 => format!(
                        "{date}. Action item AI-{} closed by {}.",
                        items[i].0, people[who]
                    ),
                    _ => format!("{date}. {} marked AI-{} as done.", people[who], items[i].0),
                });
                continue;
            }
        }
        if roll < correction_rate + open_rate + close_rate + decision_rate {
            decisions.push((day, p));
            lines.push(format!(
                "{date}. Decision ({}): {}; proposed by {}.",
                projects[p],
                decision_texts[rng.below(decision_texts.len() as u64) as usize],
                people[who]
            ));
            continue;
        }
        if roll < correction_rate + open_rate + close_rate + decision_rate + distractor_rate {
            let n = 2 + rng.below(12) as i64;
            lines.push(match rng.below(4) {
                0 => format!(
                    "{date}. Project {}: planning session with {n} attendees; {} took notes.",
                    projects[p], people[who]
                ),
                1 => format!(
                    "{date}. {} booked room {n} for the {} sync.",
                    people[who], projects[p]
                ),
                2 => format!(
                    "{date}. Project {} budget review: {n} open questions, owner {}.",
                    projects[p], people[who]
                ),
                _ => format!(
                    "{date}. {} estimated the {} backlog at {n} days of work.",
                    people[who], projects[p]
                ),
            });
            continue;
        }
        let minutes = 30 * (1 + rng.below(16) as i64);
        log.push((day, who, p, minutes));
        let filler = if rng.unit() < 0.2 + 0.4 * dial {
            format!(" {}", fillers[rng.below(fillers.len() as u64) as usize])
        } else {
            String::new()
        };
        let dur = duration_text(minutes, rng.below(3));
        lines.push(match rng.below(4) {
            0 => format!(
                "{date}. Project {}: {} logged {dur}.{filler}",
                projects[p], people[who]
            ),
            1 => format!(
                "{date}. {} spent {dur} on {} today.{filler}",
                people[who], projects[p]
            ),
            2 => format!(
                "{date}. Time entry, {}, project {}: {dur}.{filler}",
                people[who], projects[p]
            ),
            _ => format!(
                "{date}. {} worked {dur} on project {}.{filler}",
                people[who], projects[p]
            ),
        });
    }
    let preamble = "ctx holds a project tracker, one dated entry per row (YYYY-MM-DD first). \
Time entries say how long a person worked on a project, written as hours (`3 hours`, `2.5 hours`), minutes (`90 minutes`) or hours and minutes (`2h 30m`). \
`Action item AI-n opened on project P` opens an item; a later entry closing or marking AI-n done closes it and names only the id. \
`Decision (P): ...` records a decision for project P. Other entries mention projects and numbers that are neither time nor items (attendees, room numbers, estimates). \
A later entry beginning `Correction:` replaces the duration of the earlier time entry it names (same date, person and project).";
    let question = rng.below(5);
    let target_project = rng.below(n_projects as u64) as usize;
    let months_seen: Vec<usize> = {
        let mut v: Vec<usize> = log.iter().map(|e| date_of(e.0).0).collect();
        v.sort();
        v.dedup();
        v
    };
    let target_month = months_seen[rng.below(months_seen.len() as u64) as usize];
    let (kind, task, rows): (&str, String, Vec<Vec<String>>) = match question {
        0 => {
            let mut sum: HashMap<usize, i64> = HashMap::new();
            for e in &log {
                *sum.entry(e.2).or_default() += e.3;
            }
            (
                "tracker_hours_by_project",
                format!("{preamble} Answer with FINAL over one row per project: columns project (the project name) and total_hours (hours as a number, corrections applied; 90 minutes is 1.5)."),
                sum.iter()
                    .map(|(p, m)| vec![projects[*p].to_string(), hours_text(*m)])
                    .collect(),
            )
        }
        1 => {
            let mut open: HashMap<usize, i64> = HashMap::new();
            for it in items.iter().filter(|it| it.3.is_none()) {
                *open.entry(it.1).or_default() += 1;
            }
            (
                "tracker_open_items_by_project",
                format!("{preamble} At the end of the log, how many action items are still open on each project? Answer with FINAL over one row per project that has at least one open item: columns project (the project name) and open_items (an integer)."),
                open.iter()
                    .map(|(p, n)| vec![projects[*p].to_string(), n.to_string()])
                    .collect(),
            )
        }
        2 => {
            let mut count: HashMap<usize, i64> = HashMap::new();
            for (d, p) in decisions
                .iter()
                .filter(|(d, _)| date_of(*d).0 == target_month)
            {
                let _ = d;
                *count.entry(*p).or_default() += 1;
            }
            let mut best: Vec<(i64, String)> = count
                .iter()
                .map(|(p, n)| (*n, projects[*p].to_string()))
                .collect();
            best.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
            let (n, p) = best
                .first()
                .cloned()
                .unwrap_or((0, projects[0].to_string()));
            (
                "tracker_top_decisions_month",
                format!("{preamble} Which project recorded the most decisions in {} 2026? Answer with FINAL over one row: columns project (the project name) and decisions (an integer). Break a tie by the name that sorts first.", month_name(target_month)),
                vec![vec![p, n.to_string()]],
            )
        }
        3 => {
            // Pick a month in which the project has time, so the answer is
            // never empty.
            let months: Vec<usize> = {
                let mut v: Vec<usize> = log
                    .iter()
                    .filter(|e| e.2 == target_project)
                    .map(|e| date_of(e.0).0)
                    .collect();
                v.sort();
                v.dedup();
                v
            };
            let month = months
                .get(rng.below(months.len() as u64) as usize)
                .copied()
                .unwrap_or(target_month);
            let mut sum: HashMap<usize, i64> = HashMap::new();
            for e in log
                .iter()
                .filter(|e| e.2 == target_project && date_of(e.0).0 == month)
            {
                *sum.entry(e.1).or_default() += e.3;
            }
            (
                "tracker_hours_by_person_month",
                format!("{preamble} For project {} in {} 2026 only (a correction counts for the date of the entry it corrects), answer with FINAL over one row per person who logged time on it that month: columns person (the name) and total_hours (hours as a number, corrections applied).", projects[target_project], month_name(month)),
                sum.iter()
                    .map(|(w, m)| vec![people[*w].to_string(), hours_text(*m)])
                    .collect(),
            )
        }
        _ => {
            let n = items
                .iter()
                .filter(|it| it.1 == target_project)
                .filter(|it| it.3.is_some_and(|c| c - it.2 <= 7))
                .count();
            (
                "tracker_items_closed_within_week",
                format!("{preamble} How many action items opened on project {} were closed within 7 days of being opened (closing date minus opening date at most 7)? Answer with FINAL over one row with column items (an integer).", projects[target_project]),
                vec![vec![n.to_string()]],
            )
        }
    };
    let mut rows = rows;
    rows.sort();
    GeneratedTask {
        generator: "tracker".into(),
        kind: kind.into(),
        task,
        context: Some(lines.join("\n\n")),
        verify: Verify::Exact { rows },
        dial,
        difficulty: prior(dial),
    }
}

/// Finance at scale: a quarterly income statement by segment over several
/// fiscal years, one figure per row, followed by restatements that replace
/// earlier figures and by management commentary full of numbers that are
/// not figures. One segment is reported as discontinued and excluded from
/// consolidated totals. Five question shapes: a consolidated full-year
/// total, a year-on-year change in operating income, a cost ratio for one
/// segment, the segment with the highest revenue, and the quarter with the
/// highest consolidated revenue. At dial 0.9 a context is about 1,150 rows
/// and 16,000 tokens.
pub(super) fn ledger(dial: f64, rng: &mut Rng) -> GeneratedTask {
    let segment_names = ["North", "South", "Atlas", "Meridian", "Coastal", "Harbour"];
    let items = [
        "Revenue",
        "Cost of revenue",
        "Operating expenses",
        "Depreciation",
        "Interest expense",
        "Income tax",
        "Other income",
    ];
    let n_segments = 3 + (dial * 3.0).round() as usize;
    let n_years = 3 + (dial * 3.0).round() as usize;
    let first_year = 2021;
    let n_items = 5 + (dial * 2.0).round() as usize;
    let discontinued = if dial > 0.5 {
        Some(rng.below(n_segments as u64) as usize)
    } else {
        None
    };
    // figures[(year, quarter, segment, item)] = value in thousands.
    let mut figures: HashMap<(usize, usize, usize, usize), i64> = HashMap::new();
    let mut lines: Vec<String> = vec![format!(
        "Consolidated statement of operations by segment (in thousands). Fiscal years {} to {}, four quarters each; one figure per line as `FYyyyy Qn | Segment <name> | <line item>: <value>`. Restatements and notes follow the figures.",
        first_year,
        first_year + n_years - 1
    )];
    for s in 0..n_segments {
        let base = 2_000 + rng.below(7_000) as i64;
        let growth = 0.9 + 0.25 * rng.unit();
        let cost_share = 0.35 + 0.2 * rng.unit();
        let opex_share = 0.15 + 0.1 * rng.unit();
        let mut level = base as f64;
        for y in 0..n_years {
            for q in 0..4 {
                let jitter = 0.9 + 0.2 * rng.unit();
                let revenue = (level * jitter).round() as i64;
                let cost = (revenue as f64 * cost_share * (0.95 + 0.1 * rng.unit())).round() as i64;
                let opex = (revenue as f64 * opex_share * (0.95 + 0.1 * rng.unit())).round() as i64;
                let dep = (revenue as f64 * (0.02 + 0.03 * rng.unit())).round() as i64;
                let interest = (revenue as f64 * (0.01 + 0.02 * rng.unit())).round() as i64;
                let tax = (((revenue - cost - opex) as f64) * 0.2).round().max(0.0) as i64;
                let other = (revenue as f64 * 0.02 * rng.unit()).round() as i64;
                let vals = [revenue, cost, opex, dep, interest, tax, other];
                for (i, v) in vals.iter().enumerate().take(n_items) {
                    figures.insert((y, q, s, i), *v);
                }
            }
            level *= growth;
        }
    }
    let with_commas = |v: i64| -> String {
        let s = v.abs().to_string();
        let mut out = String::new();
        for (i, c) in s.chars().enumerate() {
            if i > 0 && (s.len() - i).is_multiple_of(3) {
                out.push(',');
            }
            out.push(c);
        }
        if v < 0 {
            format!("-{out}")
        } else {
            out
        }
    };
    // The body: by year, quarter, segment, item.
    for y in 0..n_years {
        for q in 0..4 {
            for s in 0..n_segments {
                for i in 0..n_items {
                    let v = figures[&(y, q, s, i)];
                    lines.push(format!(
                        "FY{} Q{} | Segment {} | {}: {}",
                        first_year + y,
                        q + 1,
                        segment_names[s],
                        items[i],
                        with_commas(v)
                    ));
                }
            }
        }
    }
    // Restatements: a share of the figures is replaced later in the text.
    let n_restated =
        ((n_years * 4 * n_segments * n_items) as f64 * (0.02 + 0.05 * dial)).round() as usize;
    for k in 0..n_restated {
        let y = rng.below(n_years as u64) as usize;
        let q = rng.below(4) as usize;
        let s = rng.below(n_segments as u64) as usize;
        let i = rng.below(n_items as u64) as usize;
        let old = figures[&(y, q, s, i)];
        let delta = (old as f64 * (0.03 + 0.12 * rng.unit())).round().max(1.0) as i64;
        let new = if rng.below(2) == 0 {
            old + delta
        } else {
            (old - delta).max(0)
        };
        figures.insert((y, q, s, i), new);
        lines.push(format!(
            "Restatement {}: FY{} Q{} | Segment {} | {} is restated to {} (previously reported as {}).",
            k + 1,
            first_year + y,
            q + 1,
            segment_names[s],
            items[i],
            with_commas(new),
            with_commas(old)
        ));
    }
    if let Some(d) = discontinued {
        lines.push(format!(
            "Note 1: Segment {} is reported as discontinued operations. Its figures are shown above for reference in every period and are excluded from all consolidated totals.",
            segment_names[d]
        ));
    }
    let commentary = [
        "Management expects revenue growth of {n}% next year, subject to market conditions.",
        "Headcount at year end was {big}, up {n}% on the prior year.",
        "The {seg} segment opened {n} new sites during the period.",
        "Capital expenditure of {big} thousand is planned for the coming year.",
        "Average contract length in the {seg} segment was {n} months.",
        "The effective tax rate is expected to remain near {n}% in future periods.",
        "Approximately {n}% of {seg} revenue is recurring.",
        "The board approved a buyback of up to {big} thousand.",
    ];
    let n_notes = 10 + (dial * 50.0).round() as usize;
    for k in 0..n_notes {
        let t = commentary[rng.below(commentary.len() as u64) as usize];
        let seg = segment_names[rng.below(n_segments as u64) as usize];
        lines.push(format!(
            "Note {}: {}",
            k + 2,
            t.replace("{n}", &(3 + rng.below(40)).to_string())
                .replace("{big}", &with_commas(1_000 + rng.below(50_000) as i64))
                .replace("{seg}", seg)
        ));
    }
    let consolidated = |y: usize, q: Option<usize>, i: usize| -> i64 {
        (0..n_segments)
            .filter(|s| Some(*s) != discontinued)
            .flat_map(|s| {
                (0..4)
                    .filter(move |qq| q.is_none_or(|q| q == *qq))
                    .map(move |qq| (s, qq))
            })
            .map(|(s, qq)| figures[&(y, qq, s, i)])
            .sum()
    };
    let preamble = format!(
        "ctx holds a quarterly income statement by segment, one figure per row as `FYyyyy Qn | Segment <name> | <line item>: <value>` (in thousands, with thousands separators), followed by numbered restatements and notes. \
A `Restatement` row replaces the figure it names (apply them in order; the last one wins); a `Note` is commentary, not a figure. \
A segment the notes mark as discontinued is excluded from every consolidated total. \
Fiscal years run {} to {}.",
        first_year,
        first_year + n_years - 1
    );
    let year = 1 + rng.below((n_years - 1) as u64) as usize;
    let fy = first_year + year;
    let seg = {
        let mut s = rng.below(n_segments as u64) as usize;
        if Some(s) == discontinued {
            s = (s + 1) % n_segments;
        }
        s
    };
    let (kind, task, verify) = match rng.below(5) {
        0 => {
            let total = consolidated(year, None, 0);
            (
                "ledger_consolidated_revenue",
                format!("{preamble} What was consolidated Revenue for the full fiscal year FY{fy}, all four quarters, restatements applied? Answer with FINAL over one row with column total (in thousands)."),
                Verify::Number {
                    value: total as f64,
                    tolerance: 0.5,
                },
            )
        }
        1 => {
            let oi = |y: usize| {
                consolidated(y, None, 0) - consolidated(y, None, 1) - consolidated(y, None, 2)
            };
            let a = oi(year - 1) as f64;
            let b = oi(year) as f64;
            let pct = (b - a) / a * 100.0;
            (
                "ledger_operating_income_growth",
                format!("{preamble} Operating income is Revenue minus Cost of revenue minus Operating expenses. By what percentage did consolidated full-year operating income change from FY{} to FY{fy}, restatements applied? Answer with FINAL over one row with column pct (a number; one decimal is fine).", fy - 1),
                Verify::Number {
                    value: pct,
                    tolerance: 0.15,
                },
            )
        }
        2 => {
            let rev: i64 = (0..4).map(|q| figures[&(year, q, seg, 0)]).sum();
            let cost: i64 = (0..4).map(|q| figures[&(year, q, seg, 1)]).sum();
            (
                "ledger_segment_cost_ratio",
                format!("{preamble} What is the ratio of Cost of revenue to Revenue for Segment {} over the full fiscal year FY{fy}, restatements applied? Answer with FINAL over one row with column ratio (two decimals).", segment_names[seg]),
                Verify::Number {
                    value: cost as f64 / rev as f64,
                    tolerance: 0.015,
                },
            )
        }
        3 => {
            let mut best: Vec<(i64, String)> = (0..n_segments)
                .filter(|s| Some(*s) != discontinued)
                .map(|s| {
                    (
                        (0..4).map(|q| figures[&(year, q, s, 0)]).sum::<i64>(),
                        segment_names[s].to_string(),
                    )
                })
                .collect();
            best.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
            (
                "ledger_top_segment",
                format!("{preamble} Which continuing segment had the highest full-year Revenue in FY{fy}, restatements applied? Answer with FINAL over one row with column segment (the segment name as written)."),
                Verify::Exact {
                    rows: vec![vec![best[0].1.clone()]],
                },
            )
        }
        _ => {
            let mut best: Vec<(i64, usize)> = (0..4)
                .map(|q| (consolidated(year, Some(q), 0), q))
                .collect();
            best.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
            (
                "ledger_top_quarter",
                format!("{preamble} Which quarter of FY{fy} had the highest consolidated Revenue, restatements applied? Answer with FINAL over one row with column quarter (Q1, Q2, Q3 or Q4)."),
                Verify::Exact {
                    rows: vec![vec![format!("Q{}", best[0].1 + 1)]],
                },
            )
        }
    };
    GeneratedTask {
        generator: "ledger".into(),
        kind: kind.into(),
        task,
        context: Some(lines.join("\n\n")),
        verify,
        dial,
        difficulty: prior(dial),
    }
}

/// The twelve clause categories of `agreement`, each with three phrasings
/// and a near miss that uses the category's vocabulary without being one.
const CATEGORIES: [(&str, [&str; 3], &str); 12] = [
    (
        "non-compete",
        [
            "During the Term and for two years after it the Supplier shall not engage in any business that competes with the Customer in the Territory.",
            "The Supplier agrees that during the Restricted Period it will not, directly or indirectly, compete with the Customer in the Territory.",
            "For twenty-four months following expiry the Supplier shall refrain from carrying on any business competitive with the Customer's.",
        ],
        "Nothing in this Agreement restricts the Supplier from competing with the Customer in any market after the Term.",
    ),
    (
        "termination for convenience",
        [
            "Either party may terminate this Agreement for any reason on ninety days' written notice to the other.",
            "This Agreement may be terminated by either party without cause by giving not less than ninety days' notice in writing.",
            "The Customer may end this Agreement at its convenience at any time on ninety days' prior notice.",
        ],
        "This Agreement may be terminated only for material breach that remains uncured thirty days after written notice.",
    ),
    (
        "cap on liability",
        [
            "In no event shall either party's aggregate liability under this Agreement exceed the fees paid in the twelve months preceding the claim.",
            "Each party's total liability arising out of this Agreement is limited to the amounts paid by the Customer in the prior year.",
            "Neither party shall be liable for amounts exceeding, in aggregate, the Fees paid under the applicable Order in the preceding twelve months.",
        ],
        "The parties acknowledge that liability for death or personal injury caused by negligence is unlimited and that nothing in this Section caps it.",
    ),
    (
        "governing law",
        [
            "This Agreement shall be governed by and construed in accordance with the laws of the State of Delaware.",
            "The laws of Delaware, without regard to its conflict of laws rules, govern this Agreement and any dispute arising from it.",
            "This Agreement and all matters arising out of it are governed by Delaware law.",
        ],
        "The parties shall attempt to resolve any dispute by negotiation between senior executives before commencing proceedings.",
    ),
    (
        "audit rights",
        [
            "The Customer may audit the Supplier's relevant records once in any twelve-month period on thirty days' notice.",
            "Upon reasonable notice the Customer shall have the right to inspect and copy the Supplier's books and records relating to the Services.",
            "The Supplier shall permit the Customer or its auditors access to records necessary to verify the Fees charged.",
        ],
        "The Supplier shall keep accurate records of the Services for six years after the Term; no inspection right is granted by this Section.",
    ),
    (
        "exclusivity",
        [
            "The Customer shall purchase the Products exclusively from the Supplier during the Term.",
            "During the Term the Supplier is the Customer's sole and exclusive source of the Products in the Territory.",
            "The Customer agrees not to procure the Products or substantially similar products from any third party during the Term.",
        ],
        "This Agreement is non-exclusive and the Customer may purchase similar products from any third party.",
    ),
    (
        "most favoured customer",
        [
            "The Supplier warrants that the Fees are no less favourable than those offered to any other customer for similar volumes.",
            "If the Supplier offers a lower price to another customer for comparable Products, the Fees shall be reduced to match.",
            "The Customer shall receive pricing at least as favourable as the Supplier's best pricing to any similarly situated customer.",
        ],
        "The Supplier may set its prices to other customers at its sole discretion and gives no assurance about them.",
    ),
    (
        "change of control",
        [
            "The Customer may terminate this Agreement on written notice if the Supplier undergoes a change of control.",
            "A change in the ownership of more than fifty percent of the Supplier's voting shares entitles the Customer to terminate.",
            "If control of the Supplier passes to a competitor of the Customer, the Customer may terminate immediately.",
        ],
        "A change in the Supplier's management team does not affect this Agreement and requires no notice.",
    ),
    (
        "non-solicitation",
        [
            "Neither party shall solicit for employment any employee of the other during the Term and for twelve months after it.",
            "During the Term and for one year thereafter the Supplier shall not hire or attempt to hire the Customer's personnel.",
            "Each party agrees not to induce any employee of the other involved in the Services to leave that employment.",
        ],
        "General advertisements not targeted at the other party's employees do not breach any obligation under this Agreement.",
    ),
    (
        "assignment restriction",
        [
            "Neither party may assign or transfer this Agreement without the prior written consent of the other.",
            "This Agreement is personal to the parties and may not be assigned, in whole or in part, without consent.",
            "Any purported assignment of this Agreement without the other party's written consent is void.",
        ],
        "The Supplier may subcontract performance of the Services provided it remains responsible for the subcontractor's acts.",
    ),
    (
        "insurance",
        [
            "The Supplier shall maintain professional indemnity insurance of not less than five million dollars per claim throughout the Term.",
            "Throughout the Term the Supplier shall hold general liability insurance with a reputable insurer in an amount of at least five million dollars.",
            "The Supplier shall carry and keep in force insurance covering its obligations under this Agreement for not less than five million dollars.",
        ],
        "The Customer acknowledges that the Supplier is not obliged to insure the Customer's equipment while on the Supplier's premises.",
    ),
    (
        "intellectual property ownership",
        [
            "All intellectual property rights in the Deliverables vest in the Customer on creation.",
            "The Customer owns all right, title and interest in the work product created under this Agreement.",
            "Deliverables are works made for hire and the Customer shall be the sole owner of the intellectual property in them.",
        ],
        "Each party retains ownership of the intellectual property it owned before the Effective Date; this Section transfers nothing.",
    ),
];

/// Legal at scale: a long agreement of a few hundred numbered sections with
/// planted clause categories (one of three phrasings each), a near miss for
/// every category, cross-references, a defined term that recurs, and
/// amendments at the end that delete a section, replace a section with a
/// clause of a new category, or touch a near miss. Three question shapes:
/// which categories are present after the amendments, which section numbers
/// hold one category, and how many sections use the defined term. At dial
/// 0.9 a context is about 350 sections and 12,000 tokens.
pub(super) fn agreement(dial: f64, rng: &mut Rng) -> GeneratedTask {
    let boilerplate = [
        "The parties agree that headings are for convenience only and do not affect interpretation. References to a Schedule are to a schedule of this Agreement.",
        "Notices shall be delivered in writing to the addresses set out in Schedule {n}. A notice is deemed received two Business Days after posting.",
        "This Agreement constitutes the entire agreement between the parties and supersedes all prior discussions on its subject matter.",
        "If any provision is held invalid, the remainder shall continue in effect. The parties shall negotiate in good faith a valid replacement.",
        "Each party shall bear its own costs in connection with the negotiation and execution of this Agreement.",
        "The Supplier shall deliver the Products in accordance with Schedule {n} and the delivery schedule agreed in each Order.",
        "Nothing in this Agreement creates a partnership, joint venture or agency between the parties.",
        "Each party shall keep the other's Confidential Information in confidence and use it only for the purposes of this Agreement.",
        "The Supplier shall provide the Services with reasonable skill and care and in accordance with Good Industry Practice.",
        "The Customer shall provide the access, information and cooperation the Supplier reasonably requires to perform the Services.",
        "Invoices are payable within {d} days of receipt. Disputed amounts shall be notified within {d} days with reasons.",
        "The Supplier shall comply with the Customer's site rules notified to it in writing when attending the Customer's premises.",
        "Each party shall comply with all applicable laws, including anti-bribery and data protection laws, in performing this Agreement.",
        "This Agreement may be executed in counterparts, each of which is an original and all of which together form one instrument.",
        "Neither party is liable for delay or failure caused by events beyond its reasonable control, provided it notifies the other promptly.",
        "The Supplier shall maintain the records described in Schedule {n} for the period required by law.",
        "Acceptance of a Deliverable occurs when the Customer confirms in writing that it meets the acceptance criteria in Schedule {n}, or {d} days after delivery if no rejection notice is given.",
        "The Supplier warrants that the Products will conform to their specification for {d} days after delivery.",
        "Confidential Information excludes information that is public, already known to the recipient, or independently developed.",
        "The recipient may disclose Confidential Information to its advisers who are bound by equivalent obligations of confidence.",
        "Any variation of this Agreement must be in writing and signed by authorised representatives of both parties.",
        "No failure or delay in exercising a right operates as a waiver of it.",
        "The Supplier shall appoint a relationship manager who shall meet the Customer's representative at least monthly.",
        "The service levels in Schedule {n} apply from the Service Commencement Date. Service credits are the Customer's sole remedy for a missed service level.",
        "Each party shall keep the terms of this Agreement confidential, except as required by law or a regulator.",
        "On expiry or termination the Supplier shall return or destroy the Customer's Confidential Information and certify that it has done so.",
        "The Supplier shall notify the Customer within {d} days of becoming aware of any security incident affecting the Customer's data.",
        "Risk in the Products passes on delivery; title passes on payment in full.",
        "Each party shall appoint a data protection contact and notify the other of any change.",
        "The Customer may request changes to the Services under the change control procedure in Schedule {n}.",
    ];
    let n_sections = 80 + (dial * 300.0).round() as usize;
    // Sections as (text, category index if planted, near-miss category if a near miss).
    let mut sections: Vec<(String, Option<usize>, Option<usize>)> = (0..n_sections)
        .map(|_| {
            let t = boilerplate[rng.below(boilerplate.len() as u64) as usize]
                .replace("{n}", &(1 + rng.below(6)).to_string())
                .replace(
                    "{d}",
                    &[10, 14, 30, 45, 60][rng.below(5) as usize].to_string(),
                );
            (t, None, None)
        })
        .collect();
    let n_present = 3 + rng.below(5) as usize;
    let mut present: Vec<usize> = vec![];
    while present.len() < n_present {
        let c = rng.below(CATEGORIES.len() as u64) as usize;
        if !present.contains(&c) {
            present.push(c);
        }
    }
    for &c in &present {
        let variant = rng.below(3) as usize;
        let pos = rng.below(sections.len() as u64) as usize;
        sections.insert(pos, (CATEGORIES[c].1[variant].to_string(), Some(c), None));
    }
    // A near miss for every category, present or not.
    for (c, (_, _, near_miss)) in CATEGORIES.iter().enumerate() {
        if rng.unit() < 0.4 + 0.6 * dial {
            let pos = rng.below(sections.len() as u64) as usize;
            sections.insert(pos, (near_miss.to_string(), None, Some(c)));
        }
    }
    // Cross-references to other sections, which mention category words
    // without being clauses.
    let n_refs = (dial * 12.0).round() as usize;
    for _ in 0..n_refs {
        let target = 1 + rng.below(sections.len() as u64) as usize;
        let t = [
            format!("Subject to Section {target}, the Supplier shall perform the Services at the times set out in each Order."),
            format!("The remedies in this Section are without prejudice to Section {target} and to any right to terminate."),
            format!("Section {target} does not apply to Orders placed before the Effective Date."),
        ][rng.below(3) as usize]
            .clone();
        let pos = rng.below(sections.len() as u64) as usize;
        sections.insert(pos, (t, None, None));
    }
    let party = format!(
        "MASTER SUPPLY AGREEMENT between {} (the \"Customer\") and {} (the \"Supplier\"), effective 1 February 2026 (the \"Effective Date\"). Sections are numbered consecutively; Amendments follow the Schedules and take effect from their date.",
        ["Alder Logistics", "Brightwater Foods", "Cobalt Analytics", "Dunmore Health"][rng.below(4) as usize],
        ["Ferrule Systems", "Greyline Services", "Halyard Software"][rng.below(3) as usize]
    );
    // Amendments: delete a planted clause, add a category by replacing a
    // boilerplate section, and (sometimes) replace a near miss.
    let original: Vec<String> = sections.iter().map(|s| s.0.clone()).collect();
    let mut amendments: Vec<String> = vec![];
    let mut amended: HashSet<usize> = HashSet::new();
    let mut n_amend = 0;
    if dial > 0.3 && !present.is_empty() {
        // Delete one planted clause: the category leaves the agreement.
        let c = present[rng.below(present.len() as u64) as usize];
        let idx = sections
            .iter()
            .position(|s| s.1 == Some(c))
            .expect("planted");
        n_amend += 1;
        amendments.push(format!(
            "Amendment No. {n_amend}, dated 1 June 2026. Section {} is deleted in its entirety and replaced with: \"{}\"",
            idx + 1,
            boilerplate[0]
        ));
        sections[idx] = (boilerplate[0].to_string(), None, None);
        amended.insert(idx);
        present.retain(|&p| p != c);
    }
    if dial > 0.3 {
        // Add a category the agreement did not have.
        let absent: Vec<usize> = (0..CATEGORIES.len())
            .filter(|c| !present.contains(c))
            .collect();
        if !absent.is_empty() {
            let c = absent[rng.below(absent.len() as u64) as usize];
            let candidates: Vec<usize> = sections
                .iter()
                .enumerate()
                .filter(|(i, s)| s.1.is_none() && s.2.is_none() && !amended.contains(i))
                .map(|(i, _)| i)
                .collect();
            let idx = candidates[rng.below(candidates.len() as u64) as usize];
            let variant = rng.below(3) as usize;
            n_amend += 1;
            amendments.push(format!(
                "Amendment No. {n_amend}, dated 15 July 2026. Section {} is deleted in its entirety and replaced with: \"{}\"",
                idx + 1,
                CATEGORIES[c].1[variant]
            ));
            sections[idx] = (CATEGORIES[c].1[variant].to_string(), Some(c), None);
            amended.insert(idx);
            present.push(c);
        }
    }
    if dial > 0.6 {
        // Replace a near miss with boilerplate: no change to the answer.
        let misses: Vec<usize> = sections
            .iter()
            .enumerate()
            .filter(|(i, s)| s.2.is_some() && !amended.contains(i))
            .map(|(i, _)| i)
            .collect();
        if !misses.is_empty() {
            let idx = misses[rng.below(misses.len() as u64) as usize];
            n_amend += 1;
            amendments.push(format!(
                "Amendment No. {n_amend}, dated 1 September 2026. Section {} is deleted in its entirety and replaced with: \"{}\"",
                idx + 1,
                boilerplate[3]
            ));
            sections[idx] = (boilerplate[3].to_string(), None, None);
            amended.insert(idx);
        }
    }
    let mut paragraphs: Vec<String> = vec![party];
    // The body as originally executed; the reader applies the amendments.
    paragraphs.extend(
        original
            .iter()
            .enumerate()
            .map(|(i, s)| format!("Section {}. {s}", i + 1)),
    );
    paragraphs.push("Schedule 1. Notice addresses. Schedule 2. Products and delivery. Schedule 3. Acceptance criteria. Schedule 4. Service levels. Schedule 5. Records. Schedule 6. Change control.".into());
    paragraphs.extend(amendments.iter().cloned());
    let preamble = "ctx holds a supply agreement, one section per row, followed by its schedules and then its amendments. \
An amendment that deletes and replaces a section puts the quoted text in that section's place; read the agreement as amended. \
A section that merely refers to a topic (a cross-reference, a carve-out, a statement that no such right is granted) is not a clause of that category.";
    let category_list = CATEGORIES
        .iter()
        .map(|(n, _, _)| n.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let (kind, task, rows): (&str, String, Vec<Vec<String>>) = match rng.below(3) {
        0 => {
            let mut rows: Vec<Vec<String>> = present
                .iter()
                .map(|&c| vec![CATEGORIES[c].0.to_string()])
                .collect();
            rows.sort();
            (
                "agreement_categories_present",
                format!("{preamble} Which of these clause categories does the agreement as amended contain: {category_list}? Answer with FINAL over one row per present category with column category (exactly the category name as written here)."),
                rows,
            )
        }
        1 => {
            let c = present[rng.below(present.len() as u64) as usize];
            let rows: Vec<Vec<String>> = sections
                .iter()
                .enumerate()
                .filter(|(_, s)| s.1 == Some(c))
                .map(|(i, _)| vec![(i + 1).to_string()])
                .collect();
            (
                "agreement_sections_of_category",
                format!("{preamble} Which section numbers of the agreement as amended contain a {} clause? Answer with FINAL over one row per section with column section (the section number as an integer).", CATEGORIES[c].0),
                rows,
            )
        }
        _ => {
            let n = sections
                .iter()
                .filter(|s| s.0.contains("Confidential Information"))
                .count();
            (
                "agreement_defined_term_count",
                format!("{preamble} In how many numbered sections of the agreement as amended does the defined term Confidential Information appear (count sections, not occurrences; the schedules and the amendments themselves are not sections)? Answer with FINAL over one row with column sections (an integer)."),
                vec![vec![n.to_string()]],
            )
        }
    };
    GeneratedTask {
        generator: "agreement".into(),
        kind: kind.into(),
        task,
        context: Some(paragraphs.join("\n\n")),
        verify: Verify::Exact { rows },
        dial,
        difficulty: prior(dial),
    }
}

#[cfg(test)]
mod tests {
    use super::super::generators::generate;
    use super::*;

    fn ctx_and_rows(g: &str, seed: u64) -> (String, GeneratedTask) {
        let t = generate(g, 0.9, seed, &std::env::temp_dir()).unwrap();
        (t.context.clone().unwrap(), t)
    }

    /// Minutes in a duration written any of the four ways.
    fn parse_minutes(s: &str) -> i64 {
        let s = s.trim_end_matches('.');
        if let Some(m) = s.strip_suffix(" minutes") {
            return m.parse().unwrap();
        }
        if let Some(h) = s.strip_suffix(" hours").or_else(|| s.strip_suffix(" hour")) {
            let v: f64 = h.parse().unwrap();
            return (v * 60.0).round() as i64;
        }
        if let Some((h, m)) = s.split_once("h ") {
            return h.parse::<i64>().unwrap() * 60
                + m.trim_end_matches('m').parse::<i64>().unwrap();
        }
        s.trim_end_matches('h').parse::<i64>().unwrap() * 60
    }

    /// Recount a tracker context the way a careful reader would, applying
    /// corrections, and compare with the generator's per-project answer.
    #[test]
    fn tracker_hours_match_a_careful_recount() {
        let mut checked = 0;
        for seed in 1..40 {
            let (ctx, t) = ctx_and_rows("tracker", seed);
            if t.kind != "tracker_hours_by_project" {
                continue;
            }
            checked += 1;
            // (date, person, project, minutes)
            let mut entries: Vec<(String, String, String, i64)> = vec![];
            for line in ctx.split("\n\n") {
                let (date, rest) = line.split_once(". ").unwrap();
                if let Some(c) = rest.strip_prefix("Correction: the ") {
                    let (d0, tail) = c.split_once(" entry for ").unwrap();
                    let (who, tail) = tail.split_once(" on project ").unwrap();
                    let (p, tail) = tail.split_once(" should read ").unwrap();
                    let (new, old) = tail.split_once(", not ").unwrap();
                    let (m1, m0) = (parse_minutes(new), parse_minutes(old));
                    let e = entries
                        .iter_mut()
                        .find(|e| e.0 == d0 && e.1 == who && e.2 == p && e.3 == m0)
                        .expect("a correction names an entry");
                    e.3 = m1;
                    continue;
                }
                let (who, p, dur) = if let Some(r) = rest.strip_prefix("Project ") {
                    // "Project P: W logged D.<filler>"
                    let (p, r) = r.split_once(": ").unwrap();
                    let Some((w, d)) = r.split_once(" logged ") else {
                        continue;
                    };
                    (w, p, d)
                } else if let Some(r) = rest.strip_prefix("Time entry, ") {
                    let (w, r) = r.split_once(", project ").unwrap();
                    let (p, d) = r.split_once(": ").unwrap();
                    (w, p, d)
                } else if rest.contains(" spent ") {
                    let (w, r) = rest.split_once(" spent ").unwrap();
                    let (d, r) = r.split_once(" on ").unwrap();
                    let (p, _) = r.split_once(" today.").unwrap();
                    (w, p, d)
                } else if rest.contains(" worked ") {
                    let (w, r) = rest.split_once(" worked ").unwrap();
                    let (d, r) = r.split_once(" on project ").unwrap();
                    let (p, _) = r.split_once('.').unwrap();
                    (w, p, d)
                } else {
                    continue;
                };
                // The duration ends at the first period after it.
                let dur = dur.split(". ").next().unwrap().trim_end_matches('.');
                entries.push((date.into(), who.into(), p.into(), parse_minutes(dur)));
            }
            let mut sum: HashMap<String, i64> = HashMap::new();
            for e in &entries {
                *sum.entry(e.2.clone()).or_default() += e.3;
            }
            let mut want: Vec<Vec<String>> = sum
                .into_iter()
                .map(|(p, m)| vec![p, hours_text(m)])
                .collect();
            want.sort();
            let Verify::Exact { rows } = &t.verify else {
                panic!()
            };
            assert_eq!(&want, rows, "seed {seed}");
            assert!(ctx.len() > 80_000, "seed {seed}: {} chars", ctx.len());
        }
        assert!(checked >= 3);
    }

    /// Replay the ledger text (figures, then restatements in order) and
    /// compare the consolidated revenue with the generator's answer.
    #[test]
    fn ledger_totals_match_the_text() {
        let mut checked = 0;
        for seed in 1..40 {
            let (ctx, t) = ctx_and_rows("ledger", seed);
            if t.kind != "ledger_consolidated_revenue" {
                continue;
            }
            checked += 1;
            let fy = t.task.split("fiscal year FY").nth(1).unwrap()[..4].to_string();
            let mut figures: HashMap<String, i64> = HashMap::new();
            let mut discontinued: Option<String> = None;
            for line in ctx.split("\n\n") {
                if let Some(r) = line.strip_prefix("Restatement ") {
                    let (_, r) = r.split_once(": ").unwrap();
                    let (key, r) = r.split_once(" is restated to ").unwrap();
                    let (v, _) = r.split_once(" (").unwrap();
                    figures.insert(key.to_string(), v.replace(',', "").parse().unwrap());
                } else if line.starts_with("FY") {
                    let (key, v) = line.rsplit_once(": ").unwrap();
                    figures.insert(key.to_string(), v.replace(',', "").parse().unwrap());
                } else if let Some(n) = line.strip_prefix("Note 1: Segment ") {
                    discontinued = Some(n.split_once(' ').unwrap().0.to_string());
                }
            }
            let total: i64 = figures
                .iter()
                .filter(|(k, _)| k.starts_with(&format!("FY{fy} ")) && k.ends_with("| Revenue"))
                .filter(|(k, _)| {
                    discontinued
                        .as_ref()
                        .is_none_or(|d| !k.contains(&format!("Segment {d} ")))
                })
                .map(|(_, v)| *v)
                .sum();
            let Verify::Number { value, .. } = t.verify else {
                panic!()
            };
            assert_eq!(total as f64, value, "seed {seed}");
        }
        assert!(checked >= 3);
    }

    /// Apply the amendments to the agreement text and find the planted
    /// phrasings; the set of categories must match the generator's answer.
    #[test]
    fn agreement_categories_match_the_amended_text() {
        let mut checked = 0;
        for seed in 1..40 {
            let (ctx, t) = ctx_and_rows("agreement", seed);
            if t.kind != "agreement_categories_present" {
                continue;
            }
            checked += 1;
            let mut sections: Vec<String> = vec![];
            let mut amendments: Vec<(usize, String)> = vec![];
            for line in ctx.split("\n\n") {
                if let Some(r) = line.strip_prefix("Section ") {
                    let (_, text) = r.split_once(". ").unwrap();
                    sections.push(text.to_string());
                } else if let Some(r) = line.strip_prefix("Amendment No. ") {
                    let (_, r) = r.split_once(". Section ").unwrap();
                    let (n, r) = r.split_once(' ').unwrap();
                    let text = r
                        .split_once("replaced with: \"")
                        .unwrap()
                        .1
                        .trim_end_matches('"');
                    amendments.push((n.parse().unwrap(), text.to_string()));
                }
            }
            assert!(!amendments.is_empty(), "seed {seed}");
            for (n, text) in amendments {
                sections[n - 1] = text;
            }
            let mut want: Vec<Vec<String>> = CATEGORIES
                .iter()
                .filter(|(_, phrasings, _)| {
                    sections.iter().any(|s| phrasings.contains(&s.as_str()))
                })
                .map(|(name, _, _)| vec![name.to_string()])
                .collect();
            want.sort();
            let Verify::Exact { rows } = &t.verify else {
                panic!()
            };
            assert_eq!(&want, rows, "seed {seed}");
        }
        assert!(checked >= 3);
    }
}
