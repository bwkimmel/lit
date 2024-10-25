#!/bin/bash

ENV=prod
IGNORE_TAGS=misparsed

cargo build --release --bin apply_tags

echo "Applying topik1 tags..."
target/release/apply_tags \
  --config=data/${ENV}/config.toml \
  -t topik1 \
  -x not:topik1 \
  -c topik2,$IGNORE_TAGS \
  --apply-exclusion-tag \
  data/topik1.txt

echo "Applying topik1v tags..."
target/release/apply_tags \
  --config=data/${ENV}/config.toml \
  -t topik1v \
  -x not:topik1v \
  -c topik2v,$IGNORE_TAGS \
  --apply-exclusion-tag \
  <(cat data/topik1.txt | grep .. | sed -e 's/$/하다/')

echo "Applying topik2 tags..."
target/release/apply_tags \
  --config=data/${ENV}/config.toml \
  -t topik2 \
  -x not:topik2 \
  -c topik1,$IGNORE_TAGS \
  --apply-exclusion-tag \
  data/topik2.txt

echo "Applying topik2v tags..."
target/release/apply_tags \
  --config=data/${ENV}/config.toml \
  -t topik2v \
  -x not:topik2v \
  -c topik1v,$IGNORE_TAGS \
  --apply-exclusion-tag \
  <(cat data/topik2.txt | grep .. | sed -e 's/$/하다/')
