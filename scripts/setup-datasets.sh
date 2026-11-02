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
#
# The playground dumps are not zips. Each is copied into the dataset folder as it
# is, under its own name (`ml-100k.bin.gz`). It comes from $DATASETS_SRC when that
# file exists there, and otherwise from https://cdn.gorse.io/example/, the URL the
# playground downloads. The playground tests read the dump from this folder.
#
# Each zip is checked for entries that escape the dataset folder before it is
# extracted, and existing files are never overwritten.
#
# Environment:
#   DATASETS_SRC     local folder of <name>.zip and dump files (default /mnt/project-files/datasets)
#   GORSE_DATASETS   space-separated dataset names to place (default: all of them)
#   GORSE_DUMPS      space-separated playground dump file names to place (default: ml-100k.bin.gz)
set -euo pipefail

DEST="$HOME/.gorse/dataset"
SRC="${DATASETS_SRC:-/mnt/project-files/datasets}"
URL_BASE="https://cdn.gorse.io/datasets"
DUMP_URL_BASE="https://cdn.gorse.io/example"
NAMES="${GORSE_DATASETS:-ml-100k ml-1m pinterest-20 frappe ml-tag criteo iris mnist yelp}"
DUMPS="${GORSE_DUMPS:-ml-100k.bin.gz}"

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

for dump in $DUMPS; do
  marker="$DEST/$dump"
  if [ -e "$marker" ]; then
    echo "present: $dump"
    continue
  fi

  src="$SRC/$dump"
  if [ -f "$src" ]; then
    echo "copy:    $dump from $src"
    cp "$src" "$TMP/$dump"
  else
    echo "fetch:   $dump from $DUMP_URL_BASE/$dump"
    curl -fsSL --retry 4 -o "$TMP/$dump" "$DUMP_URL_BASE/$dump"
  fi
  mv "$TMP/$dump" "$marker"
done

echo "datasets in $DEST"
