//! Report source files that no SCIP indexer covered, tallied by extension. See
//! the README for what it does and how to run it.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Parser as ClapParser;
use scip::types::Index;

/// Directory names whose contents are build output or vendored dependencies,
/// not source. Kept in sync with scip-tree-sitter so the two agree on what the
/// source tree is.
const SKIP_DIRS: &[&str] = &["target", "node_modules", ".git", "vendor"];

/// The extension bucket used for files that have no extension.
const NO_EXTENSION: &str = "<none>";

#[derive(ClapParser)]
#[command(about = "Report source files no SCIP indexer covered, tallied by extension.")]
struct Args {
    /// Source tree that was indexed.
    #[arg(long)]
    root: PathBuf,
    /// SCIP indexes written for this source tree. Any document they cover counts
    /// as handled. Accepts several paths and may be repeated.
    #[arg(long = "scip", value_name = "FILE", num_args = 1.., action = clap::ArgAction::Append)]
    scip: Vec<PathBuf>,
    /// Aggregate TSV to merge this package's counts into. Created if absent.
    #[arg(long)]
    aggregate: PathBuf,
}

/// Collect the set of document paths covered by the given SCIP indexes.
fn covered_paths(scip_files: &[PathBuf]) -> Result<HashSet<String>> {
    let mut covered = HashSet::new();
    for file in scip_files {
        // A package may legitimately lack a given index (e.g. no debian.scip on
        // a non-Debian tree); skip indexes that are not present.
        if !file.exists() {
            continue;
        }
        let bytes = std::fs::read(file).with_context(|| format!("reading {}", file.display()))?;
        let index = <Index as protobuf::Message>::parse_from_bytes(&bytes)
            .with_context(|| format!("parsing {}", file.display()))?;
        for doc in &index.documents {
            covered.insert(doc.relative_path.clone());
        }
    }
    Ok(covered)
}

/// The extension bucket for a path: its lowercased extension, or `<none>` for an
/// extensionless file.
fn extension_bucket(path: &Path) -> String {
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => ext.to_lowercase(),
        None => NO_EXTENSION.to_string(),
    }
}

/// Count uncovered files in `root` by extension bucket.
fn uncovered_by_extension(root: &Path, covered: &HashSet<String>) -> Result<BTreeMap<String, u64>> {
    let mut counts: BTreeMap<String, u64> = BTreeMap::new();
    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .filter_entry(|entry| {
            !(entry.file_type().is_some_and(|t| t.is_dir())
                && entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| SKIP_DIRS.contains(&name)))
        })
        .build();
    for entry in walker {
        let entry = entry?;
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path();
        let relative = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .into_owned();
        if covered.contains(&relative) {
            continue;
        }
        *counts.entry(extension_bucket(path)).or_default() += 1;
    }
    Ok(counts)
}

/// Read an aggregate TSV (`bucket\tfiles\tpackages`) into a map. Missing or
/// malformed lines are skipped so a partial file does not abort the run.
fn read_aggregate(path: &Path) -> Result<BTreeMap<String, (u64, u64)>> {
    let mut totals = BTreeMap::new();
    let Ok(text) = std::fs::read_to_string(path) else {
        return Ok(totals);
    };
    for line in text.lines() {
        if line.is_empty() || line.starts_with("extension\t") {
            continue;
        }
        let mut fields = line.split('\t');
        let (Some(bucket), Some(files), Some(packages)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        if let (Ok(files), Ok(packages)) = (files.parse::<u64>(), packages.parse::<u64>()) {
            totals.insert(bucket.to_string(), (files, packages));
        }
    }
    Ok(totals)
}

/// Write the aggregate map back as a TSV sorted by bucket.
fn write_aggregate(path: &Path, totals: &BTreeMap<String, (u64, u64)>) -> Result<()> {
    let mut out = String::from("extension\tfiles\tpackages\n");
    for (bucket, (files, packages)) in totals {
        out.push_str(&format!("{bucket}\t{files}\t{packages}\n"));
    }
    std::fs::write(path, out).with_context(|| format!("writing {}", path.display()))
}

fn main() -> Result<()> {
    let args = Args::parse();

    let covered = covered_paths(&args.scip)?;
    let counts = uncovered_by_extension(&args.root, &covered)?;

    // Merge this package's counts into the aggregate: sum file counts and bump
    // the package count once per bucket this package contributed to.
    let mut totals = read_aggregate(&args.aggregate)?;
    for (bucket, files) in &counts {
        let entry = totals.entry(bucket.clone()).or_insert((0, 0));
        entry.0 += files;
        entry.1 += 1;
    }
    write_aggregate(&args.aggregate, &totals)?;

    // A short per-package line for the log: the buckets this package left
    // uncovered, most files first.
    if counts.is_empty() {
        eprintln!("scip-coverage: all files covered by an indexer");
    } else {
        let mut summary: Vec<(&String, &u64)> = counts.iter().collect();
        summary.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        let rendered: Vec<String> = summary
            .iter()
            .map(|(bucket, files)| format!("{bucket} ({files})"))
            .collect();
        eprintln!("scip-coverage: uncovered: {}", rendered.join(", "));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_bucket_handles_extensionless_and_case() {
        assert_eq!(extension_bucket(Path::new("a/b.RB")), "rb");
        assert_eq!(extension_bucket(Path::new("a/b.go")), "go");
        assert_eq!(extension_bucket(Path::new("a/Makefile")), NO_EXTENSION);
    }

    #[test]
    fn aggregate_round_trips() {
        let dir = std::env::temp_dir().join(format!("scip-cov-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("agg.tsv");

        let mut totals = BTreeMap::new();
        totals.insert("rb".to_string(), (12u64, 3u64));
        totals.insert(NO_EXTENSION.to_string(), (5u64, 2u64));
        write_aggregate(&path, &totals).unwrap();

        let read = read_aggregate(&path).unwrap();
        assert_eq!(read, totals);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_aggregate_reads_empty() {
        let read = read_aggregate(Path::new("/nonexistent/scip-coverage/agg.tsv")).unwrap();
        assert!(read.is_empty());
    }
}
