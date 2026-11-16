#!/usr/bin/env sh
# Regenerates .sqlx/ (compile-time query metadata) after changing a query or
# a migration. Needs: cargo install sqlx-cli --no-default-features --features sqlite
set -eu
cd "$(dirname "$0")/.."
db="target/sqlx-dev.db"
mkdir -p target
rm -f "$db"
# sqlx on Windows wants a drive-letter path.
root="$(pwd -W 2>/dev/null || pwd)"
export DATABASE_URL="sqlite:$root/$db"
sqlx database create
sqlx migrate run --source crates/moonglass-db/migrations
SQLX_OFFLINE=false cargo sqlx prepare --workspace
