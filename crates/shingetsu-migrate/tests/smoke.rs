//! Smoke test: confirms the crate compiles and re-exports the
//! expected backends for each feature combination.

#[test]
fn backend_label_reflects_features() {
    let label = shingetsu_migrate::_smoke_test();
    // The label is non-empty for any non-trivial feature combo.
    // We don't pin the exact value because the configuration that
    // runs this test depends on which features cargo selected.
    assert!(!label.is_empty());
}

// Confirms the shingetsu re-export is reachable when its backend
// feature is on.  Compiled out when shingetsu-backend is disabled.
#[cfg(feature = "shingetsu-backend")]
#[test]
fn shingetsu_reexport_is_reachable() {
    use shingetsu_migrate::shingetsu;
    let _env = shingetsu::GlobalEnv::new();
}

// Mirror for mlua: confirms the re-export is reachable when its
// backend feature is on.
#[cfg(feature = "mlua-backend")]
#[test]
fn mlua_reexport_is_reachable() {
    use shingetsu_migrate::mlua;
    let _lua = mlua::Lua::new();
}
