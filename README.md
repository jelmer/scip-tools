# scip-tools

A small collection of [SCIP](https://github.com/sourcegraph/scip) utilities that
fill gaps left by language indexers.

- [scip-coverage](scip-coverage/README.md) - reports source files that no SCIP
  indexer covered, tallied by extension, so file types with no indexer become
  visible across a whole indexing run.
- [scip-tree-sitter](scip-tree-sitter/README.md) - generates a SCIP index of
  syntax-highlighting tokens (no navigation) for files tree-sitter can parse but
  no language indexer covers.

## License

Apache-2.0.
