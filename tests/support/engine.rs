//! The engine a run of the suite selects, for expectations that follow it.

/// The fingerprint label of the engine this run of the suite selects.
pub fn selected_version() -> &'static str {
    if vitest_dedup::corpus::Engine::selected().is_some() {
        vitest_dedup::model::Identity::Next
    } else {
        vitest_dedup::model::Identity::Current
    }
    .version()
}
