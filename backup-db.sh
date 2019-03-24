#!/bin/bash
cd "$(dirname "$0")"
echo "Running backup-db in $(pwd)"
sqlite3 epg.db ".backup epg.db.$(date +%Y-%m-%d)"
ls |grep 'epg\.db\.' |sort -r |tail -n +5 |xargs rm -v
