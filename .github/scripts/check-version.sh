#!/usr/bin/env bash
# The tag is the version (SPEC-release.md 3): `vX.Y.Z`, or `vX.Y.Z-suffix`
# for a pre-release, and X.Y.Z is the version `Cargo.toml` carries, the one
# the release pull request set. A pre-release of that version passes: v0.1.0-rc1
# against 0.1.0. Run by the release workflow in its first job, before
# anything is built.
#
#   check-version.sh <tag> [<Cargo.toml>]
set -euo pipefail

tag="${1:?usage: check-version.sh <tag> [<Cargo.toml>]}"
manifest="${2:-Cargo.toml}"

form='^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z]+(\.[0-9A-Za-z]+)*)?$'
if [[ ! "$tag" =~ $form ]]; then
  echo "\`$tag\` is not a release tag: a release is tagged vX.Y.Z, a pre-release vX.Y.Z-suffix (v0.1.0, v0.2.0-rc1)"
  exit 1
fi
version="${tag#v}"

# The version of the workspace: the `version` key of [workspace.package].
crate="$(awk '
  /^\[/ { section = $0 }
  section == "[workspace.package]" && /^version[ \t]*=[ \t]*"/ {
    sub(/^version[ \t]*=[ \t]*"/, ""); sub(/".*$/, ""); print; exit
  }' "$manifest")"
if [ -z "$crate" ]; then
  echo "$manifest has no version under [workspace.package]"
  exit 1
fi

if [ "$version" != "$crate" ] && [ "${version%%-*}" != "$crate" ]; then
  echo "the tag $tag is not version $crate, the version of $manifest"
  echo "The release pull request sets the version; the tag goes on its merge commit."
  exit 1
fi
echo "the tag $tag is version $crate of $manifest"
