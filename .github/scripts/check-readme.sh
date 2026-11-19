#!/usr/bin/env bash
# The README describes the release being cut (SPEC-release.md 4.5): the
# version in its status line, the first line of the `generated:status`
# block, and in every download link of its Install section is the tag's,
# without the `v` and without a pre-release suffix such as `-rc1`.
# `cargo xtask readme` writes both in the release pull request. Run by the
# release workflow in its first job, before anything is built.
#
#   check-readme.sh <tag> [<readme>]
set -euo pipefail

tag="${1:?usage: check-readme.sh <tag> [<readme>]}"
readme="${2:-README.md}"
version="${tag#v}"
version="${version%%-*}"

again="Run \`cargo xtask readme\` in the release pull request."

line="$(sed -n '/^<!-- generated:status -->$/{n;p;q;}' "$readme")"
case "$line" in
  "v$version · "*) ;;
  *)
    echo "$readme describes another release than $tag: its status line is"
    echo "  ${line:-(missing)}"
    echo "$again"
    exit 1
    ;;
esac

links="$(grep -o 'releases/download/v[^/]*/gossip-[^)" ]*' "$readme" || true)"
if [ -z "$links" ]; then
  echo "$readme has no link to an archive of the release in its Install section"
  echo "$again"
  exit 1
fi
wrong=0
while IFS= read -r link; do
  case "$link" in
    "releases/download/v$version/gossip-$version-"*) ;;
    *)
      echo "$readme links to an archive of another release than $tag: $link"
      wrong=1
      ;;
  esac
done <<< "$links"
if [ "$wrong" -ne 0 ]; then
  echo "$again"
  exit 1
fi
echo "$readme describes v$version: its status line and $(printf '%s\n' "$links" | wc -l | tr -d ' ') links to the archives"
