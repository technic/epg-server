#!/bin/bash
if ! test -d "$1"; then
  echo "Not a directory: $1"
  exit 1
fi
cd "$1"
echo "Running backup-db in $(pwd)"
sqlite3 epg.db ".backup epg.db.$(date +%Y-%m-%d)"
ls |grep 'epg\.db\.' |sort -r |tail -n +3 |xargs -r rm -v
