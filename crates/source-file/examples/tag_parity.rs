//! The sandbox's derivations against a producer's own labels for them, counted: its groups against `<ns>.lineup`, its
//! techniques against `<ns>.technique`, its landing callouts against `<ns>.land_place` (show-me-the-nades' export writes
//! them as `nades.*`). Tags are never read by the sandbox itself; this is how its ported rules are checked against the
//! results they were ported from.
//!
//!   cargo run --release -p nades-source-file --example tag_parity -- <cs2-throws file>... [--ns nades] [--json out.json]
//!                                                                    [--dump throws.csv]
//!
//! `--dump` writes every throw's id, map, start, landing, group and tags, for looking into a difference.
//!
//! A group agrees when it and one tag value name exactly the same throws.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::Instant;

use nades_model::{Library, ThrowSource, tag};
use nades_source_file::FileSource;

#[derive(Default, Debug, Clone, Copy)]
struct Tally {
    same: u64,
    both_missing: u64,
    differ: u64,
    ours_only: u64,
    theirs_only: u64,
}

impl Tally {
    fn add(&mut self, ours: Option<&str>, theirs: Option<&str>) -> bool {
        match (ours, theirs) {
            (Some(a), Some(b)) if a == b => self.same += 1,
            (None, None) => self.both_missing += 1,
            (Some(_), Some(_)) => {
                self.differ += 1;
                return false;
            }
            (Some(_), None) => {
                self.ours_only += 1;
                return false;
            }
            (None, Some(_)) => {
                self.theirs_only += 1;
                return false;
            }
        }
        true
    }

    fn json(&self) -> serde_like::Value {
        serde_like::object(&[("same", self.same), ("both_missing", self.both_missing), ("differ", self.differ), ("ours_only", self.ours_only), ("theirs_only", self.theirs_only)])
    }

    fn merge(&mut self, o: &Tally) {
        self.same += o.same;
        self.both_missing += o.both_missing;
        self.differ += o.differ;
        self.ours_only += o.ours_only;
        self.theirs_only += o.theirs_only;
    }
}

/// A few lines of JSON without a dependency: numbers, strings, arrays and objects.
mod serde_like {
    pub type Value = String;
    pub fn object(fields: &[(&str, u64)]) -> Value {
        format!("{{{}}}", fields.iter().map(|(k, v)| format!("\"{k}\":{v}")).collect::<Vec<_>>().join(","))
    }
    pub fn string(s: &str) -> Value {
        format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
    }
}

#[derive(Default, Debug, Clone, Copy)]
struct Groups {
    ours: u64,
    theirs: u64,
    agreeing: u64,
    throws_agreeing: u64,
    throws_disagreeing: u64,
    grouped_untagged: u64,
    tagged_ungrouped: u64,
}

fn main() {
    let mut files: Vec<PathBuf> = Vec::new();
    let (mut ns, mut json, mut dump) = ("nades".to_string(), None::<PathBuf>, None::<PathBuf>);
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--ns" => ns = args.next().expect("--ns needs a namespace"),
            "--json" => json = Some(args.next().expect("--json needs a file").into()),
            "--dump" => dump = Some(args.next().expect("--dump needs a file").into()),
            _ => files.push(arg.into()),
        }
    }
    assert!(!files.is_empty(), "usage: tag_parity <cs2-throws file>... [--ns nades] [--json out.json]");
    let (lineup_key, technique_key, callout_key) = (format!("{ns}.lineup"), format!("{ns}.technique"), format!("{ns}.land_place"));
    let source = FileSource::open(&files).unwrap_or_else(|e| panic!("{e}"));
    let (mut techniques, mut callouts, mut groups) = (Tally::default(), Tally::default(), Groups::default());
    let mut maps_json = Vec::new();
    let mut examples: Vec<String> = Vec::new();
    let mut rows: Vec<String> = vec!["throw_id,map,grenade,order,start_x,start_y,start_z,land_x,land_y,land_z,group,lineup_tag,technique,technique_tag,callout,callout_tag".into()];
    for map in source.maps().unwrap() {
        let started = Instant::now();
        let facts = source.map(&map).unwrap();
        let places = source.places(&map).unwrap();
        let read_ms = started.elapsed().as_secs_f64() * 1e3;
        let built = Instant::now();
        let library = Library::new(facts, places);
        let build_ms = built.elapsed().as_secs_f64() * 1e3;
        // a filter change: the slice's lineups and the filters' choices, in memory
        let listed = Instant::now();
        let shown = library.lineups(&nades_model::LineupQuery::default()).len();
        let _ = library.facets(None);
        let list_ms = listed.elapsed().as_secs_f64() * 1e3;
        let (mut t, mut c, mut g) = (Tally::default(), Tally::default(), Groups::default());
        let mut by_group: BTreeMap<u32, BTreeSet<&str>> = BTreeMap::new();
        let mut by_tag: BTreeMap<&str, BTreeSet<u32>> = BTreeMap::new();
        let mut members: BTreeMap<u32, Vec<&str>> = BTreeMap::new();
        for (order, (throw, d)) in library.throws().enumerate() {
            let technique = d.technique.map(|x| x.name());
            if dump.is_some() {
                let field = |v: Option<&str>| v.map_or(String::new(), |v| format!("\"{}\"", v.replace('"', "\"\"")));
                let land = d.landing.map_or(",,".into(), |l| format!("{},{},{}", l[0], l[1], l[2]));
                rows.push(format!(
                    "{},{},{},{order},{},{},{},{land},{},{},{},{},{},{}",
                    throw.id,
                    map,
                    throw.grenade,
                    throw.start[0],
                    throw.start[1],
                    throw.start[2],
                    d.lineup.map_or(String::new(), |l| l.0.to_string()),
                    field(tag(&throw.tags, &lineup_key)),
                    field(technique.as_deref()),
                    field(tag(&throw.tags, &technique_key)),
                    field(d.to.as_deref()),
                    field(tag(&throw.tags, &callout_key))
                ));
            }
            if !t.add(technique.as_deref(), tag(&throw.tags, &technique_key)) && examples.len() < 40 {
                examples.push(format!("technique {} {}: ours {:?}, theirs {:?}", map, throw.id, technique, tag(&throw.tags, &technique_key)));
            }
            if !c.add(d.to.as_deref(), tag(&throw.tags, &callout_key)) && examples.len() < 40 {
                examples.push(format!("callout {} {}: ours {:?}, theirs {:?} at {:?}", map, throw.id, d.to, tag(&throw.tags, &callout_key), d.landing));
            }
            match (d.lineup, tag(&throw.tags, &lineup_key)) {
                (Some(ours), Some(theirs)) => {
                    by_group.entry(ours.0).or_default().insert(theirs);
                    by_tag.entry(theirs).or_default().insert(ours.0);
                    members.entry(ours.0).or_default().push(throw.id.as_str());
                }
                (Some(_), None) => g.grouped_untagged += 1,
                (None, Some(_)) => g.tagged_ungrouped += 1,
                (None, None) => {}
            }
        }
        g.ours = by_group.len() as u64;
        g.theirs = by_tag.len() as u64;
        for (group, tags) in &by_group {
            let n = members[group].len() as u64;
            let agrees = tags.len() == 1 && by_tag[tags.iter().next().unwrap()].len() == 1;
            if agrees {
                g.agreeing += 1;
                g.throws_agreeing += n;
            } else {
                g.throws_disagreeing += n;
                if examples.len() < 40 {
                    let split: Vec<String> = tags.iter().map(|tg| format!("{tg} (also in our groups {:?})", by_tag[tg].iter().filter(|x| *x != group).collect::<Vec<_>>())).collect();
                    examples.push(format!("group {map} #{group} ({n} throws, first {}): their lineups {}", members[group][0], split.join(", ")));
                }
            }
        }
        println!(
            "{map}: {} throws in {} demos ({:.0} ms read, {:.0} ms rules, {} callout cells, {shown} lineups listed in {list_ms:.1} ms) · groups: {} of ours, {} of theirs, {} agree ({} throws), {} throws in groups that do not · technique: {:?} · callout: {:?}",
            library.throws().count(),
            library.demos().len(),
            read_ms,
            build_ms,
            library.callout_cells(),
            g.ours,
            g.theirs,
            g.agreeing,
            g.throws_agreeing,
            g.throws_disagreeing,
            t,
            c
        );
        maps_json.push(format!(
            "{{\"map\":{},\"throws\":{},\"read_ms\":{read_ms:.1},\"rules_ms\":{build_ms:.1},\"list_ms\":{list_ms:.2},\"groups\":{},\"technique\":{},\"callout\":{}}}",
            serde_like::string(&map.0),
            library.throws().count(),
            serde_like::object(&[("ours", g.ours), ("theirs", g.theirs), ("agreeing", g.agreeing), ("throws_agreeing", g.throws_agreeing), ("throws_disagreeing", g.throws_disagreeing), ("grouped_untagged", g.grouped_untagged), ("tagged_ungrouped", g.tagged_ungrouped)]),
            t.json(),
            c.json()
        ));
        techniques.merge(&t);
        callouts.merge(&c);
        for (total, part) in [(&mut groups.ours, g.ours), (&mut groups.theirs, g.theirs), (&mut groups.agreeing, g.agreeing), (&mut groups.throws_agreeing, g.throws_agreeing), (&mut groups.throws_disagreeing, g.throws_disagreeing), (&mut groups.grouped_untagged, g.grouped_untagged), (&mut groups.tagged_ungrouped, g.tagged_ungrouped)] {
            *total += part;
        }
    }
    println!("\nall maps · groups: {groups:?}\n          technique: {techniques:?}\n          callout: {callouts:?}");
    if !examples.is_empty() {
        println!("\nfirst differences:");
        for e in &examples {
            println!("  {e}");
        }
    }
    if let Some(path) = dump {
        std::fs::write(&path, rows.join("\n") + "\n").unwrap();
        println!("\n{}", path.display());
    }
    if let Some(path) = json {
        let body = format!(
            "{{\"files\":[{}],\"namespace\":{},\"maps\":[{}],\"technique\":{},\"callout\":{},\"groups\":{},\"examples\":[{}]}}\n",
            files.iter().map(|f| serde_like::string(&f.display().to_string())).collect::<Vec<_>>().join(","),
            serde_like::string(&ns),
            maps_json.join(","),
            techniques.json(),
            callouts.json(),
            serde_like::object(&[("ours", groups.ours), ("theirs", groups.theirs), ("agreeing", groups.agreeing), ("throws_agreeing", groups.throws_agreeing), ("throws_disagreeing", groups.throws_disagreeing), ("grouped_untagged", groups.grouped_untagged), ("tagged_ungrouped", groups.tagged_ungrouped)]),
            examples.iter().map(|e| serde_like::string(e)).collect::<Vec<_>>().join(",")
        );
        std::fs::write(&path, body).unwrap();
        println!("\n{}", path.display());
    }
}
