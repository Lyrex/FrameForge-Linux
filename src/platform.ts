import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

// ==============================================================================
// Platform capabilities
// ==============================================================================
//
// Encrypted credential storage depends on the machine: a desktop only has
// somewhere to keep a session if it runs a Secret Service provider. The UI
// asks the backend instead of guessing, so the "remember me" control is only
// offered when the command behind it can actually succeed.
//
// The query runs once per app load and every consumer shares that answer.

export type PlatformCapabilities = {
  persistentCredentials: boolean;
};

// Optimistic default for the brief moment before the backend answers.
let current: PlatformCapabilities = { persistentCredentials: true };
const ready = invoke<PlatformCapabilities>("get_platform_capabilities")
  .then(capabilities => (current = capabilities))
  .catch(() => current);

export function usePlatformCapabilities(): PlatformCapabilities {
  const [capabilities, setCapabilities] = useState(current);
  useEffect(() => {
    ready.then(setCapabilities);
  }, []);
  return capabilities;
}
