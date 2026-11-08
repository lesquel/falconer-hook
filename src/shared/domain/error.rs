/// Stable, machine-readable error code exposed in the JSON envelope.
/// Messages attached to a code never contain secrets (control T-7).
pub trait ErrorCode {
    fn code(&self) -> &'static str;
}
