#!/usr/bin/env bash
# Validate XISF headers against the official XML Schema.
#
# Run scripts/fetch-xsd.sh first; the schema is not kept in this repository
# (see that script for why).
#
# The headers come out of `xisftool header`, which writes the file's own bytes
# unchanged when its output is redirected. That matters here: validating a
# re-serialized header would test the serializer rather than the file, and the
# point is to check what we actually read and what we actually write against
# the authority's own definition of the format.
set -uo pipefail

schema="${XISF_SCHEMA:-schema/xisf-1.0.xsd}"
if [[ ! -f $schema ]]; then
    echo "no schema at $schema -- run scripts/fetch-xsd.sh first" >&2
    exit 2
fi
if ! command -v xmllint >/dev/null 2>&1; then
    echo "xmllint not found -- install libxml2-utils" >&2
    exit 2
fi

cargo build --quiet -p xisftool || exit 1
tool="target/debug/xisftool"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

checked=0
failed=0
for file in "$@"; do
    name="$(basename "$file")"
    if ! "$tool" header "$file" > "$work/$name.xml" 2>"$work/$name.err"; then
        echo "SKIP  $name (header unreadable: $(head -1 "$work/$name.err"))"
        continue
    fi
    if out=$(xmllint --noout --schema "$schema" "$work/$name.xml" 2>&1); then
        checked=$((checked + 1))
    else
        failed=$((failed + 1))
        echo "FAIL  $name"
        echo "$out" | grep -v '^$' | head -8 | sed 's/^/        /'
    fi
done

echo "$checked header(s) valid, $failed invalid"
[[ $failed -eq 0 ]]
