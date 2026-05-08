---
title: Events
---

# Type-safe events

Most embeddings have a few well-known extension points: "the
script will define a function called `on_request`, and the host
will call it for each incoming HTTP request."  Shingetsu calls
these *events*, and gives you a typed registration / dispatch API
so that the host knows the parameter and return types statically
and the compiler can validate that the script's handler matches.

The whole feature lives in `shingetsu::callback` and the
`declare_event!` macro.

## Declaring an event

A single-handler event:

```rust
use shingetsu::declare_event;

#[derive(shingetsu::LuaTable)]
struct Request {
    method: String,
    path: String,
}

#[derive(shingetsu::LuaTable)]
struct Response {
    status: i64,
    body: String,
}

declare_event! {
    /// Called once per inbound HTTP request.
    #[returns = "the response to send back"]
    pub static ON_REQUEST: Single(
        "on_request",
        /// the request that arrived
        request: Request,
    ) -> Response;
}
```

`declare_event!` expands to a `LazyLock<CallbackSignature<A, R>>`.
`A` is the tuple of parameter types, `R` is the return type.
Doc-comments are captured and surface in any documentation the host
generates from the event registry.

Two flavours:

- `Single(...)` — at most one handler per name, replacing the
  previous registration.
- `Multiple(...)` — multiple handlers.  Dispatch runs them in
  registration order and returns the first non-empty result.

## Wiring it up at startup

Each declared signature needs to be registered against the
`GlobalEnv` so that the registry knows the name is statically
declared (which lets it suggest near-misses when a script
registers a typo):

```rust
use shingetsu::GlobalEnv;

fn build_env() -> GlobalEnv {
    let env = GlobalEnv::new();
    ON_REQUEST.register(&env);
    env
}
```

You also need to expose *some* function for scripts to register
their handlers under.  Convention is `host.on(name, handler)` or
similar — implement it as a free function in a module:

```rust
use shingetsu::{module, Function, VmError};

#[module(name = "host")]
mod host_mod {
    use shingetsu::{callback::callback_registry, CallContext, Function, VmError};

    /// Register a handler for a host event.
    #[function]
    fn on(ctx: CallContext, name: String, handler: Function) -> Result<(), VmError> {
        let registry = callback_registry(&ctx.global);
        registry.register(name, handler).map(|_| ())
    }
}
```

A script can now write:

```lua
host.on("on_request", function(req)
    return { status = 200, body = "hello, " .. req.path }
end)
```

The compile-time type checker uses the declared signature to verify
the handler's parameter and return types match.

## Dispatching from the host

When the host wants to call into the script, use the signature's
`call` method:

```rust
use shingetsu::{GlobalEnv, VmError};

async fn handle(env: &GlobalEnv, req: Request) -> Result<Option<Response>, VmError> {
    let disposition = ON_REQUEST.call(env, (req,)).await?;
    Ok(disposition.result)
}
```

`call` returns a `CallbackDisposition<R>` with three fields:

- `handler_was_defined: bool` — was anything registered?
- `result: Option<R>` — what the handler returned (or `None` if no
  handler ran or it returned no values).
- `event_name: Bytes` — the name, for logging.

The argument is a tuple matching the declared parameter list.  For
a single-parameter signature you still pass a one-tuple, hence the
trailing comma in `(req,)`.

For return types that have a sensible default — `bool`, numeric
types, `String`, `Option<T>`, anything implementing `Default` —
`CallbackDisposition::or_default()` collapses the three states
(no handler, handler returned nothing, handler returned a value)
into the result alone:

```rust
let should_log: bool = SHOULD_LOG.call(env, (1,)).await?.or_default();
```

For return types where "no handler" is genuinely an error — the
host cannot proceed without a script-supplied value —
`CallbackDisposition::require_value()` returns `Result<R, VmError>`
instead, with descriptive errors for both the "undefined" and
"defined but returned nothing" cases.

## Name policies

Name policies kick in at *registration* time — when something
(usually a script, via the host's `host.on(...)` shim) calls
`registry.register(name, handler)`.  Dispatch (`call(...)`) does
not consult the policy; it simply looks up the name and finds a
handler or doesn't.

By default the registry is in `OpenWithSuggestions` mode: any
name is accepted, but a name close to a known one yields a "did
you mean" suggestion in the registration outcome.  You can
change the policy at startup:

```rust
use shingetsu::callback::{callback_registry, NamePolicy};

callback_registry(&env).set_policy(NamePolicy::Closed);
```

The three settings:

- **`Closed`** — every name must be statically declared.
  Registering an unknown name is a hard error and never inserts
  a handler.  Use this for tightly-controlled embeddings where
  the set of events is fixed.
- **`OpenWithSuggestions`** (default) — unknown names are
  accepted, but the `RegisterOutcome` carries a "did you mean"
  suggestion when the name is close to a known one.  Surface
  the suggestion as a warning if you want users to notice
  typos without blocking them.
- **`Open`** — unknown names accepted silently.  Right when the
  host genuinely allows arbitrary user-defined event names.

Under `Closed` mode, a script that misspells a known event sees
the error during registration:

```lua
-- ON_REQUEST is declared with name "on_request".
host.on("on_requst", function(req)        -- typo!
    return { status = 200, body = "" }
end)
-- error in 'callback': 'on_requst' is not a recognised event
--   name. did you mean: 'on_request'?
```

The error surfaces because the host's `host.on(...)` function
returns the registry's `Err` to Lua — nothing else fires it.
A host that wants the same behaviour under `OpenWithSuggestions`
can inspect `RegisterOutcome::Novel { suggestion }` and decide
for itself whether to log, warn, or reject.

For names that only exist at runtime (a user creates a workflow,
the workflow's name becomes an event), use
`registry.register_user_defined(...)` to skip the suggestion
check for that specific call.  The name is added to the dynamic
set so subsequent typo-registrations against that newly-known
name still surface a suggestion.

## Multi-handler events

`Multiple` events let several scripts (or several handlers in the
same script) listen to the same name.  Dispatch walks the
handlers in registration order and stops at the first one that
returns a value; subsequent handlers are *not* called.  A handler
that opts out by not returning anything (a bare `return` or
falling off the end of the function) lets the next one try.  If
every handler opts out, `disposition.result` is `None`.

This is exactly the chain-of-responsibility pattern: each
handler is asked in turn, and the first one with an opinion
wins.  A worked example — resolving a name to an address, where
most handlers know about a small set of names and a final
handler is the catch-all:

```rust
use shingetsu::declare_event;

declare_event! {
    /// Resolve a name to an address.  Handlers run in registration
    /// order; the first one that returns a value wins.
    pub static RESOLVE: Multiple(
        "resolve",
        /// the name being resolved
        name: String,
    ) -> String;
}
```

Script side:

```lua
-- Specific overrides come first.
host.on("resolve", function(name)
    if name == "localhost" then
        return "127.0.0.1"
    end
    -- not a name we recognise; fall through with no return
end)

-- Catch-all fallback.
host.on("resolve", function(name)
    return "0.0.0.0"
end)
```

Host side:

```rust
let addr = RESOLVE
    .call(env, ("localhost".to_owned(),))
    .await?
    .or_default();
assert_eq!(addr, "127.0.0.1");      // first handler matched

let addr = RESOLVE
    .call(env, ("unknown.host".to_owned(),))
    .await?
    .or_default();
assert_eq!(addr, "0.0.0.0");        // first opted out, fallback wins
```

If the script registered only the first handler and called
`resolve("unknown.host")`, dispatch would walk past the bare
`return`, find no further handlers, and the disposition's
`result` would be `None` — which `or_default()` collapses to
the empty string.

## Why bother?

A host that just wanted scripts to call back into it could skip
the whole machinery: define a global, let scripts assign a
function to it, and call that function directly.  `declare_event!`
adds enough on top of that approach to be worth it whenever the
set of callbacks is something users register *by name*:

- **The signature is explicit.**  Parameter names, types, return
  type, and doc-comments are part of the declaration, not
  implicit in whatever the host happens to call.  Documentation
  and `--list` output pick them up automatically.
- **The script's handler is checked at compile time.**  The
  type checker validates the function passed to `host.on(...)`
  against the declared signature — wrong parameter count, wrong
  argument types, wrong return type, all caught before the
  script runs.
- **Misspellings become structured feedback.**  A typo in an
  event name produces a "did you mean ...?" hint (or, under
  `Closed` policy, an outright error) instead of dead code
  that silently never fires.
- **Multiple handlers are first-class.**  The chain-of-
  responsibility pattern shown above is one line of declaration
  and one method call to dispatch; rolling it by hand against a
  list of globals is harder to read and harder to test.

For one-off internal callbacks where you control both sides — a
`#[lua_method]` that takes a `Function` parameter, for example,
or a globally-registered helper that only host code calls — a
plain `Function` value is simpler and entirely fine.  Reach for
`declare_event!` when scripts will register handlers against
stable, named extension points.
