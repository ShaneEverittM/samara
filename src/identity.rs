//! Program-issued component identity and optional application subscription keys.
use std::{
    any::{Any, TypeId},
    cmp::Ordering,
    fmt,
    hash::{Hash, Hasher},
    sync::Arc,
};

use crate::{CapabilityToken, SourceCapability, SourceDescriptor};

/// An opaque component identity within one Program.
///
/// Obtained from [`crate::ComponentRef::id`]. The builder issues identities;
/// optional registration names are diagnostics and do not affect equality.
/// Addresses also carry Program provenance, so IDs alone cannot send messages.
#[derive(Clone, Debug)]
pub struct ComponentId {
    index: u64,
    name: Arc<str>,
}

impl ComponentId {
    pub(crate) fn new(index: u64, name: impl Into<Arc<str>>) -> Self {
        Self {
            index,
            name: name.into(),
        }
    }

    /// Returns the diagnostic name, defaulting to the Component kind's type name.
    pub fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn runtime() -> Self {
        Self::new(u64::MAX, "<runtime>")
    }
    pub(crate) fn external_port() -> Self {
        Self::new(u64::MAX - 1, "<external-port>")
    }
}

impl PartialEq for ComponentId {
    fn eq(&self, other: &Self) -> bool {
        self.index == other.index
    }
}
impl Eq for ComponentId {}
impl Hash for ComponentId {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.index.hash(state);
    }
}
impl PartialOrd for ComponentId {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for ComponentId {
    fn cmp(&self, other: &Self) -> Ordering {
        self.index.cmp(&other.index)
    }
}

trait ApplicationKey: fmt::Debug + Send + Sync {
    fn as_any(&self) -> &dyn Any;
    fn equals(&self, other: &dyn ApplicationKey) -> bool;
    fn hash_key(&self, state: &mut dyn Hasher);
}
impl<K: Eq + Hash + fmt::Debug + Send + Sync + 'static> ApplicationKey for K {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn equals(&self, other: &dyn ApplicationKey) -> bool {
        other.as_any().downcast_ref::<K>() == Some(self)
    }
    fn hash_key(&self, mut state: &mut dyn Hasher) {
        TypeId::of::<K>().hash(&mut state);
        self.hash(&mut state);
    }
}

#[derive(Clone)]
enum Slot {
    Default(CapabilityToken),
    Key(Arc<dyn ApplicationKey>),
}

/// A component-local subscription slot, normally selected by its capability.
///
/// Constructors need no ID. Use [`crate::Subscription::key`] when maintaining
/// multiple inputs through one capability. To select a keyed input in a test,
/// use `SubscriptionId::key(the_same_key)`; for a default input, pass its
/// [`SourceCapability`] directly to the controlled runtime.
///
/// Equality is Program-local and preserves key type and value. Diagnostic
/// formatting never determines identity. Default test selectors additionally
/// retain and validate their capability's Program provenance.
#[derive(Clone)]
pub struct SubscriptionId(Slot);

impl SubscriptionId {
    /// Selects an explicit application key, matching [`crate::Subscription::key`].
    ///
    /// Keys must have pure, stable equality and hashing. Different Rust key
    /// types remain distinct even when their values format identically.
    pub fn key<K: Eq + Hash + fmt::Debug + Send + Sync + 'static>(value: K) -> Self {
        Self(Slot::Key(Arc::new(value)))
    }

    pub(crate) fn for_capability(token: &CapabilityToken) -> Self {
        Self(Slot::Default(token.clone()))
    }

    pub(crate) fn belongs_to(&self, program: &Arc<()>) -> bool {
        match &self.0 {
            Slot::Default(token) => token.belongs_to(program),
            Slot::Key(_) => true,
        }
    }

    pub(crate) fn duplicate_message(&self) -> String {
        match &self.0 {
            Slot::Default(_) => format!(
                "ambiguous default Subscription slot {self:?}; use .key(...) to distinguish multiple subscriptions through the same capability"
            ),
            Slot::Key(_) => format!("duplicate desired Subscription key {self:?}"),
        }
    }
}

impl fmt::Debug for SubscriptionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            Slot::Default(token) => f
                .debug_tuple("DefaultSubscription")
                .field(&token.id)
                .finish(),
            Slot::Key(key) => f.debug_tuple("SubscriptionKey").field(key).finish(),
        }
    }
}
impl PartialEq for SubscriptionId {
    fn eq(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (Slot::Default(left), Slot::Default(right)) => left.id == right.id,
            (Slot::Key(left), Slot::Key(right)) => left.equals(right.as_ref()),
            _ => false,
        }
    }
}
impl Eq for SubscriptionId {}
impl Hash for SubscriptionId {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match &self.0 {
            Slot::Default(token) => {
                0_u8.hash(state);
                token.id.hash(state);
            }
            Slot::Key(key) => {
                1_u8.hash(state);
                key.hash_key(state);
            }
        }
    }
}
impl From<&SubscriptionId> for SubscriptionId {
    fn from(id: &SubscriptionId) -> Self {
        id.clone()
    }
}
impl<S: SourceDescriptor> From<&SourceCapability<S>> for SubscriptionId {
    fn from(capability: &SourceCapability<S>) -> Self {
        Self::for_capability(&capability.token)
    }
}
