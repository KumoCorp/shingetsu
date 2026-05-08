---
title: Custom module loaders
---

# Custom module loaders

`Libraries::PACKAGE` enables `require` and ships with a default
loader that searches the filesystem.  The loader is pluggable: you
can replace it with one that reads modules from anywhere — an
in-memory bundle, a database, an HTTP service, an archive embedded
in the host binary.

## The `ModuleLoader` trait

```rust
use shingetsu::{ModuleLoader, LoadedModule, VmError};
use std::path::Path;

#[async_trait::async_trait]
pub trait ModuleLoader: Send + Sync {
    async fn load(&self, name: &str, path: &Path) -> Result<LoadedModule, VmError>;
}
```

`name` is the original argument to `require`.  `path` is one of
the candidate paths produced by expanding `package.path` against
the name.  The runtime calls `load` once per candidate, in order,
and stops at the first success.

A `LoadedModule` carries two things:

- `proto: Arc<Proto>` — the compiled top-level chunk, ready to
  execute.
- `type_info: ModuleTypeInfo` — what the compiler learned about
  the module's exports, so cross-module type checking works.

You produce both by going through the compiler.  The default
implementation (in `shingetsu::module_loader::LuaModuleLoader`) is
a good starting point; for non-filesystem sources you write a
loader that reaches the source bytes some other way and then hands
them to the compiler.

## Sketch: an in-memory bundle

For a host that ships its scripts inside the binary, a loader
might look like this:

```rust
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use shingetsu::{
    GlobalTypeMap, LoadedModule, ModuleLoader, VmError,
    compiler::{CompileOptions, Compiler},
};

pub struct BundledLoader {
    sources: HashMap<PathBuf, &'static str>,
    global_types: GlobalTypeMap,
}

#[async_trait::async_trait]
impl ModuleLoader for BundledLoader {
    async fn load(&self, name: &str, path: &Path) -> Result<LoadedModule, VmError> {
        let source = self.sources.get(path).ok_or_else(|| VmError::HostError {
            name: "require".to_owned(),
            source: anyhow::anyhow!("module {name:?} not bundled at {path:?}").into(),
        })?;

        let opts = CompileOptions {
            debug_info: true,
            source_name: Arc::new(format!("@{}", path.display())),
            type_check: true,
        };
        let compiler = Compiler::new(opts, self.global_types.clone());
        let bc = compiler
            .compile(source)
            .await
            .map_err(|e| VmError::HostError {
                name: "require".to_owned(),
                source: anyhow::anyhow!("compile failed: {e:?}").into(),
            })?;

        Ok(LoadedModule {
            proto: bc.top_level,
            type_info: bc.module_type_info,
        })
    }
}
```

To install it:

```rust
let env = GlobalEnv::new();
shingetsu::register_libs(&env, Libraries::SANDBOXED)?;
env.set_package_path(Some("?.lua".to_string()));
env.set_module_loader(build_bundle());
```

Now `require("greeter")` will look up the path `greeter.lua`
against the bundle, regardless of what is on the filesystem.

## Path templates

`package.path` is a `;`-separated list of templates; every `?` in
a template is replaced with the requested name (with `.` expanded
to the platform path separator):

```text
"./scripts/?.lua;./shared/?.lua;./shared/?/init.lua"
```

`shingetsu_vm::candidate_paths` turns a name and a template string
into the ordered list of candidates.  When writing a custom
loader you usually do not need to call it yourself — the runtime
does, and hands you each candidate path one at a time — but it is
useful for tests and tooling.

## The cache

`require` caches its results in `package.loaded` (or the equivalent
internal cache).  A second `require("greeter")` returns the same
table without re-loading.  The loader is *not* called a second
time, so it is fine to do work in `load` that you would not want
to repeat.

If you need to invalidate a module — for example, while developing
— call `env.set_loaded("greeter", Value::Nil)` to remove the
cached entry, and the next `require` will go back through the
loader.

## Preloaded modules vs. loaded ones

There are two ways for `require("name")` to succeed:

1. **Preloaded** — the module was registered with
   `register_preload` (or its equivalent for hand-rolled modules).
   The opener runs lazily on first `require` and the result is
   cached.  No `package.path` search is performed for preloaded
   names.
2. **Loaded via the loader** — the name is looked up against
   `package.path`, and the configured `ModuleLoader` is asked to
   resolve it.  Cached as in (1).

Preloads always win, which means a host module can shadow a
filesystem module of the same name.  This is usually what you
want: a `host.foo` module that the embedder ships should always
override any `host/foo.lua` a user happened to drop in the working
directory.

## When to bother

For most embeddings the default filesystem loader is fine — it
respects `set_package_path` and the standard `?` templating.
Reach for a custom loader when:

- You want to ship scripts inside the host binary.
- Scripts live in a content-addressable store or a database.
- You need to authorise or audit every `require`.
- You want to pre-resolve and cache compilation across processes
  (e.g. a build step that produces precompiled `LoadedModule`s the
  loader hands back without running the compiler).

In all four cases the loader is a small adapter — a handful of
lines plus whatever your storage layer needs.
