#!/bin/env bash

set -e
cargo build
../target/debug/skidder-cli import --metadata -r ../../../tree-sitter-grammars/ ../../../master/runtime/grammars/sources/*
../target/debug/skidder-cli import --metadata -r ../../../tree-sitter-grammars ../../../master/runtime/grammars/sources/markdown/tree-sitter-markdown:markdown
../target/debug/skidder-cli import --metadata -r ../../../tree-sitter-grammars ../../../master/runtime/grammars/sources/markdown/tree-sitter-markdown-inline:markdown-inline
../target/debug/skidder-cli import --metadata -r ../../../tree-sitter-grammars ../../../master/runtime/grammars/sources/v/tree_sitter_v:v
../target/debug/skidder-cli import --metadata -r ../../../tree-sitter-grammars ../../../master/runtime/grammars/sources/wat/wat
../target/debug/skidder-cli import --metadata -r ../../../tree-sitter-grammars ../../../master/runtime/grammars/sources/wat/wast
../target/debug/skidder-cli import --metadata -r ../../../tree-sitter-grammars ../../../master/runtime/grammars/sources/typescript/typescript
../target/debug/skidder-cli import --metadata -r ../../../tree-sitter-grammars ../../../master/runtime/grammars/sources/typescript/tsx
../target/debug/skidder-cli import --metadata -r ../../../tree-sitter-grammars ../../../master/runtime/grammars/sources/php_only/php_only
../target/debug/skidder-cli import --metadata -r ../../../tree-sitter-grammars ../../../master/runtime/grammars/sources/php-only/php_only:php-only
../target/debug/skidder-cli import --metadata -r ../../../tree-sitter-grammars ../../../master/runtime/grammars/sources/ocaml/ocaml
../target/debug/skidder-cli import --metadata -r ../../../tree-sitter-grammars ../../../master/runtime/grammars/sources/ocaml/interface:ocaml-interface
