//! Hook for typed localStorage persistence with automatic save on change.

use serde::{de::DeserializeOwned, Serialize};
use yew::prelude::*;

/// Return value from the use_local_storage hook.
pub struct UseLocalStorage<T: Clone + PartialEq + 'static> {
    /// Current value
    pub value: T,
    /// Set a new value (automatically persists to localStorage)
    pub set: Callback<T>,
}

/// Load a value from localStorage
fn load_from_storage<T: DeserializeOwned + Default>(key: &str) -> T {
    web_sys::window()
        .and_then(|w| w.local_storage().ok().flatten())
        .and_then(|storage| storage.get_item(key).ok().flatten())
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default()
}

/// Save a value to localStorage
fn save_to_storage<T: Serialize>(key: &str, value: &T) {
    if let Some(storage) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
        if let Ok(json) = serde_json::to_string(value) {
            let _ = storage.set_item(key, &json);
        }
    }
}

/// Hook for managing state that persists to localStorage.
///
/// The value is loaded from localStorage on mount and saved whenever it changes.
/// If no value exists in localStorage, the default value is used.
///
/// # Arguments
/// * `key` - The localStorage key to use
///
/// # Returns
/// * `UseLocalStorage<T>` - The current value and a callback to update it
///
/// # Example
/// ```ignore
/// let storage = use_local_storage::<HashSet<Uuid>>("my-key");
/// // Access current value
/// let current = &storage.value;
/// // Update value (automatically persists)
/// storage.set.emit(new_value);
/// ```
#[hook]
pub fn use_local_storage<T>(key: &'static str) -> UseLocalStorage<T>
where
    T: Clone + PartialEq + Serialize + DeserializeOwned + Default + 'static,
{
    let state = use_state(|| load_from_storage::<T>(key));

    let set = {
        let state = state.clone();
        Callback::from(move |new_value: T| {
            save_to_storage(key, &new_value);
            state.set(new_value);
        })
    };

    UseLocalStorage {
        value: (*state).clone(),
        set,
    }
}
