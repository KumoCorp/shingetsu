//! `declare_event!` captures rustdoc on the static and per-param,
//! plus an optional `#[returns = "..."]` attribute, into the
//! resulting [`EventSignature`].  The metadata is accessible
//! regardless of which backend is active so kumomta-style
//! mlua-only doc-build pipelines can render per-event reference
//! pages from the same source as the runtime registration.

#![cfg(any(feature = "shingetsu-backend", feature = "mlua-backend"))]

use shingetsu_migrate::declare_event;

declare_event! {
    /// Resolve the queue config for an outgoing message.
    /// Multiple handlers may participate; the first non-empty
    /// result wins.
    #[returns = "The QueueConfig that drives delivery scheduling for this message."]
    pub static GET_QUEUE_CONFIG: Multiple(
        "get_queue_config",
        /// Fully-qualified destination domain.
        domain: String,
        /// Optional tenant identifier from message metadata.
        tenant: Option<String>,
    ) -> String;
}

declare_event! {
    pub static ON_RESET: Single("on_reset") -> ();
}

#[test]
fn declare_event_captures_event_summary() {
    // The captured strings preserve the leading space after `///`
    // exactly as rustdoc emits them; the doc-build pipeline that
    // consumes this metadata is responsible for trimming if it
    // wants tighter formatting.  Asserting the raw bytes here
    // makes any future change to the macro's whitespace handling
    // visible.
    k9::assert_equal!(
        GET_QUEUE_CONFIG.doc(),
        Some(
            " Resolve the queue config for an outgoing message.
 Multiple handlers may participate; the first non-empty
 result wins.
"
        )
    );
}

#[test]
fn declare_event_captures_return_doc() {
    k9::assert_equal!(
        GET_QUEUE_CONFIG.return_doc(),
        Some("The QueueConfig that drives delivery scheduling for this message.")
    );
}

#[test]
fn declare_event_captures_per_param_docs_and_names() {
    let params = GET_QUEUE_CONFIG.params();
    let names: Vec<String> = params.iter().map(|p| p.name.clone()).collect();
    let docs: Vec<Option<&'static str>> = params.iter().map(|p| p.doc).collect();
    k9::assert_equal!(names, vec!["domain".to_owned(), "tenant".to_owned()]);
    k9::assert_equal!(
        docs,
        vec![
            Some(" Fully-qualified destination domain.\n"),
            Some(" Optional tenant identifier from message metadata.\n"),
        ]
    );
}

#[test]
fn declare_event_doc_metadata_is_optional() {
    k9::assert_equal!(ON_RESET.doc(), None);
    k9::assert_equal!(ON_RESET.return_doc(), None);
    k9::assert_equal!(ON_RESET.params().len(), 0);
}
