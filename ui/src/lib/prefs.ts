// Preferences that belong to the window, not to the device: which macro the
// user last worked with. `Settings` (src/settings.rs) has no field for these and
// adding one would need a Rust change, so they live in localStorage next to the
// bundle instead. Everything the worker needs still goes through settings_save.
const PREFIX = "fmt.";

const storage = (): Storage | null => {
  try {
    return window.localStorage;
  } catch {
    return null; // privacy mode / disabled storage
  }
};

export function loadPref(key: string, fallback: string): string {
  try {
    return storage()?.getItem(PREFIX + key) ?? fallback;
  } catch {
    return fallback;
  }
}

export function savePref(key: string, value: string): void {
  try {
    if (value) storage()?.setItem(PREFIX + key, value);
    else storage()?.removeItem(PREFIX + key);
  } catch {
    // A full or disabled localStorage must never break the UI.
  }
}
