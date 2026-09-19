fn main() {
    // sqlx::migrate! embeds migrations at compile time. Make Docker/Cargo rebuild
    // that embed whenever any migration is added or changed.
    println!("cargo:rerun-if-changed=migrations");
}
