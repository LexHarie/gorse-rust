#!/usr/bin/env bash
# Places the model-test datasets under $HOME/.gorse/dataset, the folder the Rust
# tests read (and Go's common/datautil uses). Run it before `cargo test` on any
# machine where the dataset-dependent tests run: nn_test::test_iris,
# nn_test::test_mnist, ann::tests::test_mnist and ann::tests::test_multithread,
# among others. Without it those tests fail on a missing file.
#
# As Go's DownloadAndUnzip does, a dataset is fetched only when it is missing,
# and its zip is unzipped into $HOME/.gorse/dataset. The zip comes from, in order:
#   1. $DATASETS_SRC/<name>.zip, when that file exists. The default is
#      /mnt/project-files/datasets, where the court keeps the uploads.
#   2. https://cdn.gorse.io/datasets/<name>.zip, the URL Go uses.
#
# `yelp` unzips its two files into the dataset root, as Go's TestUnzip does.
# The .bin.gz files are not zips and are not handled here.
#
# Each zip is checked for entries that escape the dataset folder before it is
# extracted, and existing files are never overwritten.
#
# Environment:
#   DATASETS_SRC     local folder of <name>.zip files (default /mnt/project-files/datasets)
#   GORSE_DATASETS   space-separated dataset names to place (default: all of them)
set -euo pipefail

DEST="$HOME/.gorse/dataset"
SRC="${DATASETS_SRC:-/mnt/project-files/datasets}"
URL_BASE="https://cdn.gorse.io/datasets"
NAMES="${GORSE_DATASETS:-ml-100k ml-1m pinterest-20 frappe ml-tag criteo iris mnist yelp}"

mkdir -p "$DEST"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

for name in $NAMES; do
  if [ "$name" = yelp ]; then
    marker="$DEST/train.txt"
  else
    marker="$DEST/$name"
  fi
  if [ -e "$marker" ]; then
    echo "present: $name"
    continue
  fi

  zip="$SRC/$name.zip"
  if [ -f "$zip" ]; then
    echo "unzip:   $name from $zip"
  else
    zip="$TMP/$name.zip"
    echo "fetch:   $name from $URL_BASE/$name.zip"
    curl -fsSL --retry 4 -o "$zip" "$URL_BASE/$name.zip"
  fi

  if unzip -Z1 "$zip" | grep -E -q '^/|(^|/)\.\.(/|$)'; then
    echo "refusing $name: an entry escapes the dataset folder" >&2
    exit 1
  fi
  unzip -q -n "$zip" -d "$DEST"
  rm -f "$TMP/$name.zip"
done

echo "datasets in $DEST"
