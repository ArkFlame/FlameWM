/// DEBUG-ONLY gate (local; no new cross-crate dep): true only when
/// `FLAMEWM_DEBUG=1`. Normal mode issues zero extra X sync.
pub(crate) fn host_debug_enabled() -> bool {
    std::env::var_os("FLAMEWM_DEBUG").is_some_and(|v| v == "1")
}

/// R2 tab-latency probe (debug-only, per click; never per frame). Stamps are
/// microseconds since the first probe in this process.
pub(crate) fn r2_now_us() -> u128 {
    static EPOCH: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    EPOCH
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_micros()
}

thread_local! {
    /// Ingress instant of the latest non-motion pointer event, consumed by the
    /// next reprojection present.
    pub(crate) static R2_INGRESS: std::cell::Cell<Option<std::time::Instant>> = const { std::cell::Cell::new(None) };
}
