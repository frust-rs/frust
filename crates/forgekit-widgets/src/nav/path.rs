//! Location parsing and path-pattern matching for the [`router`](super::router)
//! (Phase 6b, task 04): the go_router-subset path layer.
//!
//! Two pieces, both dependency-free (hand-rolled, no `regex`):
//!
//! * [`Location`] — a *concrete* navigation target (`/users/42?tab=posts`) parsed
//!   into normalized path segments plus a query map. This is what `go`/`push`
//!   resolve and what a deep link delivers.
//! * [`PathPattern`] — a compiled *route path* (`/users/:id`) whose
//!   [`match_prefix`](PathPattern::match_prefix) consumes a run of location
//!   segments, capturing every `:param` into a [`RouteParams`] map. The
//!   [`router`](super::router) walks the route tree matching each route's pattern
//!   against the remaining segments, so parent + child patterns compose (go_router
//!   nesting semantics).
//!
//! No wildcards in v1 (only static segments and `:param`); percent-decoding is
//! minimal (`%XX` hex escapes), enough for the custom-scheme deep links this
//! phase targets.

use std::collections::BTreeMap;

/// Path/query parameters captured by a route match, keyed by name. A `BTreeMap`
/// so iteration (and therefore any derived query string) is deterministic.
pub type RouteParams = BTreeMap<String, String>;

/// A concrete navigation target: a normalized path plus its parsed segments and
/// query map. Produced by [`Location::parse`]; the unit the
/// [`router`](super::router) matches, redirects, and drives the navigator from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    /// The normalized path (leading slash, no trailing slash, percent-decoded):
    /// `/users/42`. The root path is `/`.
    pub path: String,
    /// The path split into decoded segments: `["users", "42"]`. Empty for `/`.
    pub segments: Vec<String>,
    /// The parsed, decoded query map (`?tab=posts&sort=asc` → `{tab: posts,
    /// sort: asc}`). A key with no `=` maps to an empty string.
    pub query: RouteParams,
}

impl Location {
    /// Parse a raw location string (`/users/:id`-style patterns are *not* parsed
    /// here — this is a concrete location like `/users/42?tab=posts`).
    ///
    /// Splits off the query at the first `?`, normalizes the path (strips a
    /// leading/trailing slash, percent-decodes each segment — trailing-slash
    /// tolerant), and parses `&`-separated `key=value` query pairs.
    pub fn parse(raw: &str) -> Location {
        let (path_part, query_part) = match raw.split_once('?') {
            Some((p, q)) => (p, Some(q)),
            None => (raw, None),
        };

        let segments: Vec<String> = path_part
            .split('/')
            .filter(|s| !s.is_empty())
            .map(percent_decode)
            .collect();

        let path = if segments.is_empty() {
            "/".to_string()
        } else {
            format!("/{}", segments.join("/"))
        };

        let mut query = RouteParams::new();
        if let Some(q) = query_part {
            for pair in q.split('&').filter(|s| !s.is_empty()) {
                let (k, v) = match pair.split_once('=') {
                    Some((k, v)) => (percent_decode(k), percent_decode(v)),
                    None => (percent_decode(pair), String::new()),
                };
                query.insert(k, v);
            }
        }

        Location {
            path,
            segments,
            query,
        }
    }

    /// The canonical string form (`path` plus a deterministically-ordered query
    /// string). Used to compare two locations for redirect fixed-point/loop
    /// detection, so it must be stable — hence the `BTreeMap`-ordered query.
    pub fn location_string(&self) -> String {
        if self.query.is_empty() {
            self.path.clone()
        } else {
            let query: Vec<String> = self
                .query
                .iter()
                .map(|(k, v)| {
                    if v.is_empty() {
                        percent_encode(k)
                    } else {
                        format!("{}={}", percent_encode(k), percent_encode(v))
                    }
                })
                .collect();
            format!("{}?{}", self.path, query.join("&"))
        }
    }
}

/// One segment of a compiled [`PathPattern`].
#[derive(Clone, Debug, PartialEq, Eq)]
enum Segment {
    /// A literal segment that must match a location segment verbatim.
    Static(String),
    /// A `:name` capture: matches any single location segment, binding it to
    /// `name` in the resulting [`RouteParams`].
    Param(String),
}

/// A compiled route path pattern (`/users/:id`) — a sequence of static and
/// `:param` [`Segment`]s. Matched against a run of [`Location`] segments by
/// [`match_prefix`](Self::match_prefix); the [`router`](super::router) composes
/// nested patterns by matching parent then child against successive segment runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathPattern {
    segments: Vec<Segment>,
}

impl PathPattern {
    /// Compile a pattern string into segments (trailing/leading slashes ignored).
    /// A `:name` segment is a parameter capture; everything else is static.
    pub fn parse(pattern: &str) -> PathPattern {
        let segments = pattern
            .split('/')
            .filter(|s| !s.is_empty())
            .map(|s| match s.strip_prefix(':') {
                Some(name) => Segment::Param(name.to_string()),
                None => Segment::Static(s.to_string()),
            })
            .collect();
        PathPattern { segments }
    }

    /// The number of segments this pattern consumes.
    pub fn len(&self) -> usize {
        self.segments.len()
    }

    /// Whether this pattern is empty (an index/root pattern that consumes no
    /// segments — e.g. a `/` route or a path-less shell route).
    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }

    /// Try to match this pattern as a *prefix* of `segs`, capturing every
    /// `:param`. Returns `(consumed, params)` — the number of leading segments
    /// consumed (always `self.len()`) and the captured params — or `None` if a
    /// static segment mismatches or there are too few segments to cover the
    /// pattern.
    ///
    /// Prefix (not whole-slice) matching is what lets nested routes compose: a
    /// parent consumes its segments, then the router feeds the remainder to the
    /// child.
    pub fn match_prefix(&self, segs: &[String]) -> Option<(usize, RouteParams)> {
        if segs.len() < self.segments.len() {
            return None;
        }
        let mut params = RouteParams::new();
        for (pat, seg) in self.segments.iter().zip(segs.iter()) {
            match pat {
                Segment::Static(s) => {
                    if s != seg {
                        return None;
                    }
                }
                Segment::Param(name) => {
                    params.insert(name.clone(), seg.clone());
                }
            }
        }
        Some((self.segments.len(), params))
    }
}

/// Decode minimal `%XX` percent-escapes in a single path/query token. Invalid or
/// truncated escapes are passed through literally (lenient — a deep link with a
/// stray `%` should not vanish).
fn percent_decode(s: &str) -> String {
    if !s.contains('%') {
        return s.to_string();
    }
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(hi), Some(lo)) = (hi, lo) {
                out.push((hi * 16 + lo) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    // Decoded bytes may or may not be valid UTF-8; fall back losslessly.
    String::from_utf8_lossy(&out).into_owned()
}

/// Percent-encode the characters that would break a `key=value&...` query string
/// or path (minimal set: the reserved delimiters, space, and `%` itself). Used
/// only to re-serialize a [`Location`] for stable comparison and for named-route
/// query building.
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'%' | b'?' | b'&' | b'=' | b'#' | b'/' | b' ' => {
                out.push('%');
                out.push_str(&format!("{b:02X}"));
            }
            _ => out.push(b as char),
        }
    }
    out
}

/// Percent-encode a single path segment value for named-route path building
/// (leaves `/` alone is *not* wanted here — a param value is one segment).
pub(super) fn encode_segment(s: &str) -> String {
    percent_encode(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segs(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_static_path() {
        let loc = Location::parse("/users/list");
        assert_eq!(loc.path, "/users/list");
        assert_eq!(loc.segments, segs(&["users", "list"]));
        assert!(loc.query.is_empty());
    }

    #[test]
    fn root_path_normalizes() {
        let loc = Location::parse("/");
        assert_eq!(loc.path, "/");
        assert!(loc.segments.is_empty());
        // An empty string parses to root too.
        assert_eq!(Location::parse("").path, "/");
    }

    #[test]
    fn trailing_slash_is_tolerated() {
        assert_eq!(Location::parse("/users/").segments, segs(&["users"]));
        assert_eq!(Location::parse("/users").segments, segs(&["users"]));
        assert_eq!(
            Location::parse("/users/").location_string(),
            Location::parse("/users").location_string()
        );
    }

    #[test]
    fn parses_query() {
        let loc = Location::parse("/users/42?tab=posts&sort=asc");
        assert_eq!(loc.segments, segs(&["users", "42"]));
        assert_eq!(loc.query.get("tab").map(String::as_str), Some("posts"));
        assert_eq!(loc.query.get("sort").map(String::as_str), Some("asc"));
        // Deterministic (BTreeMap-ordered) re-serialization.
        assert_eq!(loc.location_string(), "/users/42?sort=asc&tab=posts");
    }

    #[test]
    fn query_key_without_value() {
        let loc = Location::parse("/search?debug");
        assert_eq!(loc.query.get("debug").map(String::as_str), Some(""));
    }

    #[test]
    fn percent_decodes_segments_and_query() {
        let loc = Location::parse("/notes/hello%20world?q=a%26b");
        assert_eq!(loc.segments, segs(&["notes", "hello world"]));
        assert_eq!(loc.query.get("q").map(String::as_str), Some("a&b"));
    }

    #[test]
    fn matches_static_pattern() {
        let pat = PathPattern::parse("/users");
        let (consumed, params) = pat.match_prefix(&segs(&["users"])).expect("matches");
        assert_eq!(consumed, 1);
        assert!(params.is_empty());
        assert!(pat.match_prefix(&segs(&["posts"])).is_none());
    }

    #[test]
    fn captures_param() {
        let pat = PathPattern::parse("/users/:id");
        let (consumed, params) = pat.match_prefix(&segs(&["users", "42"])).expect("matches");
        assert_eq!(consumed, 2);
        assert_eq!(params.get("id").map(String::as_str), Some("42"));
    }

    #[test]
    fn match_prefix_leaves_remaining_segments() {
        // A parent pattern consumes its own segments only, leaving the rest for
        // a child (the nesting seam).
        let parent = PathPattern::parse("/users");
        let (consumed, _) = parent
            .match_prefix(&segs(&["users", "42", "posts"]))
            .expect("prefix matches");
        assert_eq!(consumed, 1);
    }

    #[test]
    fn too_few_segments_fails() {
        let pat = PathPattern::parse("/users/:id");
        assert!(pat.match_prefix(&segs(&["users"])).is_none());
    }

    #[test]
    fn empty_pattern_consumes_nothing() {
        let pat = PathPattern::parse("/");
        assert!(pat.is_empty());
        let (consumed, params) = pat.match_prefix(&segs(&["anything"])).expect("matches");
        assert_eq!(consumed, 0);
        assert!(params.is_empty());
    }
}
