//! Every sample's sites, read off its runs beside what today's graph did there.

use std::collections::BTreeMap;
use std::fmt::Write as _;


use super::*;
use crate::samples::samples;

/// The census of step C1: every sample's sites on every run, what their paths give beside what
/// today's graph did there. Watching the lowering changes nothing it gives; and no sample's site
/// is made where its path says it isn't reached, or missed where it is and has what it needs.
/// Printed with `--nocapture`: the count of each verdict, and each site run that isn't plainly
/// agreeing.
///
/// **If it fails:** a sample's site differs from its path. Read the line it names: either the
/// Reader's path for it is wrong (fix the path, in `lang/ts.rs`), or the graph does other than
/// JavaScript there, which is a new hole: a named row in `holon`'s `tests/reached.rs`, never a
/// patch here.
#[test]
fn every_samples_sites_are_read_off_its_runs() {
    let mut report = String::new();
    let mut differs = Vec::new();
    let mut tally: BTreeMap<String, usize> = BTreeMap::new();
    let mut sites_all = 0;
    for s in samples() {
        let w = Watched::of(s.text).unwrap();
        let plain = ts::compile(s.text).unwrap();
        assert!(plain.doc == w.lowered.doc, "{}: watching the lowering changed what it gives", s.name);
        sites_all += w.lowered.sites.len();
        for r in site_runs(&w, &given, answer_ok) {
            let site = &w.lowered.sites[r.site];
            let verdict = r.verdict(site);
            *tally.entry(format!("{verdict:?}")).or_default() += 1;
            if !matches!(verdict, Verdict::Agrees | Verdict::NotCompared | Verdict::NothingToGoOn) {
                let line = s.text.lines().nth(site.at.start.line as usize - 1).unwrap_or("").trim();
                let said = format!("{} | {} | input {} | {} at line {} | {verdict:?} {:?} values {:?} | {line}", s.name, r.handler, r.input, site.does, site.at.start.line, r.reached, r.has_values);
                let _ = writeln!(report, "  {said}");
                if matches!(verdict, Verdict::MadeUnreached | Verdict::MissedReached | Verdict::MadeUnlooked) {
                    differs.push(said);
                }
            }
        }
    }
    println!("sites: {sites_all}");
    for (cell, n) in &tally {
        println!("  {n:5} {cell}");
    }
    println!("not plainly agreeing:
{report}");
    assert!(differs.is_empty(), "sample sites that differ from their paths:
{}", differs.join("
"));
}
