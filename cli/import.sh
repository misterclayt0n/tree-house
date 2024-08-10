#!/bin/env bash

set -e
cargo build --release
../target/release/skidder-cli import --metadata -r ../../../tree-sitter-grammars/ ../../../master/runtime/grammars/sources/*
../target/release/skidder-cli import --metadata -r ../../../tree-sitter-grammars ../../../master/runtime/grammars/sources/markdown/tree-sitter-markdown:markdown
../target/release/skidder-cli import --metadata -r ../../../tree-sitter-grammars ../../../master/runtime/grammars/sources/markdown/tree-sitter-markdown-inline:markdown.inline
../target/release/skidder-cli import --metadata -r ../../../tree-sitter-grammars ../../../master/runtime/grammars/sources/v/tree_sitter_v:v
../target/release/skidder-cli import --metadata -r ../../../tree-sitter-grammars ../../../master/runtime/grammars/sources/wat/wat
../target/release/skidder-cli import --metadata -r ../../../tree-sitter-grammars ../../../master/runtime/grammars/sources/wat/wast
../target/release/skidder-cli import --metadata -r ../../../tree-sitter-grammars ../../../master/runtime/grammars/sources/typescript/typescript
../target/release/skidder-cli import --metadata -r ../../../tree-sitter-grammars ../../../master/runtime/grammars/sources/typescript/tsx
../target/release/skidder-cli import --metadata -r ../../../tree-sitter-grammars ../../../master/runtime/grammars/sources/php_only/php_only
../target/release/skidder-cli import --metadata -r ../../../tree-sitter-grammars ../../../master/runtime/grammars/sources/php-only/php_only:php-only
../target/release/skidder-cli import --metadata -r ../../../tree-sitter-grammars ../../../master/runtime/grammars/sources/ocaml/ocaml
../target/release/skidder-cli import --metadata -r ../../../tree-sitter-grammars ../../../master/runtime/grammars/sources/ocaml/interface:ocaml-interface
