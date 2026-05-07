//! Shared diagnostic helpers used by derive-generated converters and
//! (eventually) the type checker.
//!
//! Currently provides Jaro-Winkler "did you mean" suggestions for
//! unknown field/key names.

use bstr::BStr;
use std::fmt::Write as _;

/// Jaro-Winkler similarity threshold for a name to be considered a
/// "close match" worth suggesting.  Tuned for struct field name
/// suggestions where typos are typically off by one or two characters.
pub const FIELD_SUGGEST_THRESHOLD: f64 = 0.8;

/// Maximum number of suggestions listed inline.  Past this the
/// `did you mean` line is replaced by a single "too many close
/// matches" hint to keep error messages readable when the user is
/// poking at a struct with many similar field names.
pub const FIELD_SUGGEST_MAX: usize = 5;

/// Maximum number of "other alternatives" listed inline.  Beyond
/// this we recommend consulting docs rather than dumping a long
/// field list into a one-line error.
pub const FIELD_SUGGEST_LIST_LIMIT: usize = 5;

/// Jaro-Winkler threshold used by the compile-time event-handler
/// checker to decide when two parameter names are "related enough"
/// to participate in transposition detection.  Lower than
/// [`FIELD_SUGGEST_THRESHOLD`] because it must pair canonical
/// signature names (`message`, `domain`, `password`) with the
/// abbreviated forms users commonly write (`msg`, `dom`, `pwd`),
/// while still rejecting unrelated names.  Tuned against real
/// declare_event! signatures from real-world hosts.
pub const EVENT_PARAM_MATCH_THRESHOLD: f64 = 0.6;

/// One detected parameter swap between handler and signature
/// positions.  Used by the compile-time event-handler checker.
#[derive(Debug, Clone, PartialEq)]
pub struct ParamSwap {
    /// Handler position (0-based) where the user wrote `name_at_handler`.
    pub handler_position: usize,
    /// Name in the user's handler at that position.
    pub name_at_handler: Vec<u8>,
    /// Signature position the handler's name best matches.
    pub signature_position: usize,
    /// Signature parameter name at `signature_position`.
    pub name_at_signature: Vec<u8>,
}

/// Detect transposed parameter names between a handler and a typed
/// signature.
///
/// The strategy is symmetric-best-match: for each handler parameter
/// position `i`, find the signature parameter most similar (above
/// [`EVENT_PARAM_MATCH_THRESHOLD`]).  A swap is reported when:
///
/// 1. handler position `i`'s best match is at signature position `j`
///    (with `j != i`); **and**
/// 2. handler position `j`'s best match is back at signature
///    position `i`.
///
/// Mere abbreviations at the same position (`msg` vs `message`),
/// novel names with no close match, and forward-compatible
/// shorter handlers all fall through silently.
///
/// `handler_params` and `signature_params` are byte slices.  Returns
/// the detected swaps in handler-position order.
pub fn detect_param_swaps(handler_params: &[&[u8]], signature_params: &[&[u8]]) -> Vec<ParamSwap> {
    // For each handler position, find the signature position whose
    // name best matches.  Placeholder names (`_`, `_1`, ...) opt the
    // position out of swap detection.
    let best_match_for: Vec<Option<usize>> = handler_params
        .iter()
        .map(|h| {
            if is_placeholder_name(h) {
                None
            } else {
                best_match_position(h, signature_params)
            }
        })
        .collect();

    let mut swaps = Vec::new();
    let mut already_reported: std::collections::HashSet<usize> = std::collections::HashSet::new();

    for (i, best_j) in best_match_for.iter().enumerate() {
        if already_reported.contains(&i) {
            continue;
        }
        let Some(j) = *best_j else { continue };
        if j == i {
            continue;
        }
        if i >= handler_params.len() || j >= handler_params.len() {
            continue;
        }
        let symmetric = best_match_for.get(j).and_then(|x| *x);
        if symmetric != Some(i) {
            continue;
        }
        if j >= signature_params.len() {
            continue;
        }
        swaps.push(ParamSwap {
            handler_position: i,
            name_at_handler: handler_params[i].to_vec(),
            signature_position: j,
            name_at_signature: signature_params[j].to_vec(),
        });
        // Also record the symmetric swap so we don't double-emit.
        swaps.push(ParamSwap {
            handler_position: j,
            name_at_handler: handler_params[j].to_vec(),
            signature_position: i,
            name_at_signature: signature_params[i].to_vec(),
        });
        already_reported.insert(i);
        already_reported.insert(j);
    }
    swaps
}

/// Strip a leading single underscore for scoring purposes only.  The
/// returned slice is purely an input to `jaro_winkler`; diagnostic
/// messages still display the original name.  Honours the lua
/// convention that `_foo` means "intentionally unused" — the body of
/// such a parameter is the same name minus the underscore.
fn normalize_for_scoring(name: &[u8]) -> &[u8] {
    if name.len() > 1 && name[0] == b'_' && name[1] != b'_' {
        &name[1..]
    } else {
        name
    }
}

/// True for placeholder identifiers that signal "I don't care about
/// this position".  Honours `_`, `_1`, `_2`, etc.  Anything else —
/// including `_message` — is a real (just intentionally-unused) name
/// and still participates in swap detection.
pub fn is_placeholder_name(name: &[u8]) -> bool {
    if name == b"_" {
        return true;
    }
    if name.starts_with(b"_") && name.len() > 1 && name[1..].iter().all(u8::is_ascii_digit) {
        return true;
    }
    false
}

/// Return the signature position whose name best matches `name`,
/// when at least one exceeds [`EVENT_PARAM_MATCH_THRESHOLD`].
fn best_match_position(name: &[u8], signature: &[&[u8]]) -> Option<usize> {
    let used = String::from_utf8_lossy(normalize_for_scoring(name));
    let mut best: Option<(usize, f64)> = None;
    for (i, sig_name) in signature.iter().enumerate() {
        let candidate = String::from_utf8_lossy(normalize_for_scoring(sig_name));
        let score = strsim::jaro_winkler(&used, &candidate);
        if score < EVENT_PARAM_MATCH_THRESHOLD {
            continue;
        }
        match best {
            None => best = Some((i, score)),
            Some((_, prev)) if score > prev => best = Some((i, score)),
            _ => {}
        }
    }
    best.map(|(i, _)| i)
}

/// Return the close-match candidates for `used` from `possible`,
/// sorted by descending Jaro-Winkler similarity.  Names with
/// similarity at or below [`FIELD_SUGGEST_THRESHOLD`] are excluded.
///
/// Inputs are byte slices to handle non-UTF8 lua keys; the
/// similarity comparison uses the lossy UTF-8 view of each name.
pub fn close_matches<'a>(used: &str, possible: &'a [&[u8]]) -> Vec<&'a [u8]> {
    let mut scored: Vec<(f64, &[u8])> = possible
        .iter()
        .map(|name| {
            let s = String::from_utf8_lossy(name);
            (strsim::jaro_winkler(used, s.as_ref()), *name)
        })
        .filter(|(score, _)| *score > FIELD_SUGGEST_THRESHOLD)
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.into_iter().map(|(_, n)| n).collect()
}

/// Render a "did you mean ..." message for an unknown name.  Returns
/// an empty string when there's nothing useful to say (no close
/// matches and no alternatives to list).
///
/// `item_kind` (e.g. `"field"`, `"event"`, `"parameter"`) is
/// pluralised in the truncation hint and the singular-alternative
/// phrasing so messages read naturally for whatever kind of name
/// the caller is suggesting.  Convenience wrappers below pre-supply
/// common item kinds.
pub fn render_suggestion(used: &str, item_kind: &str, possible: &[&[u8]]) -> String {
    let suggestions = close_matches(used, possible);

    let mut others: Vec<&[u8]> = possible
        .iter()
        .copied()
        .filter(|name| !suggestions.iter().any(|s| s == name))
        .collect();
    others.sort_unstable();

    let mut msg = String::new();
    let too_many_suggestions = suggestions.len() > FIELD_SUGGEST_MAX;
    let display_suggestions: &[&[u8]] = if too_many_suggestions {
        &[]
    } else {
        suggestions.as_slice()
    };

    match display_suggestions.len() {
        0 if too_many_suggestions => {
            write!(
                msg,
                "Many {item_kind}s share a similar name; consult the documentation \
                 for the full list.",
            )
            .ok();
        }
        0 => {}
        1 => {
            write!(msg, "Did you mean `{}`?", BStr::new(display_suggestions[0])).ok();
        }
        _ => {
            msg.push_str("Did you mean one of ");
            for (i, s) in display_suggestions.iter().enumerate() {
                if i > 0 {
                    msg.push_str(", ");
                }
                write!(msg, "`{}`", BStr::new(s)).ok();
            }
            msg.push('?');
        }
    }

    // If we already replaced suggestions with the truncation hint, don't
    // also dump a 45-name "others" list.
    if too_many_suggestions {
        return msg;
    }

    if !others.is_empty() {
        if others.len() > FIELD_SUGGEST_LIST_LIMIT && !suggestions.is_empty() {
            msg.push_str(
                " There are too many alternatives to list here; consult the documentation!",
            );
        } else if others.len() <= FIELD_SUGGEST_LIST_LIMIT {
            // Choose phrasing based on the number of *others* (the items
            // we're about to enumerate), not the suggestion count.  Same
            // shape regardless of whether we offered close matches.
            match (suggestions.is_empty(), others.len()) {
                (true, 1) => {
                    write!(msg, "The only valid {item_kind} is ").ok();
                }
                (true, _) => msg.push_str("Possible alternatives are "),
                (false, 1) => msg.push_str(" The other option is "),
                (false, _) => msg.push_str(" Other alternatives are "),
            }
            for (i, name) in others.iter().enumerate() {
                if i > 0 {
                    msg.push_str(", ");
                }
                write!(msg, "`{}`", BStr::new(name)).ok();
            }
        }
    }

    msg
}

/// Convenience wrapper: [`render_suggestion`] with `item_kind = "field"`.
pub fn render_field_suggestion(used: &str, possible: &[&[u8]]) -> String {
    render_suggestion(used, "field", possible)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_match_picks_typo() {
        let possible: Vec<&[u8]> = vec![b"font_size", b"font_family", b"line_height"];
        let matches = close_matches("font_sze", &possible);
        k9::assert_equal!(matches, vec![&b"font_size"[..]]);
    }

    #[test]
    fn no_close_match_returns_empty_vec() {
        let possible: Vec<&[u8]> = vec![b"alpha", b"beta", b"gamma"];
        let matches = close_matches("xyzzy", &possible);
        let empty: Vec<&[u8]> = vec![];
        k9::assert_equal!(matches, empty);
    }

    #[test]
    fn render_single_match_with_few_alternatives() {
        // `fooo` has high similarity to both `foo` and `foo_x`, so both
        // surface as suggestions.  Bare `bar` / `baz` go to alternatives.
        let possible: Vec<&[u8]> = vec![b"foo", b"bar", b"baz", b"foo_x"];
        let msg = render_field_suggestion("fooo", &possible);
        k9::assert_equal!(
            msg,
            "Did you mean one of `foo`, `foo_x`? Other alternatives are `bar`, `baz`"
        );
    }

    #[test]
    fn render_multi_match_lists_all() {
        let possible: Vec<&[u8]> = vec![b"foo_bar", b"foo_baz", b"unrelated"];
        let msg = render_field_suggestion("foo_bay", &possible);
        k9::assert_equal!(
            msg,
            "Did you mean one of `foo_bar`, `foo_baz`? The other option is `unrelated`"
        );
    }

    #[test]
    fn render_no_match_with_single_alternative_uses_singular_phrasing() {
        let possible: Vec<&[u8]> = vec![b"only_one"];
        let msg = render_field_suggestion("xyzzy", &possible);
        k9::assert_equal!(msg, "The only valid field is `only_one`");
    }

    #[test]
    fn render_suggestion_uses_provided_item_kind() {
        let possible: Vec<&[u8]> = vec![b"only_one"];
        let msg = render_suggestion("xyzzy", "event", &possible);
        k9::assert_equal!(msg, "The only valid event is `only_one`");
    }

    #[test]
    fn render_suggestion_pluralises_truncation_hint() {
        // 50 events sharing a similar prefix — the truncation hint
        // should pluralise the item kind correctly.
        let names: Vec<Vec<u8>> = (0..50)
            .map(|i| format!("on_evt_{i:02}").into_bytes())
            .collect();
        let possible: Vec<&[u8]> = names.iter().map(|v| v.as_slice()).collect();
        let msg = render_suggestion("on_evt_07x", "event", &possible);
        k9::assert_equal!(
            msg,
            "Many events share a similar name; consult the documentation for the full list."
        );
    }

    #[test]
    fn render_no_match_lists_alternatives_when_short() {
        let possible: Vec<&[u8]> = vec![b"alpha", b"beta", b"gamma"];
        let msg = render_field_suggestion("xyzzy", &possible);
        k9::assert_equal!(msg, "Possible alternatives are `alpha`, `beta`, `gamma`");
    }

    #[test]
    fn render_long_field_list_truncates_with_doc_pointer() {
        // 50-field struct fixture: confirm
        // we don't dump 50 names into the message even when many
        // names share a similar prefix.
        let names: Vec<Vec<u8>> = (0..50)
            .map(|i| format!("field_{i:02}").into_bytes())
            .collect();
        let possible: Vec<&[u8]> = names.iter().map(|v| v.as_slice()).collect();
        let msg = render_field_suggestion("field_07x", &possible);
        k9::assert_equal!(
            msg,
            "Many fields share a similar name; consult the documentation for the full list."
        );
    }

    #[test]
    fn render_returns_empty_when_nothing_useful() {
        let possible: Vec<&[u8]> = vec![];
        let msg = render_field_suggestion("anything", &possible);
        k9::assert_equal!(msg, "");
    }

    // ---- detect_param_swaps -----------------------------------------

    fn s(b: &str) -> &[u8] {
        b.as_bytes()
    }

    #[test]
    fn swap_canonical_transposition() {
        let handler = vec![s("domain"), s("message")];
        let sig = vec![s("message"), s("domain")];
        k9::assert_equal!(
            detect_param_swaps(&handler, &sig),
            vec![
                ParamSwap {
                    handler_position: 0,
                    name_at_handler: b"domain".to_vec(),
                    signature_position: 1,
                    name_at_signature: b"domain".to_vec(),
                },
                ParamSwap {
                    handler_position: 1,
                    name_at_handler: b"message".to_vec(),
                    signature_position: 0,
                    name_at_signature: b"message".to_vec(),
                },
            ]
        );
    }

    #[test]
    fn swap_abbreviated_transposition() {
        // `dom` and `msg` swapped against canonical `message, domain`.
        let handler = vec![s("dom"), s("msg")];
        let sig = vec![s("message"), s("domain")];
        k9::assert_equal!(
            detect_param_swaps(&handler, &sig),
            vec![
                ParamSwap {
                    handler_position: 0,
                    name_at_handler: b"dom".to_vec(),
                    signature_position: 1,
                    name_at_signature: b"domain".to_vec(),
                },
                ParamSwap {
                    handler_position: 1,
                    name_at_handler: b"msg".to_vec(),
                    signature_position: 0,
                    name_at_signature: b"message".to_vec(),
                },
            ]
        );
    }

    #[test]
    fn no_swap_when_correctly_ordered() {
        let handler = vec![s("message"), s("domain")];
        let sig = vec![s("message"), s("domain")];
        k9::assert_equal!(detect_param_swaps(&handler, &sig), vec![]);
    }

    #[test]
    fn no_swap_when_abbreviated_at_correct_positions() {
        let handler = vec![s("msg"), s("dom")];
        let sig = vec![s("message"), s("domain")];
        k9::assert_equal!(detect_param_swaps(&handler, &sig), vec![]);
    }

    #[test]
    fn no_swap_when_handler_uses_novel_names() {
        let handler = vec![s("a"), s("b")];
        let sig = vec![s("message"), s("domain")];
        k9::assert_equal!(detect_param_swaps(&handler, &sig), vec![]);
    }

    #[test]
    fn no_swap_when_handler_shorter_than_signature() {
        // Forward-compat: handler accepts only the first param.
        let handler = vec![s("message")];
        let sig = vec![s("message"), s("domain")];
        k9::assert_equal!(detect_param_swaps(&handler, &sig), vec![]);
    }

    #[test]
    fn detects_authz_authc_swap_via_exact_match_dominance() {
        // Both signatures pair with score 0.92 via Jaro-Winkler;
        // exact matches at score 1.0 still drive the swap detector
        // when the handler transposes them.
        let handler = vec![s("authc"), s("authz")];
        let sig = vec![s("authz"), s("authc")];
        k9::assert_equal!(
            detect_param_swaps(&handler, &sig),
            vec![
                ParamSwap {
                    handler_position: 0,
                    name_at_handler: b"authc".to_vec(),
                    signature_position: 1,
                    name_at_signature: b"authc".to_vec(),
                },
                ParamSwap {
                    handler_position: 1,
                    name_at_handler: b"authz".to_vec(),
                    signature_position: 0,
                    name_at_signature: b"authz".to_vec(),
                },
            ]
        );
    }

    #[test]
    fn no_false_positive_on_authz_authc_when_correctly_ordered() {
        let handler = vec![s("authz"), s("authc")];
        let sig = vec![s("authz"), s("authc")];
        k9::assert_equal!(detect_param_swaps(&handler, &sig), vec![]);
    }

    // ---- underscore-prefix and placeholder handling -----------------

    #[test]
    fn underscore_prefixed_handler_params_match_canonical() {
        // `_msg` and `_domain` are intentionally-unused names; they
        // should still be position-checked, just with the underscore
        // stripped for similarity scoring.  Same positions → no swap.
        let handler = vec![s("_msg"), s("_domain")];
        let sig = vec![s("message"), s("domain")];
        k9::assert_equal!(detect_param_swaps(&handler, &sig), vec![]);
    }

    #[test]
    fn underscore_prefixed_handler_params_caught_when_swapped() {
        // Even with the underscore convention, transpositions are
        // still detectable.
        let handler = vec![s("_dom"), s("_msg")];
        let sig = vec![s("message"), s("domain")];
        k9::assert_equal!(
            detect_param_swaps(&handler, &sig),
            vec![
                ParamSwap {
                    handler_position: 0,
                    name_at_handler: b"_dom".to_vec(),
                    signature_position: 1,
                    name_at_signature: b"domain".to_vec(),
                },
                ParamSwap {
                    handler_position: 1,
                    name_at_handler: b"_msg".to_vec(),
                    signature_position: 0,
                    name_at_signature: b"message".to_vec(),
                },
            ]
        );
    }

    #[test]
    fn placeholder_names_are_skipped_from_swap_detection() {
        // `function(_, _, _, _, x)` against any 5-arity sig: no swap
        // warnings should surface even when the lone real name `x`
        // happens to abbreviate something on a different position.
        let handler = vec![s("_"), s("_"), s("_"), s("_"), s("msg")];
        let sig = vec![s("a"), s("b"), s("c"), s("message"), s("d")];
        k9::assert_equal!(detect_param_swaps(&handler, &sig), vec![]);
    }

    #[test]
    fn numeric_placeholder_names_skipped() {
        // `_1`, `_2` etc. are also placeholders.
        let handler = vec![s("_1"), s("_2"), s("x")];
        let sig = vec![s("a"), s("b"), s("y")];
        k9::assert_equal!(detect_param_swaps(&handler, &sig), vec![]);
    }

    #[test]
    fn is_placeholder_name_classifies_correctly() {
        k9::assert_equal!(is_placeholder_name(b"_"), true);
        k9::assert_equal!(is_placeholder_name(b"_1"), true);
        k9::assert_equal!(is_placeholder_name(b"_42"), true);
        k9::assert_equal!(is_placeholder_name(b"_msg"), false);
        k9::assert_equal!(is_placeholder_name(b"_message"), false);
        k9::assert_equal!(is_placeholder_name(b"__internal"), false);
        k9::assert_equal!(is_placeholder_name(b"msg"), false);
        k9::assert_equal!(is_placeholder_name(b""), false);
    }
}
