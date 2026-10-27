//! `nades-sandbox check <file>...`: each cs2-throws file's problems, and what the sandbox can do with it, feature by
//! feature (docs/standalone-api.md §2): what works, what is off, and which fact is missing.

use std::path::Path;

use nades_model::features::{self, Status};
use nades_source_file::{Report, check};

/// Checks each file and prints what it found; false when any file has errors or does not open.
pub fn run(files: &[String]) -> bool {
    if files.is_empty() {
        eprintln!("usage: nades-sandbox check <cs2-throws file>...");
        return false;
    }
    let mut clean = true;
    for (i, file) in files.iter().enumerate() {
        if i > 0 {
            println!();
        }
        match check(Path::new(file)) {
            Ok(report) => {
                clean &= report.errors.is_empty();
                print!("{}", describe(file, &report));
            }
            Err(e) => {
                clean = false;
                println!("{file}: {e}");
            }
        }
    }
    clean
}

/// The report as `check` prints it.
pub fn describe(file: &str, report: &Report) -> String {
    let meta = |key: &str| report.meta.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
    let mut out = format!("{file}: {} {}", meta("format").unwrap_or("?"), meta("version").unwrap_or("?"));
    let about: Vec<String> = [("producer", meta("producer")), ("created", meta("created"))].into_iter().filter_map(|(k, v)| Some(format!("{k} {}", v?))).collect();
    if !about.is_empty() {
        out += &format!(" ({})", about.join(", "));
    }
    let c = &report.census;
    out += &format!("\n  {}, {}, {}\n", features::count(c.demos, "demos"), features::count(c.throws, "throws"), features::count(c.samples, "player samples"));
    if report.errors.is_empty() && report.warnings.is_empty() {
        out += "  no problems\n";
    }
    for e in &report.errors {
        out += &format!("  error: {e}\n");
    }
    for w in &report.warnings {
        out += &format!("  warning: {w}\n");
    }
    out += "\n";
    for f in features::table(c) {
        let status = match f.status {
            Status::Partly { .. } => "partly",
            Status::On => "on",
            Status::Off => "off",
            Status::NotApplicable => "n/a",
        };
        let counts = match f.status {
            Status::Partly { with, of } => format!(" ({with} of {of})"),
            _ => String::new(),
        };
        out += &format!("  {status:<7} {}{counts}\n", f.name);
        let note = f.note.map(|n| format!(" · {n}")).unwrap_or_default();
        out += &format!("          needs {}{note}\n", f.needs);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use nades_model::features::Census;

    #[test]
    fn the_report_names_the_file_its_problems_and_each_feature() {
        let report = Report {
            meta: vec![("format".into(), "cs2-throws".into()), ("version".into(), "1.0".into()), ("producer".into(), "a parser".into())],
            errors: vec!["2 throws with a side other than 'T' or 'CT'".into()],
            warnings: Vec::new(),
            census: Census { demos: 1, throws: 4, landed: 4, with_flight: 1, ..Default::default() },
        };
        let text = describe("x.sqlite", &report);
        assert!(text.starts_with("x.sqlite: cs2-throws 1.0 (producer a parser)\n  1 demo, 4 throws, 0 player samples\n  error: 2 throws"), "{text}");
        assert!(text.contains("  partly  the drawn trajectory (1 of 4)\n          needs the flight · 3 throws without a flight\n"), "{text}");
        assert!(text.contains("  off     technique (movement, click)\n"), "{text}");
    }
}
