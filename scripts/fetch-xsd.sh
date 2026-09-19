#!/usr/bin/env bash
# Fetch the official XISF 1.0 XML Schema.
#
# The schema is not kept in this repository, and that is deliberate. It carries
# "Copyright (c) 2014-2026 Pleiades Astrophoto S.L. All rights reserved." and
# grants no redistribution permission anywhere in the file. This project is MIT
# and has been careful about provenance -- nothing here derives from libXISF or
# the PixInsight Class Library -- so committing an all-rights-reserved file, and
# then shipping it inside four crates on crates.io, is not a thing to do on the
# strength of an assumption. Fetching it is not redistributing it.
#
# The destination is in .gitignore for the same reason. Run this before
# `validate-headers.sh`, in CI or locally.
set -euo pipefail

url="https://pixinsight.com/xisf/xisf-1.0.xsd"
dest="${1:-schema/xisf-1.0.xsd}"

# The server refuses a request without a browser-ish User-Agent with 406.
mkdir -p "$(dirname "$dest")"
curl --fail --silent --show-error --location \
     --user-agent "Mozilla/5.0 (X11; Linux x86_64)" \
     --output "$dest" "$url"

# Report the digest rather than pinning one. The schema is the authority's
# living document: pinning would turn "they corrected the schema" into a build
# failure that looks like our bug, while printing it means a change is visible
# in the log and can be pinned deliberately if that is ever wanted.
if command -v sha256sum >/dev/null 2>&1; then
    printf 'fetched %s\n  sha256 %s\n' "$dest" "$(sha256sum < "$dest" | cut -d' ' -f1)"
else
    printf 'fetched %s\n' "$dest"
fi
