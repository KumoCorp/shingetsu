use std::sync::Arc;

use bytes::Bytes;
use dashmap::DashMap;
use parking_lot::RwLock;

use crate::{
    error::VmError,
    function::NativeFunction,
    proto::Proto,
    task::Task,
    value::Value,
};

/// Shared compiled environment.  Cheap to clone (Arc-backed).
/// `Send + Sync`; safe to share across threads and async tasks.
#[derive(Clone)]
pub struct GlobalEnv(pub(crate) Arc<GlobalEnvInner>);

pub(crate) struct GlobalEnvInner {
    /// Global variable table.  Fine-grained sharded locking: concurrent
    /// readers never block each other; a write only locks the relevant shard.
    pub(crate) globals: DashMap<Bytes, Value>,
    /// Loaded top-level prototypes.
    #[allow(dead_code)]
    pub(crate) protos: RwLock<Vec<Arc<Proto>>>,
    /// Registered native functions (also inserted into `globals`).
    pub(crate) natives: DashMap<Bytes, Arc<NativeFunction>>,
}

impl GlobalEnv {
    pub fn new() -> Self {
        GlobalEnv(Arc::new(GlobalEnvInner {
            globals: DashMap::new(),
            protos: RwLock::new(Vec::new()),
            natives: DashMap::new(),
        }))
    }

    /// Set a global variable by name.
    pub fn set_global(&self, name: impl Into<Bytes>, value: Value) {
        self.0.globals.insert(name.into(), value);
    }

    /// Get a global variable by name.
    pub fn get_global(&self, name: &[u8]) -> Option<Value> {
        self.0.globals.get::<[u8]>(name).map(|v| v.clone())
    }

    /// Register a native function as a global.
    pub fn register_native(&self, func: NativeFunction) {
        let name = func.signature.name.clone();
        let func = Arc::new(func);
        self.0
            .globals
            .insert(name.clone(), Value::Function(crate::function::Function::native((*func).clone())));
        self.0.natives.insert(name, func);
    }

    /// Create a task that calls the named global function with the given args.
    pub fn task(&self, function: &str, args: Vec<Value>) -> Result<Task, VmError> {
        let name = Bytes::copy_from_slice(function.as_bytes());
        let func = self
            .0
            .globals
            .get(&name)
            .map(|v| v.clone())
            .ok_or_else(|| VmError::CallNonFunction { type_name: "nil" })?;
        match func {
            Value::Function(f) => Ok(Task::new(self.clone(), f, args)),
            other => Err(VmError::CallNonFunction {
                type_name: other.type_name(),
            }),
        }
    }

    /// Run a cycle-collection pass over `Table` and `Function` values.
    /// Phase 1 stub — no cycle tracking yet.
    pub fn collect_cycles(&self) {
        // Phase 3 will implement GcHeader and tri-color mark-and-sweep.
    }
}

impl Default for GlobalEnv {
    fn default() -> Self {
        Self::new()
    }
}
