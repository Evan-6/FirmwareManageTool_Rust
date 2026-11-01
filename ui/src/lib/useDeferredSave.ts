// Settings used to be written only when the user found the right button, so a
// tweaked slider or path silently vanished on restart. Every page now persists
// through the store as soon as an edit settles; this hook is the "settles" part.
import { useEffect, useRef } from "react";

/**
 * Runs `save` once `deps` stop changing for `delay` ms.
 *
 * `ready` gates the first write: hydrating a form from `settings_load` must not
 * be written straight back (that would race a slow load and clobber the file).
 * The very first ready render is only priming the baseline.
 */
export function useDeferredSave(ready: boolean, deps: unknown[], save: () => void, delay = 600) {
  const saveRef = useRef(save);
  saveRef.current = save;
  const primed = useRef(false);

  useEffect(() => {
    if (!ready) return;
    if (!primed.current) {
      primed.current = true;
      return;
    }
    const handle = window.setTimeout(() => saveRef.current(), delay);
    return () => window.clearTimeout(handle);
    // `deps` comes from the call site and keeps a fixed length there; the ESLint
    // hook rule cannot see through it, hence the manual list.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ready, delay, ...deps]);
}
