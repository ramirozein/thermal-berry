pub(super) const VENDOR: &str = "thinkpad";
pub(super) const HWMON_NAME: &str = "thinkpad";

pub(super) fn matches_device(
    manufacturer: &str,
    model: Option<&str>,
    family: Option<&str>,
    has_driver: impl FnOnce() -> bool,
) -> bool {
    let branded = [model, family]
        .into_iter()
        .flatten()
        .any(|name| name.to_ascii_lowercase().contains(VENDOR));

    match manufacturer {
        "lenovo" => branded || has_driver(),
        "ibm" => branded,
        _ => false,
    }
}
