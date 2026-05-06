---
title: Shingetsu
---

# Shingetsu

*Shingetsu* (新月, "new moon") is a small scripting language and runtime
designed to be embedded inside larger programs. It speaks a blend of
[Lua 5.5](https://www.lua.org/) and
[Luau](https://luau.org/) syntax, and is built to run thousands of
short, concurrent scripts inside a single host application without
fighting over memory or stalling each other.

If you have written Lua before, Shingetsu will feel familiar — there
are some differences, which are called out in admonition boxes
throughout the [Syntax guide](syntax/index.md). If you have not, the
syntax guide is written so you can read it cold.

## What is it for?

Shingetsu is aimed at host applications that want to expose
configuration, automation hooks, or per-request scripting to their
users. The host writes its application in Rust; Shingetsu lets users
of that application write small programs in a friendly scripting
language that drives the Rust code.

Some examples of the sort of thing it is good at:

- Per-message rules in a mail server.
- Per-request handlers in a web service.
- Configuration files that are scripts rather than static data.
- Plugins or extensions to a larger Rust application.

## Project goals

The design choices behind Shingetsu come from a few core goals:

- **A familiar surface.** Lua-shaped syntax means a lot of people can
  read and write Shingetsu without learning a new language from
  scratch.
- **Scale to many concurrent scripts.** A single compiled environment
  is shared across many lightweight script tasks. Starting a new
  script is cheap.
- **No hidden cost when crossing into the host.** Host objects are
  passed in and out of scripts without copies and without extra
  allocations.
- **Predictable cleanup.** Resources held by a script are released as
  soon as the script lets go of them — there is no waiting for a
  garbage collector to notice.
- **Sandboxed by default.** A script can only see what the host
  explicitly hands it. There is no automatic access to the file
  system, network, or process environment.
- **Suspend and resume.** Scripts can call into asynchronous host
  functions, pause while the host does work, and pick up where they
  left off when the result is ready.
- **Useful error messages.** Both compile-time and runtime errors
  point at the source location and explain what went wrong in
  human-readable terms.

## Project non-goals

To keep the project focused, a few things are explicitly out of scope:

- Compatibility with Lua's C API.
- Byte-for-byte identical error messages to reference Lua.
- The `coroutine` library. (Asynchronous host calls fill the same
  niche.)

## How the documentation is organised

- The [Syntax guide](syntax/index.md) walks through the language
  itself: the values you can manipulate, how to write expressions and
  statements, how to define and call functions, and so on.
- The [Reference](reference/index.md) lists every built-in module and
  function that the standard environment provides, with the type
  signature and behaviour of each one. This section is generated from
  the implementation, so it always matches the running code.
