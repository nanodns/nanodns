#!/bin/bash
# Quick AST-level check without network (rustfmt parses the whole crate)
for f in $(find src -name "*.rs"); do
  rustfmt --check "$f" 2>/dev/null && echo "OK: $f" || echo "SYNTAX: $f"
done
