---
title: Shingetsu
---

<div class="sg-hero" markdown>

<div class="sg-hero__mark" aria-hidden="true">
  <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64">
    <defs>
      <radialGradient id="hero-disc" cx="35%" cy="32%" r="78%">
        <stop offset="0%" stop-color="#3A2A78"/>
        <stop offset="100%" stop-color="#0A0524"/>
      </radialGradient>
      <radialGradient id="hero-glow" cx="50%" cy="50%" r="50%">
        <stop offset="0%" stop-color="currentColor" stop-opacity="0.45"/>
        <stop offset="55%" stop-color="currentColor" stop-opacity="0.15"/>
        <stop offset="100%" stop-color="currentColor" stop-opacity="0"/>
      </radialGradient>
      <filter id="hero-noise" x="0" y="0" width="100%" height="100%">
        <feTurbulence type="fractalNoise" baseFrequency="1.4" numOctaves="2" seed="7"/>
        <feColorMatrix values="0 0 0 0 0  0 0 0 0 0  0 0 0 0 0  0 0 0 0.35 0"/>
        <feComposite in2="SourceGraphic" operator="in"/>
      </filter>
      <clipPath id="hero-clip"><circle cx="32" cy="32" r="20.5"/></clipPath>
    </defs>
    <circle cx="32" cy="32" r="32" fill="url(#hero-glow)"/>
    <circle cx="32" cy="32" r="26.9" fill="none" stroke="currentColor" stroke-width="0.9"/>
    <circle cx="32" cy="32" r="20.5" fill="url(#hero-disc)"/>
    <g clip-path="url(#hero-clip)">
      <rect x="11.5" y="11.5" width="41" height="41" fill="#000" filter="url(#hero-noise)" opacity="0.55"/>
      <ellipse cx="28.3" cy="26.3" rx="6.6" ry="4.5" fill="#3A2A78" opacity="0.35"/>
      <ellipse cx="39.2" cy="33.0" rx="4.5" ry="3.3" fill="#3A2A78" opacity="0.22"/>
      <circle cx="24.8" cy="35.7" r="2.05" fill="#000" opacity="0.45"/>
      <circle cx="24.8" cy="35.7" r="2.05" fill="none" stroke="#3A2A78" stroke-width="0.4" opacity="0.5"/>
      <circle cx="40.6" cy="25.8" r="1.45" fill="#000" opacity="0.4"/>
      <circle cx="34.1" cy="41.2" r="1.25" fill="#000" opacity="0.35"/>
      <circle cx="20.7" cy="29.9" r="0.85" fill="#000" opacity="0.3"/>
      <circle cx="43.3" cy="40.2" r="0.65" fill="#000" opacity="0.3"/>
      <circle cx="31.0" cy="20.7" r="0.75" fill="#000" opacity="0.3"/>
    </g>
  </svg>
</div>

<div class="sg-hero__type">
  <div class="sg-hero__kanji">新月</div>
  <h1 class="sg-hero__name">shingetsu</h1>
  <div class="sg-hero__tagline">A small embeddable scripting language</div>
</div>

</div>

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
- [Differences from Lua and Luau](differences.md) consolidates,
  in one place, every meaningful difference — syntax, standard
  library, runtime model — between Shingetsu and either
  reference Lua 5.4/5.5 or Luau.
- The [Embedding guide](embedding/index.md) is for the host side:
  writing Rust code that compiles and runs scripts, exposes host
  objects to them, and turns Lua values back into Rust values.
- The [Reference](reference/index.md) lists every built-in module and
  function that the standard environment provides, with the type
  signature and behaviour of each one. This section is generated from
  the implementation, so it always matches the running code.
