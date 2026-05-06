---
title: Syntax guide
---

# Syntax guide

This section describes the Shingetsu language. It is self-contained:
you do not need to have used Lua or Luau before to follow it. Where
Shingetsu differs from one or the other, that difference is called out
in a box like this:

!!! note "For Lua and Luau users"

    Shingetsu borrows most of its syntax from Lua 5.5 and most of its
    type-annotation syntax from Luau. Boxes like this one note where
    Shingetsu does something different from either.

## Pages in this section

- [Comments](comments.md) — how to write notes that the language ignores.
- [Values and types](values-and-types.md) — the kinds of data a script
  can hold and pass around.
- [Variables and scope](variables-and-scope.md) — naming values and
  controlling where those names are visible.
- [Operators](operators.md) — arithmetic, comparison, logical, and
  string operators.
- [Strings](strings.md) — writing string literals, including
  multi-line forms and escapes.
- [Tables](tables.md) — the all-purpose container type used for
  arrays, records, and more.
- [Control flow](control-flow.md) — `if`, loops, `break`, `continue`,
  and `return`.
- [Functions](functions.md) — defining and calling functions, methods,
  multiple returns, and varargs.
- [Type annotations](type-annotations.md) — optional Luau-style
  annotations for values, parameters, and return types.
- [Metatables](metatables.md) — customising how a value responds to
  built-in operations.
- [Error handling](error-handling.md) — raising errors and catching
  them with protected calls.
- [Modules](modules.md) — splitting a program across files with
  `require`.
- [Linting and diagnostics](linting.md) — the checks the compiler
  runs and how to tune their severity from a script.
