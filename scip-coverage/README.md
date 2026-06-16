# scip-coverage

Reports source files that no SCIP indexer covered, tallied by extension, so file
types with no indexer become visible across a whole indexing run.

For one package it takes the source tree and the `*.scip` indexes written for
it, finds files present in the tree but in none of the indexes, and groups them
by extension (extensionless files share one bucket). The counts are merged into
an aggregate TSV (`extension<TAB>files<TAB>packages`) that accumulates across the
run.

## Usage

```
scip-coverage \
    --root path/to/source \
    --scip foo.scip bar.scip \
    --aggregate uncovered.tsv
```

- `--root` - the source tree that was indexed.
- `--scip` - SCIP indexes written for the tree; any document they cover counts
  as handled. Accepts several paths and may be repeated. Indexes that do not
  exist are skipped, so a package may legitimately lack a given index.
- `--aggregate` - the TSV to merge this package's counts into, created if
  absent. File counts are summed and the package count is bumped once per bucket
  the package contributed to.

On each run it also prints a short per-package summary of the uncovered buckets,
most files first.
