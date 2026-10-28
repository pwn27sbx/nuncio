//! The session bus, used for two things: starting `nunciod` on demand (the activation file the
//! package installs makes the bus start it for us, exactly as the CLI does) and enabling its
//! `systemd --user` unit once, so it comes back after a logout.
//!
//! Both are best effort and never fatal: if the bus is not there, the clients keep trying the
//! Unix socket and report that error instead.

use std::sync::Once;

/// The daemon's bus name, as `packaging/io.github.pwn27sbx.nuncio.service` declares it.
pub const DAEMON_BUS_NAME: &str = "io.github.pwn27sbx.nuncio";
/// The daemon's user unit.
pub const DAEMON_UNIT: &str = "nunciod.service";

fn session() -> Option<zbus::blocking::Connection> {
    zbus::blocking::Connection::session().ok()
}

/// Ask the session bus to start the daemon. The bus starts it through the activation file the
/// package installs; without that file, or without a session bus, this does nothing and the
/// caller's next connection attempt reports the truth.
pub fn start_daemon() {
    let Some(bus) = session() else { return };
    let _ = bus.call_method(
        Some("org.freedesktop.DBus"),
        "/org/freedesktop/DBus",
        Some("org.freedesktop.DBus"),
        "StartServiceByName",
        &(DAEMON_BUS_NAME, 0u32),
    );
}

/// Enable the daemon's user unit through the systemd **user** manager: no root, no polkit, no
/// terminal. systemd writes `~/.config/systemd/user/default.target.wants/nunciod.service` for a
/// unit that lives in `/usr/lib/systemd/user`.
///
/// Best effort, and only once per process: a machine without systemd (a container, a Mac) simply
/// does not get the unit, and the clients still work through bus activation.
pub fn enable_daemon_unit() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let Some(bus) = session() else { return };
        let _ = bus.call_method(
            Some("org.freedesktop.systemd1"),
            "/org/freedesktop/systemd1",
            Some("org.freedesktop.systemd1.Manager"),
            "EnableUnitFiles",
            &(vec![DAEMON_UNIT], false, true),
        );
    });
}
