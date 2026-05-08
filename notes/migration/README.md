# Migration playbooks

This directory contains the operational playbooks for moving
existing mlua-based hosts onto `shingetsu` via the
`shingetsu_migrate` facade.  The design spec and phased plan
live next to these playbooks in [`MIGRATE.md`](MIGRATE.md);
these documents are the per-host and post-migration recipes that
the spec's Phase 8 references.

## Documents

- [`MIGRATE.md`](MIGRATE.md) -- design spec and phased plan.
  The other documents in this directory reference it for
  authoritative rules (§3.7 patterns A--D, §6 memoization).
- [`kumomta.md`](kumomta.md) -- order of operations for migrating
  kumomta, walking through the registry-key fixup, conversion
  derives, manual serde call sites, userdata blocks, event
  registry, mod-memoize port, and config pool integration.
- [`wezterm.md`](wezterm.md) -- order of operations for migrating
  wezterm, with the `wezterm-event-` to `host-event-` fixup,
  the `luahelper` to `DynamicLua<T>` conversion, the
  `__wezterm_to_dynamic` metamethod retirement, and the
  recommended dependency-order rewrite of the userdata tree
  (leaves first, `Window` last).
- [`final-removal.md`](final-removal.md) -- recipe for removing
  the migration facade once the host playbook has run to
  completion.  Search-and-replace, feature-flag flip, dependency
  removal.

Both playbooks reference §3.7 (canonical migration patterns A--D)
and §6 (memoization readiness) of `MIGRATE.md` for the
authoritative rules.  The playbooks sequence and contextualise
those rules; they do not restate them.

## Reading order

If you're migrating a host:

1. Read [`MIGRATE.md`](MIGRATE.md) end-to-end first.  In
   particular: §1 (goals and constraints), §3.7 (patterns),
   §6 (memoization).
2. Read your host's playbook here.
3. Use [`final-removal.md`](final-removal.md) once your host
   playbook is complete.

If you're working on the facade itself:

1. [`MIGRATE.md`](MIGRATE.md) is the spec.
2. The playbooks are validation: a change to the facade that
   breaks a documented playbook step is a regression.
