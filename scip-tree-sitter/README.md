# scip-tree-sitter

Generates a SCIP index carrying only syntax-highlighting tokens (no navigation)
for files tree-sitter can parse but no language indexer covers: config formats,
markup, and a few scripting languages.

Each emitted occurrence has a `syntax_kind` and an empty `symbol`, so it
highlights but offers no navigation. Positions are UTF-16 code-unit offsets,
matching the encoding the renderer assumes (and recorded in the index metadata).

## Usage

```
scip-tree-sitter \
    --root path/to/source \
    --output tree-sitter.scip \
    --exclude-scip foo.scip bar.scip
```

- `--root` - the source tree to walk.
- `--output` - the `.scip` path to write.
- `--exclude-scip` - existing indexes to defer to: any document they already
  cover is skipped, so a real language indexer's richer tokens win over the
  syntax-only ones. Accepts several paths per flag and may be repeated.

## Grammars

Config and markup: TOML, JSON, YAML, Make, INI (including freedesktop `.desktop`
entries and systemd unit files), Nix, XML, CSS, HTML, and Markdown.

Languages: C, C++, Python, Bash, Java, JavaScript, TypeScript, Ruby, Rust, Go,
PHP, C#, Lua, Haskell, OCaml, Scheme, Fortran, R, Fish, and Vimscript.

Many of these languages also have dedicated SCIP indexers that produce richer,
navigable tokens. Where one runs, pass its output via `--exclude-scip` so its
documents win and tree-sitter only fills the gaps it leaves.

Files are matched by extension and by well-known filenames (`Makefile`,
`Cargo.lock`, ...). Extensionless files are matched by their `#!` line for the
interpreters of covered grammars.

Markdown uses the block grammar only, so headings, code fences, and list/quote
markers highlight, but inline spans (emphasis, links) render plain.
