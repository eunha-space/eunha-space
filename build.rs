// `sqlx::migrate!()` embeds the migrations directory at compile time, and
// nothing in `src` changes when a migration is added. Without this, Cargo
// considers the crate fresh and a migration-only commit deploys a binary that
// still carries the previous set, silently skipping the new migration.
fn main() {
    println!("cargo:rerun-if-changed=migrations");
}
