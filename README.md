# Shingetsu

*Shingetsu* (新月, "new moon") is a Lua compiler and virtual machine
embedded in a Rust async runtime.  It is designed for pervasive use of
host-owned Rust objects exposed to Lua as userdata, with thousands of
concurrent Lua tasks sharing a single initialised environment.

## Goals

- Compiles a blend of Lua 5.5 and LuaU syntax source to bytecode at load
  time; execute that bytecode in a lightweight per-task VM.
- Share a single compiled, initialised environment across many
  concurrent async tasks without copying.
- Pass host Rust objects into Lua with zero copies and zero extra
  allocations; return results from Lua to the host with the same
  constraint.
- Release all resources (host objects, large strings) immediately when
  Lua code releases its last reference — no deferred GC for leaf
  values.
- Sandbox Lua code naturally: the VM exposes nothing to scripts beyond
  what the host explicitly provides.
- Support async host functions that can suspend a Lua task and resume
  it when the host operation completes.
- Excellent diagnostic error messages for both compile time and runtime errors

## Non-Goals

The following are explicitly out of scope:

- lua C API compatibility
- bit-for-bit compatibility with lua error messages
- The `coroutine` library


