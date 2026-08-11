//! Parsing and comparing k2g release versions.
//!
//! Shared by `build.rs` (which warns when `Cargo.toml` falls behind the newest git
//! tag) and by `runtime::update` (which decides whether a GitHub release is newer
//! than the running build) via `include!`, because a build script cannot import from
//! the crate it builds. One definition, two users, no chance of the two disagreeing
//! about what "newer" means.
//!
//! # Why not `semver`
//!
//! This project tags releases `vX.Y.Z-<codename>` — `v0.9.0-typed-values`. Semver
//! reads everything after the hyphen as a **pre-release**, and a pre-release sorts
//! *below* the plain version. `semver::Version::parse("0.9.0-typed-values") <
//! Version::parse("0.9.0")`, so a straight semver comparison concludes that every
//! release this project has ever made is older than itself, and the update check
//! would never fire.
//!
//! The codename is decoration. Only the numeric core is compared, and anything that
//! is not three dot-separated integers is somebody else's tagging scheme and is
//! ignored rather than guessed at.

/// The `(major, minor, patch)` core of a version or tag, if it has one.
///
/// Accepts an optional leading `v` and ignores any `-suffix` or `+build`:
/// `"v0.9.0-typed-values"`, `"0.9.0"` and `"v0.9.0"` all yield `(0, 9, 0)`.
pub fn parse_core(text: &str) -> Option<(u32, u32, u32)> {
    let numeric = text
        .trim()
        .trim_start_matches('v')
        .split(['-', '+'])
        .next()?;

    let mut parts = numeric.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    // Exactly three parts: "1.2.3.4" is not a version this project produces, and
    // silently reading it as 1.2.3 would compare two different things as equal.
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

/// Whether `candidate` names a strictly newer release than `current`.
///
/// `false` when either side is unparsable. That direction is deliberate: an
/// unrecognised tag must never be offered as an upgrade, because the download and
/// install that follow are the most dangerous thing this application does.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_core(candidate), parse_core(current)) {
        (Some(new), Some(now)) => new > now,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_codename_suffix_this_project_tags_with_is_ignored() {
        // The whole reason this module exists instead of a `semver` dependency.
        assert_eq!(parse_core("v0.9.0-typed-values"), Some((0, 9, 0)));
        assert_eq!(parse_core("0.9.0"), Some((0, 9, 0)));
        assert_eq!(parse_core("v0.9.0"), Some((0, 9, 0)));
        assert_eq!(parse_core(" v1.10.2-edge-routing \n"), Some((1, 10, 2)));
        assert_eq!(parse_core("0.9.0+build7"), Some((0, 9, 0)));
    }

    #[test]
    fn a_tag_is_not_newer_than_the_version_it_names() {
        // The bug a naive semver comparison produces: `0.9.0-typed-values` parses as
        // a pre-release of 0.9.0 and sorts below it, so the running build would see
        // its own release as an upgrade and offer to install it, forever.
        assert!(!is_newer("v0.9.0-typed-values", "0.9.0"));
        assert!(!is_newer("v0.9.0", "0.9.0"));
    }

    #[test]
    fn ordering_is_numeric_per_component() {
        assert!(is_newer("v0.10.0-anything", "0.9.0"), "10 > 9, not \"10\" < \"9\"");
        assert!(is_newer("v1.0.0", "0.99.99"));
        assert!(is_newer("v0.9.1", "0.9.0"));
        assert!(!is_newer("v0.9.0", "0.9.1"));
        assert!(!is_newer("v0.8.9", "0.9.0"));
    }

    #[test]
    fn anything_unparsable_is_never_an_upgrade() {
        // Fail closed. Whatever this is, it is not a release worth downloading and
        // running an installer from.
        for junk in ["", "latest", "v", "nightly", "1.2", "1.2.3.4", "v.9.0", "0.9.x"] {
            assert!(!is_newer(junk, "0.9.0"), "{junk:?} must not be an upgrade");
        }
        assert!(!is_newer("v99.0.0", "not-a-version"));
    }
}
