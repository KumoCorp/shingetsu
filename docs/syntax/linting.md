---
title: Linting and diagnostics
---

# Linting and diagnostics

When Shingetsu compiles your script it does more than check the
syntax. It also runs a collection of *lints* — extra checks that
catch mistakes likely to be bugs even though the code parses cleanly.
Each finding is reported as a *diagnostic* with a source location, a
short message, and a severity.

There are three severities:

- **error** — the script is rejected; nothing runs.
- **warning** — the diagnostic is printed, but the script still runs.
- **allow** — the diagnostic is suppressed entirely (this is what
  you set when you want a particular lint silenced).

Every lint has a built-in default severity. You can override that
default in three places: with a comment in the script itself, with a
CLI flag, or with a project-level configuration file.

## The lints

| Name              | Default | What it catches                                                  |
| ----------------- | ------- | ---------------------------------------------------------------- |
| `unused_variable` | warn    | A local variable that is declared but never read.                |
| `shadowing`       | warn    | A new local that hides another local with the same name.         |
| `unreachable_code`| warn    | Code that can never run (after `return`, `break`, `goto`, …).    |
| `empty_loop`      | warn    | A loop whose body is empty.                                      |
| `call_convention` | warn    | A method defined with `:` called with `.` (or vice versa).       |
| `arg_count`       | error   | A function called with the wrong number of arguments.            |
| `arg_type`        | error   | A function argument whose type doesn't match the parameter.      |
| `return_type`     | error   | A `return` value whose type doesn't match the function's return. |
| `assign_type`     | error   | An assignment whose value doesn't match the target's type.       |
| `field_access`    | error   | Reading or writing a field that doesn't exist on the type.       |
| `missing_return`  | error   | A function that should return a value but doesn't on some path.  |

The type-related lints (`arg_type`, `return_type`, `assign_type`,
`field_access`, `missing_return`) only fire when type checking is
enabled and you have written enough
[type annotations](type-annotations.md) for the compiler to reason
about the code.

!!! tip "Suppressing `unused_variable` with a leading underscore"

    Sometimes a local exists for a reason but is not actually read —
    most often a function parameter you cannot remove because the
    function has to match a particular shape, or the second value of
    a multi-return call where you only need the first. Prefixing the
    name with `_` tells the compiler the omission is deliberate, and
    `unused_variable` is silently skipped:

    ```lua
    local function on_event(_event, payload)
        -- _event is required by the callback shape but unused here
        process(payload)
    end

    local _ok, err = pcall(might_fail)
    if err then handle(err) end
    ```

    A bare `_` works too, and is the conventional name for a value
    you genuinely do not care about. Reach for this before
    `-- shingetsu: allow(unused_variable)` — the underscore is the
    idiom most readers will recognise.

## Adjusting severity from a comment

Two comment forms control diagnostics. They differ only in scope.

### File-level: `--# shingetsu: action(lints…)`

A file-level directive must appear *before any code* — typically as
one of the first lines in the file. It applies to the entire chunk:

```lua
--# shingetsu: allow(shadowing, unused_variable)

local x = 1
local x = 2     -- normally a `shadowing` warning; suppressed here
local _y = 3    -- normally an `unused_variable` warning; suppressed here

print(x)
```

You can repeat the directive on several lines to control more lints:

```lua
--# shingetsu: allow(shadowing)
--# shingetsu: deny(unreachable_code)
```

### Statement-level: `-- shingetsu: action(lints…)`

A statement-level directive (note the *single* `#`-less form) applies
only to the **next statement**. This is the right granularity when
you want to silence one specific spot without loosening the rules
for the whole file:

```lua
-- shingetsu: allow(shadowing)
local x = 2     -- this declaration is allowed to shadow

local x = 3     -- this one is still flagged as usual
```

Use the most local form that fits — a statement-level `allow` is
easier for a future reader to evaluate than a file-wide one.

### The three actions

Each directive uses one of:

- `allow(lint, …)` — suppress the diagnostic entirely.
- `warn(lint, …)` — emit a warning (don't fail the build).
- `deny(lint, …)` — emit an error (fail the build).

Multiple lint names inside a single directive are comma-separated:

```lua
--# shingetsu: deny(unused_variable, shadowing)
```

## Adjusting severity from outside the script

For one-off invocations, the `shingetsu run` command takes
`--allow`, `--warn`, and `--deny` flags. Each accepts a
comma-separated list of lint names:

```
shingetsu run script.lua --allow shadowing,unused_variable
```

For project-wide defaults, drop a `shingetsu.toml` next to your
sources. The compiler walks upward from the script's directory and
picks up the first one it finds:

```toml
[lints]
shadowing = "allow"
empty_loop = "deny"
unused_variable = "warn"
```

## Which override wins?

When several sources disagree about a lint, the most specific one
wins. From highest to lowest priority:

1. A statement-level `-- shingetsu:` directive on the offending
   statement.
2. A file-level `--# shingetsu:` directive in the same file.
3. A `--allow` / `--warn` / `--deny` flag on the command line.
4. A `[lints]` entry in `shingetsu.toml`.
5. The compiled-in default severity for that lint.

In practice, this means you can lock down a project to error-level
in `shingetsu.toml`, then carve out narrow exceptions in individual
files (or lines) where you have a justified reason — and a future
reader can find that justification right next to the code.
