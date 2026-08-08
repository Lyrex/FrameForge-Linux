// ==============================================================================
// Linux overlay placement
// ==============================================================================
//
// The relic band and the riven panel are ordinary Tauri windows that have to
// behave like game overlays: above a fullscreen client, undecorated, never
// stealing focus, and — for the band — invisible to the pointer. On Linux none
// of that comes from Tauri's window options alone. It comes from EWMH
// properties set on the X11 window, which this module writes.
//
// Two facts drive the whole design.
//
// The first is ordering. KWin reads `_NET_WM_WINDOW_TYPE` when it takes a
// window under management and never looks again, so a property written to a
// window that is already on screen changes nothing: the overlay sits under the
// game while `xprop` reports the property present. The window has to be
// unmapped when the properties are written and mapped afterwards. Since the X11
// id only exists once the window has been realised, that makes the full
// sequence show, unmap, write, map.
//
// The second is that a window type is enough on its own. `_NET_WM_STATE_ABOVE`
// cannot win against a fullscreen client, because KWin promotes the focused
// fullscreen window above every keep-above window. `_NET_WM_WINDOW_TYPE_DOCK`
// loses for the same reason. KDE's critical-notification type lands in a layer
// above both, which is why it is worth a KDE-specific atom.

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use tauri::WebviewWindow;
use xcb::{x, Xid, XidNew};

use crate::ocr::x11_connect;

// ==============================================================================
// The window type
// ==============================================================================

fn window_type_atom() -> &'static str {
    window_type_atom_from(|key| std::env::var(key).ok())
}

/// The window type that reaches a layer above a fullscreen game.
///
/// Only KDE needs a type of its own. Everything else this module writes is
/// standard EWMH and needs no per-compositor branch.
///
/// Written over an environment reader rather than the real environment so it can
/// be exercised without `set_var` racing other tests.
///
/// TODO: a compositor that refuses `_NET_WM_WINDOW_TYPE_DOCK` above a fullscreen
/// client needs a branch of its own here, and enough detection to tell it apart.
/// Nothing outside KDE has been run against a game yet.
fn window_type_atom_from(env: impl Fn(&str) -> Option<String>) -> &'static str {
    // The desktop name survives in a user's environment long after they stop
    // running that desktop, and handing Hyprland an atom only KWin knows would
    // leave the overlay an ordinary window under the game.
    let names_itself = env("HYPRLAND_INSTANCE_SIGNATURE").is_some() || env("SWAYSOCK").is_some();

    // Distributions disagree on case, on KDE versus plasma, and some publish
    // several names joined by colons, so this is a substring test rather than an
    // equality one.
    let desktop = env("XDG_CURRENT_DESKTOP").unwrap_or_default().to_lowercase();
    if !names_itself && (desktop.contains("kde") || desktop.contains("plasma")) {
        "_KDE_NET_WM_WINDOW_TYPE_CRITICAL_NOTIFICATION"
    } else {
        "_NET_WM_WINDOW_TYPE_DOCK"
    }
}

// ==============================================================================
// The hints
// ==============================================================================

/// Properties whose values are themselves atoms, paired with the atom names to
/// resolve into them.
fn atom_hints(window_type: &'static str) -> Vec<(&'static str, Vec<&'static str>)> {
    vec![
        ("_NET_WM_WINDOW_TYPE", vec![window_type]),
        (
            "_NET_WM_STATE",
            vec![
                "_NET_WM_STATE_ABOVE",
                "_NET_WM_STATE_STAYS_ON_TOP",
                "_NET_WM_STATE_SKIP_TASKBAR",
                "_NET_WM_STATE_SKIP_PAGER",
            ],
        ),
    ]
}

/// Properties carrying plain numbers.
///
/// `_NET_WM_USER_TIME` of zero marks the map as not user-initiated, which is
/// what stops the freshly mapped overlay from pulling focus off the game.
///
/// TODO: GTK points every toplevel at a second window through
/// `_NET_WM_USER_TIME_WINDOW`, and a window manager honouring that redirection
/// reads user time from there rather than from the window written below. KWin
/// does not advertise it in `_NET_SUPPORTED`, so the zero lands; elsewhere the
/// write has to follow the property.
const CARDINAL_HINTS: &[(&str, &[u32])] = &[
    ("_NET_WM_BYPASS_COMPOSITOR", &[2]),
    ("_NET_WM_USER_TIME", &[0]),
];

/// The five-word Motif structure, with only the decorations field flagged as
/// present and set to none. Belt and braces against a window manager that draws
/// a frame despite Tauri's `decorations: false`.
///
/// Motif hints are typed with the `_MOTIF_WM_HINTS` atom rather than with
/// `CARDINAL`, which is why this is not in the list above. A reader asks for
/// the property by type, so the wrong type does not fail: it returns nothing,
/// and the hint is quietly ignored.
const MOTIF_HINTS: (&str, &[u32]) = ("_MOTIF_WM_HINTS", &[2, 0, 0, 0, 0]);

/// Resolve an atom, creating it if the server has never seen it.
///
/// Not `ocr`'s `x11_atom`, which asks the server for existing atoms only. That
/// is the right question when reading a property — a missing
/// `_NET_CLIENT_LIST_STACKING` means there is no EWMH window manager — but the
/// wrong one when writing. `_NET_WM_STATE_STAYS_ON_TOP` in particular is not
/// part of EWMH and may well be unknown to the server until we name it, and an
/// existence check would silently hand back the null atom.
fn intern(conn: &xcb::Connection, name: &str) -> Result<x::Atom, String> {
    let cookie = conn.send_request(&x::InternAtom {
        only_if_exists: false,
        name: name.as_bytes(),
    });
    conn.wait_for_reply(cookie)
        .map(|reply| reply.atom())
        .map_err(|e| format!("Cannot intern the {name} atom: {e}"))
}

/// Write every hint onto an X11 window.
///
/// Each request is checked rather than fired and forgotten, which also makes
/// the call a round trip: by the time this returns, the properties are on the
/// server and any subsequent map will be managed with them in place.
fn apply_hints(xid: u32, window_type: &'static str) -> Result<(), String> {
    let conn = x11_connect()?;
    let window = x::Window::new(xid);

    let write = |property: &str, r#type: x::Atom, data: &[u32]| -> Result<(), String> {
        let property = intern(&conn, property)?;
        conn.send_and_check_request(&x::ChangeProperty {
            mode: x::PropMode::Replace,
            window,
            property,
            r#type,
            data,
        })
        .map_err(|e| format!("Cannot write an overlay window hint: {e}"))
    };

    for (property, atoms) in atom_hints(window_type) {
        let values = atoms
            .iter()
            .map(|name| intern(&conn, name).map(|atom| atom.resource_id()))
            .collect::<Result<Vec<u32>, String>>()?;
        write(property, x::ATOM_ATOM, &values)?;
    }
    for (property, values) in CARDINAL_HINTS {
        write(property, x::ATOM_CARDINAL, values)?;
    }
    let (motif, structure) = MOTIF_HINTS;
    write(motif, intern(&conn, motif)?, structure)?;
    Ok(())
}

/// Put an overlay window where the caller asked, through X rather than through
/// GTK.
///
/// Tauri's `set_position` on a window that is not yet mapped is a request the
/// window manager answers with its own placement, and the client's value is only
/// applied on a later turn of the GTK main loop. The overlay is shown at the
/// moment the app is busiest, with OCR running on the reward screen, so that
/// turn can be a long time coming, and until it arrives the band sits wherever
/// KWin put it. On a two-monitor desktop that is routinely the wrong monitor.
///
/// A `ConfigureWindow` on our own connection does not queue behind any of that.
///
/// The size rides along because `set_size` defers the same way, and the band is
/// a strip as wide as the game's monitor, so a late size is as visible as a late
/// position.
pub(crate) fn place(window: &WebviewWindow, x: i32, y: i32, width: u32, height: u32) {
    let Some(xid) = xid(window) else { return };
    if let Err(e) = configure(xid, x, y, width, height) {
        eprintln!("[overlay] {}: {e}", window.label());
    }
}

fn configure(xid: u32, x: i32, y: i32, width: u32, height: u32) -> Result<(), String> {
    let conn = x11_connect()?;
    conn.send_and_check_request(&x::ConfigureWindow {
        window: x::Window::new(xid),
        value_list: &[
            x::ConfigWindow::X(x),
            x::ConfigWindow::Y(y),
            x::ConfigWindow::Width(width),
            x::ConfigWindow::Height(height),
        ],
    })
    .map_err(|e| format!("Cannot place the overlay window: {e}"))
}

/// The window's X11 id. `None` means the window is not an X11 window at all,
/// which under `GDK_BACKEND=x11` means it has never been realised.
fn xid(window: &WebviewWindow) -> Option<u32> {
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Xlib(handle) => Some(handle.window as u32),
        RawWindowHandle::Xcb(handle) => Some(handle.window.into()),
        _ => None,
    }
}

/// What should happen to the window once its hints are written.
///
/// The caller states this rather than the window being asked, because asking is
/// what breaks. `tauri://window-created` fires once the window has been handed
/// to GTK to show, so a freshly created panel may report itself either mapped or
/// not depending on how far that has got — and the branch where it reports
/// itself hidden is the one where nobody maps it again afterwards, leaving the
/// hints on a window the compositor already took under management unhinted.
pub(crate) enum AfterHinting {
    /// Leave the window down. The relic band is parked off screen at startup and
    /// the show that brings it up for a real fissure is the map that reads the
    /// hints, so it needs no cycle of its own.
    LeaveHidden,
    /// Put the window back up. The riven panel is on screen from the moment it
    /// exists, so the only map that can read its hints is one we perform.
    ShowAgain,
}

/// Put an overlay window's hints in place so the next map is managed with them.
///
/// The window comes down first in both cases. That is free for a window that is
/// already down, and it is what guarantees the ordering for one that is not: the
/// unmap and the write both precede the map, whichever way the two X connections
/// happen to interleave.
///
/// Every failure here is reported and then swallowed. An overlay that ends up
/// behind the game is worth a line in the log; it is not worth refusing to
/// start over. The one failure that must not pass quietly is a window taken
/// down for hinting and never put back, so that path says so.
pub(crate) fn hint_before_map(window: &WebviewWindow, after: AfterHinting) {
    let Some(xid) = xid(window) else {
        eprintln!("[overlay] {} has no X11 window id, hints skipped", window.label());
        return;
    };

    let _ = window.hide();

    // Off the main thread: the hint writes round-trip to the X server, and the
    // unmap issued above needs the main loop back to reach the server at all.
    let window = window.clone();
    std::thread::spawn(move || {
        if let Err(e) = apply_hints(xid, window_type_atom()) {
            eprintln!("[overlay] {}: {e}", window.label());
            return;
        }
        if matches!(after, AfterHinting::LeaveHidden) {
            return;
        }
        // Position is left to GTK, which re-applies the geometry it was given at
        // creation when the window maps again. Re-asserting it from here would
        // mean reading a position back off a window that may never have been
        // mapped, and a wrong origin puts the panel on the wrong monitor.
        let restored = window.clone();
        let put_back = window.run_on_main_thread(move || {
            let _ = restored.show();
        });
        if let Err(e) = put_back {
            eprintln!(
                "[overlay] {} was hidden for hinting and cannot be shown again: {e}",
                window.label()
            );
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |key: &str| map.get(key).cloned()
    }

    /// The distinction the whole module exists for: DOCK reaches KWin's
    /// AboveLayer, which is below the layer a focused fullscreen window is
    /// promoted into, so on KDE it would leave the overlay under the game.
    #[test]
    fn kde_gets_the_critical_notification_type() {
        let env = env_of(&[("XDG_CURRENT_DESKTOP", "KDE"), ("XDG_SESSION_TYPE", "wayland")]);
        assert_eq!(
            window_type_atom_from(env),
            "_KDE_NET_WM_WINDOW_TYPE_CRITICAL_NOTIFICATION"
        );
    }

    /// Distributions disagree on the case and on whether the value reads KDE or
    /// plasma, and some concatenate several names with colons.
    #[test]
    fn the_desktop_name_is_matched_loosely() {
        let env = env_of(&[("XDG_CURRENT_DESKTOP", "plasmawayland:KDE")]);
        assert_eq!(
            window_type_atom_from(env),
            "_KDE_NET_WM_WINDOW_TYPE_CRITICAL_NOTIFICATION"
        );
    }

    /// Hyprland sets `XDG_CURRENT_DESKTOP=Hyprland`, but a session started from
    /// a Plasma-configured user account can carry a stale KDE value, and neither
    /// Hyprland nor Sway would know what to do with KDE's atom.
    #[test]
    fn a_compositor_that_names_itself_does_not_get_kdes_atom() {
        for signature in ["HYPRLAND_INSTANCE_SIGNATURE", "SWAYSOCK"] {
            let env = env_of(&[(signature, "abc123"), ("XDG_CURRENT_DESKTOP", "KDE")]);
            assert_eq!(window_type_atom_from(env), "_NET_WM_WINDOW_TYPE_DOCK");
        }
    }

    #[test]
    fn anything_else_gets_dock() {
        let env = env_of(&[]);
        assert_eq!(window_type_atom_from(env), "_NET_WM_WINDOW_TYPE_DOCK");
    }

    #[test]
    fn the_state_hint_keeps_the_overlay_above_and_out_of_the_task_switcher() {
        let hints = atom_hints("_NET_WM_WINDOW_TYPE_DOCK");
        let state = hints
            .iter()
            .find(|(property, _)| *property == "_NET_WM_STATE")
            .expect("_NET_WM_STATE is written whatever the window type");
        assert_eq!(
            state.1,
            vec![
                "_NET_WM_STATE_ABOVE",
                "_NET_WM_STATE_STAYS_ON_TOP",
                "_NET_WM_STATE_SKIP_TASKBAR",
                "_NET_WM_STATE_SKIP_PAGER",
            ]
        );
    }

    /// A non-zero user time would read as "the user just did this", which is
    /// exactly the claim that makes a window manager hand over focus.
    #[test]
    fn the_user_time_hint_is_zero() {
        let (_, values) = CARDINAL_HINTS
            .iter()
            .find(|(property, _)| *property == "_NET_WM_USER_TIME")
            .expect("_NET_WM_USER_TIME is written");
        assert_eq!(*values, &[0]);
    }

    /// Motif hints are typed with their own atom. Written as `CARDINAL` they
    /// read back as nothing at all, so the property has to stay out of the list
    /// that is written that way.
    #[test]
    fn the_motif_hint_is_not_written_as_a_cardinal() {
        assert!(!CARDINAL_HINTS
            .iter()
            .any(|(property, _)| *property == MOTIF_HINTS.0));
    }
}
