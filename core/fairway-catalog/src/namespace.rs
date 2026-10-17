//! Declared namespaces and the operations on their values.

use std::{
    borrow::Cow,
    collections::BTreeMap,
    sync::{OnceLock, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard},
};

use crate::{
    Error, address,
    pattern::Pattern,
    registry::{self, Builtin, Store},
};

type Values<V> = BTreeMap<Cow<'static, str>, V>;

/// A named part of the catalog: values of type `V` by address.
///
/// Declared by [`namespace!`](crate::namespace), which also chooses how
/// [`initialize`](crate::initialize) fills it. Addresses are relative to the
/// namespace: `lorem/ipsum` in the namespace `alpha-items` is the full address
/// `alpha-items/lorem/ipsum`. Changes are made in place under the namespace's
/// lock and last for the rest of the process.
///
/// # Panics
///
/// Every operation panics if [`initialize`](crate::initialize) has not succeeded.
pub struct Namespace<V> {
    name: &'static str,
    initial: Initial<V>,
    values: OnceLock<RwLock<Values<V>>>,
}

/// How [`initialize`](crate::initialize) fills a namespace.
enum Initial<V> {
    /// The namespace starts empty.
    Empty,
    /// Builtin tables for this namespace, with the conversion of their bytes.
    Builtin(fn(&'static [u8]) -> V),
    /// The values returned by the owner's function.
    Function(fn() -> Vec<(String, V)>),
}

impl<V> Namespace<V> {
    #[doc(hidden)]
    pub const fn new(name: &'static str) -> Self {
        Self::with(name, Initial::Empty)
    }

    #[doc(hidden)]
    pub const fn builtin(name: &'static str) -> Self
    where
        V: From<&'static [u8]>,
    {
        Self::with(name, Initial::Builtin(V::from))
    }

    #[doc(hidden)]
    pub const fn initial(name: &'static str, initial: fn() -> Vec<(String, V)>) -> Self {
        Self::with(name, Initial::Function(initial))
    }

    const fn with(name: &'static str, initial: Initial<V>) -> Self {
        address::valid_name(name);
        Self {
            name,
            initial,
            values: OnceLock::new(),
        }
    }

    /// Returns a copy of the value at `address`, or `None` if there is none.
    ///
    /// The copy is made under the read lock. For a builtin `Cow::Borrowed`
    /// value only the reference is copied.
    pub fn get(&self, address: &str) -> Option<V>
    where
        V: Clone,
    {
        self.read().get(address).cloned()
    }

    /// Adds or replaces the value at `address` and returns the previous one.
    ///
    /// # Errors
    ///
    /// Returns an error if `address` has an empty part or contains `*`.
    pub fn put(&self, address: &str, value: V) -> Result<Option<V>, Error> {
        self.check(address)?;
        let address = Cow::Owned(address.to_owned());
        // The previous value is freed by the caller, after the lock is released.
        let previous = self.write().insert(address, value);
        Ok(previous)
    }

    /// Removes the value at `address` and returns it.
    ///
    /// A builtin value is removed from the namespace; its bytes stay in the
    /// program.
    ///
    /// # Errors
    ///
    /// Returns an error if `address` has an empty part or contains `*`.
    pub fn delete(&self, address: &str) -> Result<Option<V>, Error> {
        self.check(address)?;
        let removed = self.write().remove_entry(address);
        Ok(removed.map(|(_, value)| value))
    }

    /// Returns the addresses matching `pattern` in lexicographic order.
    ///
    /// `*` matches any characters, including `/`; other characters are
    /// literal. `"*"` lists the whole namespace, and `"lorem/*"` lists every
    /// address that starts with `lorem/`.
    pub fn list(&self, pattern: &str) -> Vec<String> {
        let pattern = Pattern::new(pattern);
        self.read()
            .keys()
            .filter(|address| pattern.matches(address))
            .map(|address| address.to_string())
            .collect()
    }

    fn check(&self, address: &str) -> Result<(), Error> {
        address::check(address).map_err(|reason| {
            Error::new(format!(
                "address {:?} {reason}",
                address::full(self.name, address)
            ))
        })
    }

    // A panic cannot leave a map half-changed: each write is one map
    // operation. A poisoned lock therefore still holds consistent values.
    fn read(&self) -> RwLockReadGuard<'_, Values<V>> {
        self.values().read().unwrap_or_else(PoisonError::into_inner)
    }

    fn write(&self) -> RwLockWriteGuard<'_, Values<V>> {
        self.values()
            .write()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn values(&self) -> &RwLock<Values<V>> {
        self.values
            .get()
            .expect("catalog is not initialized successfully")
    }
}

impl<V: Send + Sync + 'static> Store for Namespace<V> {
    fn name(&self) -> &'static str {
        self.name
    }

    fn builtin(&self) -> bool {
        matches!(self.initial, Initial::Builtin(_))
    }

    fn prepare(
        &'static self,
        builtins: &[&Builtin],
        source: &'static str,
        problems: &mut Vec<String>,
    ) -> Box<dyn FnOnce()> {
        let mut entries = Vec::new();
        match self.initial {
            Initial::Empty => {}
            Initial::Builtin(from) => {
                for builtin in builtins {
                    for &(address, bytes) in builtin.entries {
                        entries.push((Cow::Borrowed(address), builtin.source, from(bytes)));
                    }
                }
                registry::check("builtin", self.name, &mut entries, problems);
            }
            Initial::Function(initial) => {
                for (address, value) in initial() {
                    entries.push((Cow::Owned(address), source, value));
                }
                registry::check("initial", self.name, &mut entries, problems);
            }
        }
        let values: Values<V> = entries
            .into_iter()
            .map(|(address, _, value)| (address, value))
            .collect();
        Box::new(move || {
            self.values.get_or_init(|| RwLock::new(values));
        })
    }
}
