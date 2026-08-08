# 1. Secret Service over D-Bus for the Linux session store

Date: 2026-07-31

## Status

Accepted

## Context

The warframe.market session has to survive the app closing, or the user logs in
by hand every run. On Windows it lives in the Credential Manager. Linux had no
store at all: the save refused, the load reported nothing, and the UI hid the
"remember" control.

Linux offers four places to put it.

The **Secret Service D-Bus interface** is what gnome-keyring, KWallet and
KeePassXC all implement, so one client reaches every mainstream desktop. It
needs a provider to be running, which not every machine has.

**libsecret** speaks that same interface, and needs the same provider. It adds a
C library, GLib and GIO to link against, and no capability the pure-Rust client
lacks.

The **kernel keyring** (keyutils) needs no daemon at all, but its keys do not
survive a logout, which is the one thing a persisted session exists to do.

An **encrypted file** would have to keep its key next to the ciphertext, which
buys obfuscation and calls it encryption.

Ruling out the last two leaves the choice of client. It is not a neutral one
here: FrameForge ships as an AppImage, and bundling a native library into one
has already broken a release, where the bundled libwayland-client stopped EGL
from starting.

## Decision

Use the `secret-service` crate over zbus with the `rt-async-io-crypto-rust`
feature. Nothing links libsecret or OpenSSL; the protocol and its session
encryption are pure Rust.

The `keyring` crate would be the obvious front door and is deliberately not
used. It abstracts over per-platform backends, but the Windows path here is
hand-written `windows-sys` FFI and does not go through it, so the abstraction
would wrap one platform and be bypassed on the other. Its Secret Service backend
also derives item attributes from a fixed `(service, user)` pair, with no room
for the email attribute that makes an entry identifiable in Seahorse.

Because availability is a property of the machine and not the target triple,
`get_platform_capabilities` asks the running system whether a provider answers,
rather than reporting a compile-time constant.

## Consequences

A machine with no Secret Service provider cannot persist a session, and the UI
hides the control rather than offering a save that can only fail. Users of a
manually unlocked vault such as KeePassXC get an unlock prompt at launch, but
only if they saved a session in the first place: the lookup reports locked
matches without opening them, so a user who never opted in is never asked.

A missing default keyring is reported rather than fixed. Creating one is a
password-manager decision with its own setup prompt, and a companion app should
not make it on the user's behalf.

Swapping backends later means rewriting three functions and the probe, so this
is cheap to revisit if the pure-Rust client proves unable to keep up with the
providers.
